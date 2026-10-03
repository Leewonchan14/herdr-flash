//! Cell-accurate model of a captured pane viewport.
//!
//! `pane.read` with `source = "visible"` returns the pane's visible text, one hard line per
//! logical line. A logical line can be wider than the pane, so it is re-wrapped here into visual
//! rows at the pane's wrap width. Every cursor, hint, selection and yank coordinate in this plugin
//! lives in that visual grid: `row` is a visual row, `col` is a display cell.
//!
//! Soft-wrapped rows are marked with `starts_line = false`, which is what keeps a yank from
//! inserting a newline inside a URL or path that only wrapped on screen.

use unicode_width::UnicodeWidthChar;

/// One terminal cell. Zero-width characters (combining marks) carry `width == 0` and belong to the
/// cell of the preceding character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub width: u16,
}

/// One visual row of the captured viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Row {
    /// Index of the first cell in the flat cell stream.
    pub start: usize,
    /// One past the last cell in the flat cell stream.
    pub end: usize,
    /// Display width of this row in cells.
    pub cells: u16,
    /// True when a hard newline precedes this row (a soft-wrap continuation is `false`).
    pub starts_line: bool,
}

/// One logical line: a half-open range of the flat cell stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Line {
    pub start: usize,
    pub end: usize,
}

/// A half-open range of cell indices, such as one query match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Buffer {
    cells: Vec<Cell>,
    rows: Vec<Row>,
    lines: Vec<Line>,
}

impl Buffer {
    /// Wrap `text` into a visual grid. `wrap_width` of `None` or `0` keeps one row per logical
    /// line.
    pub fn from_text(text: &str, wrap_width: Option<usize>) -> Self {
        let width = wrap_width.filter(|width| *width > 0).unwrap_or(usize::MAX);
        let mut buffer = Buffer::default();
        for logical in text.split('\n') {
            let line_start = buffer.cells.len();
            let mut row_start = buffer.cells.len();
            let mut row_cells: u16 = 0;
            let mut row_index = 0usize;
            for ch in logical.chars() {
                if ch.is_control() {
                    continue;
                }
                let advance = u16::try_from(ch.width().unwrap_or(0)).unwrap_or(0);
                if row_cells > 0 && usize::from(row_cells) + usize::from(advance) > width {
                    buffer.push_row(row_start, row_cells, row_index == 0);
                    row_index += 1;
                    row_start = buffer.cells.len();
                    row_cells = 0;
                }
                buffer.cells.push(Cell { ch, width: advance });
                row_cells = row_cells.saturating_add(advance);
            }
            buffer.push_row(row_start, row_cells, row_index == 0);
            buffer.lines.push(Line {
                start: line_start,
                end: buffer.cells.len(),
            });
        }
        buffer
    }

    fn push_row(&mut self, start: usize, cells: u16, starts_line: bool) {
        self.rows.push(Row {
            start,
            end: self.cells.len(),
            cells,
            starts_line,
        });
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Last row index, or 0 for an empty buffer.
    pub fn last_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    pub fn row(&self, row: usize) -> Option<&Row> {
        self.rows.get(row)
    }

    pub fn row_cells(&self, row: usize) -> u16 {
        self.rows.get(row).map_or(0, |row| row.cells)
    }

    pub fn row_text(&self, row: usize) -> String {
        let Some(row) = self.rows.get(row) else {
            return String::new();
        };
        self.cells[row.start..row.end]
            .iter()
            .map(|cell| cell.ch)
            .collect()
    }

    /// Row that owns a cell index.
    pub fn row_of(&self, cell_index: usize) -> usize {
        match self.rows.binary_search_by(|row| {
            if cell_index < row.start {
                std::cmp::Ordering::Greater
            } else if cell_index >= row.end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(row) => row,
            Err(_) => self.last_row(),
        }
    }

    /// Cell index whose display span covers `col` on `row`, if any.
    pub fn cell_index_at(&self, row: usize, col: u16) -> Option<usize> {
        let row = self.rows.get(row)?;
        let mut cursor: u16 = 0;
        for index in row.start..row.end {
            let width = self.cells[index].width;
            if col < cursor.saturating_add(width.max(1)) {
                return Some(index);
            }
            cursor = cursor.saturating_add(width);
        }
        None
    }

    /// First cell index of a row.
    pub fn row_head(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(|row| row.start)
    }

    /// Last cell index of a row.
    pub fn row_tail(&self, row: usize) -> Option<usize> {
        self.rows
            .get(row)
            .and_then(|row| (row.end > row.start).then_some(row.end - 1))
    }

    /// First non-blank cell index of a row, falling back to the row head.
    pub fn row_first_non_blank(&self, row: usize) -> Option<usize> {
        let row = self.rows.get(row)?;
        Some(
            (row.start..row.end)
                .find(|index| self.cells[*index].ch != ' ')
                .unwrap_or(row.start),
        )
    }

    /// Visual position of a cell index.
    pub fn position_of(&self, cell_index: usize) -> (usize, u16) {
        let row = self.row_of(cell_index);
        let Some(row_ref) = self.rows.get(row) else {
            return (0, 0);
        };
        let mut col: u16 = 0;
        for index in row_ref.start..cell_index.min(row_ref.end) {
            col = col.saturating_add(self.cells[index].width);
        }
        (row, col)
    }

    /// Display column of a cell index.
    pub fn column_of(&self, cell_index: usize) -> u16 {
        self.position_of(cell_index).1
    }

    /// Walk one character to the right, crossing a soft wrap but not a hard line break.
    pub fn next_char(&self, cell_index: usize) -> Option<usize> {
        let row = self.row_of(cell_index);
        let row_ref = self.rows.get(row)?;
        if cell_index + 1 < row_ref.end {
            return Some(cell_index + 1);
        }
        let next = self.rows.get(row + 1)?;
        (!next.starts_line).then_some(next.start)
    }

    /// Walk one character to the left, crossing a soft wrap but not a hard line break.
    pub fn previous_char(&self, cell_index: usize) -> Option<usize> {
        let row = self.row_of(cell_index);
        let row_ref = self.rows.get(row)?;
        if cell_index > row_ref.start {
            return Some(cell_index - 1);
        }
        let previous = self.rows.get(row.checked_sub(1)?)?;
        (!row_ref.starts_line).then_some(previous.end.checked_sub(1)?)
    }

    /// Characterwise text between two cell indices, inclusive, preserving hard line breaks and
    /// joining soft-wrapped rows without a newline.
    pub fn extract_indices(&self, first: usize, last: usize) -> String {
        if self.cells.is_empty() {
            return String::new();
        }
        let (first, last) = if first <= last {
            (first, last)
        } else {
            (last, first)
        };
        let start = first.min(self.cells.len() - 1);
        let end = last.min(self.cells.len() - 1);
        let start_row = self.row_of(start);
        let end_row = self.row_of(end);
        let mut text = String::new();
        for row in start_row..=end_row {
            let Some(row_ref) = self.rows.get(row) else {
                break;
            };
            if row > start_row && row_ref.starts_line {
                text.push('\n');
            }
            let low = if row == start_row {
                start.max(row_ref.start)
            } else {
                row_ref.start
            };
            let high = if row == end_row {
                (end + 1).min(row_ref.end)
            } else {
                row_ref.end
            };
            for index in low..high {
                text.push(self.cells[index].ch);
            }
        }
        text
    }

    /// Visual rows of the logical lines containing `first_row` and `last_row`.
    pub fn logical_row_range(&self, first_row: usize, last_row: usize) -> (usize, usize) {
        let (first_row, last_row) = if first_row <= last_row {
            (first_row, last_row)
        } else {
            (last_row, first_row)
        };
        let mut start = first_row.min(self.last_row());
        while start > 0 && !self.rows[start].starts_line {
            start -= 1;
        }
        let mut end = last_row.min(self.last_row());
        while end + 1 < self.rows.len() && !self.rows[end + 1].starts_line {
            end += 1;
        }
        (start, end)
    }

    /// Characterwise text between two visual positions, inclusive.
    pub fn extract(&self, first: (usize, u16), second: (usize, u16)) -> String {
        let (first, second) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        let Some(start) = self
            .cell_index_at(first.0, first.1)
            .or_else(|| self.row_head(first.0))
        else {
            return String::new();
        };
        let Some(end) = self
            .cell_index_at(second.0, second.1)
            .or_else(|| self.row_tail(second.0))
        else {
            return String::new();
        };
        self.extract_indices(start, end)
    }

    /// Linewise text for the given visual row range, inclusive.
    pub fn extract_rows(&self, first_row: usize, last_row: usize) -> String {
        let (first_row, last_row) = self.logical_row_range(first_row, last_row);
        let Some(start) = self.row_head(first_row) else {
            return String::new();
        };
        let Some(end) = self.row_tail(last_row) else {
            return String::new();
        };
        self.extract_indices(start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn wraps_logical_lines_at_cell_width() {
        let buffer = Buffer::from_text("abcdef\nxyz", Some(3));
        assert_eq!(buffer.row_count(), 3);
        assert_eq!(buffer.row_text(0), "abc");
        assert_eq!(buffer.row_text(1), "def");
        assert_eq!(buffer.row_text(2), "xyz");
        assert!(buffer.rows()[0].starts_line);
        assert!(!buffer.rows()[1].starts_line);
        assert!(buffer.rows()[2].starts_line);
    }

    #[test]
    fn keeps_wide_characters_whole_at_a_wrap_boundary() {
        let buffer = Buffer::from_text("a한b", Some(2));
        assert_eq!(buffer.row_text(0), "a");
        assert_eq!(buffer.row_text(1), "한");
        assert_eq!(buffer.row_text(2), "b");
        assert_eq!(buffer.row_cells(1), 2);
    }

    #[test]
    fn soft_wrapped_rows_join_without_a_newline_on_extract() {
        let buffer = Buffer::from_text("abcdef\nxy", Some(3));
        assert_eq!(buffer.extract((0, 0), (1, 2)), "abcdef");
        assert_eq!(buffer.extract((0, 0), (2, 1)), "abcdef\nxy");
        assert_eq!(buffer.extract_rows(0, 2), "abcdef\nxy");
    }

    #[test]
    fn positions_map_between_cells_and_grid_coordinates() {
        let buffer = Buffer::from_text("ab\ncdef", None);
        assert_eq!(buffer.position_of(0), (0, 0));
        assert_eq!(buffer.position_of(1), (0, 1));
        assert_eq!(buffer.position_of(2), (1, 0));
        assert_eq!(buffer.position_of(3), (1, 1));
        assert_eq!(buffer.cell_index_at(0, 1), Some(1));
        assert_eq!(buffer.cell_index_at(0, 2), None);
        assert_eq!(buffer.cell_index_at(1, 2), Some(4));
    }

    #[test]
    fn next_and_previous_char_cross_soft_wraps_only() {
        let buffer = Buffer::from_text("abcde\nfg", Some(3));
        assert_eq!(buffer.next_char(2), Some(3));
        assert_eq!(buffer.previous_char(3), Some(2));
        assert_eq!(buffer.next_char(4), None);
        assert_eq!(buffer.previous_char(5), None);
    }

    #[test]
    fn empty_lines_keep_their_row() {
        let buffer = Buffer::from_text("a\n\nb", None);
        assert_eq!(buffer.row_count(), 3);
        assert_eq!(buffer.row_text(1), "");
        assert_eq!(buffer.extract((0, 0), (2, 0)), "a\n\nb");
    }

    #[test]
    fn wide_characters_extract_as_a_unit() {
        let buffer = Buffer::from_text("가나다", None);
        assert_eq!(buffer.extract((0, 3), (0, 3)), "나");
        assert_eq!(buffer.row_cells(0), 6);
    }
}
