//! The interactive flash state machine: search → jump → cursor/selection → yank.
//!
//! All logic here is pure (no terminal or socket I/O), so `handle_key` is fully unit-testable.

use crate::buffer::Buffer;
use crate::hints::{self, Hint};
use crate::matcher;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Search,
    Cursor,
    Select,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stay in the picker and keep reading input.
    Continue,
    /// Copy this text to the clipboard and exit.
    Copy(String),
    /// Abort without copying.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

const HALF_PAGE_ROWS: isize = 10;

pub struct App {
    buffer: Buffer,
    keys: Vec<char>,
    theme: Theme,
    query: String,
    matches: Vec<crate::buffer::Span>,
    hints: Vec<Hint>,
    phase: Phase,
    cursor: usize,
    anchor: Option<usize>,
    linewise: bool,
    message: String,
    pending_g: bool,
    viewport_rows: usize,
    view_offset: usize,
}

impl App {
    pub fn new(buffer: Buffer, keys: Vec<char>, theme: Theme) -> Self {
        let mut app = Self {
            buffer,
            keys,
            theme,
            query: String::new(),
            matches: Vec::new(),
            hints: Vec::new(),
            phase: Phase::Search,
            cursor: 0,
            anchor: None,
            linewise: false,
            message: String::new(),
            pending_g: false,
            viewport_rows: 24,
            view_offset: 0,
        };
        app.cursor = app.buffer.row_head(app.buffer.last_row()).unwrap_or(0);
        app
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn hint_keys(&self) -> &[char] {
        &self.keys
    }

    /// Reset after a yank when the picker stays open.
    pub fn after_yank(&mut self, text: &str) {
        self.anchor = None;
        self.linewise = false;
        self.phase = Phase::Cursor;
        self.message = format!("copied {} chars", text.chars().count());
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn matches(&self) -> &[crate::buffer::Span] {
        &self.matches
    }

    pub fn hints(&self) -> &[Hint] {
        &self.hints
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn linewise(&self) -> bool {
        self.linewise
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cursor_position(&self) -> (usize, u16) {
        self.buffer.position_of(self.cursor)
    }

    pub fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    pub fn view_offset(&self) -> usize {
        self.view_offset
    }

    /// Called by the renderer: record the visible row count and keep the cursor on screen.
    pub fn update_viewport(&mut self, rows: usize) {
        self.viewport_rows = rows.max(1);
        let cursor_row = self.cursor_position().0;
        if cursor_row < self.view_offset {
            self.view_offset = cursor_row;
        } else if cursor_row >= self.view_offset + self.viewport_rows {
            self.view_offset = cursor_row + 1 - self.viewport_rows;
        }
        let last = self.buffer.last_row();
        self.view_offset = self.view_offset.min(last);
    }

    /// The label drawn on top of a match start, when that match carries a hint.
    pub fn hint_at(&self, cell_index: usize) -> Option<char> {
        self.hints
            .iter()
            .find(|hint| hint.span.start == cell_index)
            .map(|hint| hint.key)
    }

    /// Inclusive cell-index range of the active selection.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (start, end) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        if self.linewise {
            let (first_row, _) = self.buffer.position_of(start);
            let (last_row, _) = self.buffer.position_of(end);
            let (first_row, last_row) = self.buffer.logical_row_range(first_row, last_row);
            Some((
                self.buffer.row_head(first_row).unwrap_or(start),
                self.buffer.row_tail(last_row).unwrap_or(end),
            ))
        } else {
            Some((start, end))
        }
    }

    pub fn handle_key(&mut self, key: Key) -> Outcome {
        if self.pending_g {
            self.pending_g = false;
            if key == Key::Char('g') {
                self.move_to(self.buffer.row_head(0).unwrap_or(0));
                return Outcome::Continue;
            }
        }
        match self.phase {
            Phase::Search => self.handle_search_key(key),
            Phase::Cursor | Phase::Select => self.handle_cursor_key(key),
        }
    }

    fn handle_search_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Esc => Outcome::Cancel,
            Key::Ctrl('c') => Outcome::Cancel,
            Key::Backspace => {
                self.query.pop();
                self.refresh();
                Outcome::Continue
            }
            Key::Enter => match self.nearest_match() {
                Some(start) => {
                    self.jump(start);
                    Outcome::Continue
                }
                None => {
                    self.message = "no match".to_string();
                    Outcome::Continue
                }
            },
            Key::Char(ch) => {
                if let Some(hint) = self.hints.iter().find(|hint| hint.key == ch) {
                    self.jump(hint.span.start);
                    return Outcome::Continue;
                }
                if ch.is_control() {
                    return Outcome::Continue;
                }
                self.query.push(ch);
                self.refresh();
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn handle_cursor_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Esc => {
                if self.anchor.is_some() {
                    self.anchor = None;
                    self.linewise = false;
                    self.phase = Phase::Cursor;
                    self.message = "selection cleared".to_string();
                    return Outcome::Continue;
                }
                Outcome::Cancel
            }
            Key::Ctrl('c') => Outcome::Cancel,
            Key::Backspace => {
                self.phase = Phase::Search;
                self.anchor = None;
                self.linewise = false;
                self.message = "search".to_string();
                Outcome::Continue
            }
            Key::Enter | Key::Char('y') => {
                if self.anchor.is_none() {
                    self.message = "no selection: press v to start one".to_string();
                    return Outcome::Continue;
                }
                Outcome::Copy(self.yank_text())
            }
            Key::Char('v') => {
                if self.phase == Phase::Select && !self.linewise {
                    self.anchor = None;
                    self.phase = Phase::Cursor;
                } else {
                    self.anchor = Some(self.cursor);
                    self.linewise = false;
                    self.phase = Phase::Select;
                }
                Outcome::Continue
            }
            Key::Char('V') => {
                if self.phase == Phase::Select && self.linewise {
                    self.anchor = None;
                    self.phase = Phase::Cursor;
                } else {
                    self.anchor = Some(self.cursor);
                    self.linewise = true;
                    self.phase = Phase::Select;
                }
                Outcome::Continue
            }
            Key::Char('o') if self.anchor.is_some() => {
                let anchor = self.anchor.unwrap_or(self.cursor);
                self.anchor = Some(self.cursor);
                self.move_to(anchor);
                Outcome::Continue
            }
            Key::Char('g') => {
                self.pending_g = true;
                self.message = "g…".to_string();
                Outcome::Continue
            }
            Key::Char('G') => {
                self.move_to(self.buffer.row_head(self.buffer.last_row()).unwrap_or(0));
                Outcome::Continue
            }
            Key::Char('h') | Key::Left => {
                if let Some(previous) = self.buffer.previous_char(self.cursor) {
                    self.move_to(previous);
                }
                Outcome::Continue
            }
            Key::Char('l') | Key::Right => {
                if let Some(next) = self.buffer.next_char(self.cursor) {
                    self.move_to(next);
                }
                Outcome::Continue
            }
            Key::Char('j') | Key::Down => {
                self.move_vertical(1);
                Outcome::Continue
            }
            Key::Char('k') | Key::Up => {
                self.move_vertical(-1);
                Outcome::Continue
            }
            Key::Char('0') | Key::Home => {
                let (row, _) = self.cursor_position();
                self.move_to(self.buffer.row_head(row).unwrap_or(self.cursor));
                Outcome::Continue
            }
            Key::Char('^') => {
                let (row, _) = self.cursor_position();
                self.move_to(self.buffer.row_first_non_blank(row).unwrap_or(self.cursor));
                Outcome::Continue
            }
            Key::Char('$') | Key::End => {
                let (row, _) = self.cursor_position();
                self.move_to(self.buffer.row_tail(row).unwrap_or(self.cursor));
                Outcome::Continue
            }
            Key::Char('w') => {
                self.move_to(self.word_forward_start());
                Outcome::Continue
            }
            Key::Char('b') => {
                self.move_to(self.word_backward_start());
                Outcome::Continue
            }
            Key::Char('e') => {
                self.move_to(self.word_forward_end());
                Outcome::Continue
            }
            Key::Ctrl('u') => {
                self.move_vertical(-HALF_PAGE_ROWS);
                Outcome::Continue
            }
            Key::Ctrl('d') => {
                self.move_vertical(HALF_PAGE_ROWS);
                Outcome::Continue
            }
            Key::PageUp => {
                self.move_vertical(-(self.viewport_rows as isize));
                Outcome::Continue
            }
            Key::PageDown => {
                self.move_vertical(self.viewport_rows as isize);
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    /// The nearest match: the same target the first hint key selects.
    fn nearest_match(&self) -> Option<usize> {
        self.hints
            .first()
            .map(|hint| hint.span.start)
            .or_else(|| self.matches.first().map(|span| span.start))
    }

    fn refresh(&mut self) {
        self.matches = matcher::find_matches(&self.buffer, &self.query);
        self.hints = hints::assign(
            &self.buffer,
            &self.matches,
            self.anchor_position(),
            &self.keys,
        );
        self.message = if self.query.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            format!("no match for {:?}", self.query)
        } else {
            String::new()
        };
    }

    fn jump(&mut self, cell_index: usize) {
        self.move_to(cell_index);
        self.phase = Phase::Cursor;
        self.anchor = None;
        self.linewise = false;
        self.hints.clear();
        self.message = String::new();
    }

    fn yank_text(&self) -> String {
        match self.selection_range() {
            Some((start, end)) => self.buffer.extract_indices(start, end),
            None => String::new(),
        }
    }

    fn move_to(&mut self, cell_index: usize) {
        self.cursor = cell_index.min(self.buffer.cells().len().saturating_sub(1));
    }

    fn move_vertical(&mut self, delta: isize) {
        let (row, col) = self.cursor_position();
        let last = self.buffer.last_row() as isize;
        let target = (row as isize + delta).clamp(0, last) as usize;
        let max_col = self.buffer.row_cells(target).saturating_sub(1);
        let col = col.min(max_col);
        let landing = self
            .buffer
            .cell_index_at(target, col)
            .or_else(|| self.buffer.row_tail(target));
        if let Some(landing) = landing {
            self.move_to(landing);
        }
    }

    /// Anchor for ranking hints: the bottom of the visible screen, which is where the prompt and
    /// Herdr's copy-mode cursor start.
    fn anchor_position(&self) -> (usize, u16) {
        let row = self.buffer.last_row();
        self.buffer
            .row_first_non_blank(row)
            .or_else(|| self.buffer.row_head(row))
            .map_or((row, 0), |index| self.buffer.position_of(index))
    }

    fn word_forward_start(&self) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if class_of(cells[index].ch) != Class::Blank {
            let class = class_of(cells[index].ch);
            while index + 1 < cells.len() && class_of(cells[index + 1].ch) == class {
                index += 1;
            }
        }
        while index + 1 < cells.len() && class_of(cells[index + 1].ch) == Class::Blank {
            index += 1;
        }
        if index + 1 < cells.len() {
            index + 1
        } else {
            index
        }
    }

    fn word_backward_start(&self) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if index == 0 {
            return 0;
        }
        index -= 1;
        while index > 0 && class_of(cells[index].ch) == Class::Blank {
            index -= 1;
        }
        let class = class_of(cells[index].ch);
        while index > 0 && class_of(cells[index - 1].ch) == class {
            index -= 1;
        }
        while index > 0 && class_of(cells[index - 1].ch) == Class::Blank {
            index -= 1;
        }
        index
    }

    fn word_forward_end(&self) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if index + 1 >= cells.len() {
            return index;
        }
        index += 1;
        while index < cells.len() && class_of(cells[index].ch) == Class::Blank {
            index += 1;
        }
        if index >= cells.len() {
            return cells.len().saturating_sub(1);
        }
        let class = class_of(cells[index].ch);
        while index + 1 < cells.len() && class_of(cells[index + 1].ch) == class {
            index += 1;
        }
        index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    Word,
    Punctuation,
}

fn class_of(ch: char) -> Class {
    if ch.is_whitespace() {
        Class::Blank
    } else if ch.is_alphanumeric() || ch == '_' {
        Class::Word
    } else {
        Class::Punctuation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hints::{sanitize_keys, DEFAULT_HINT_KEYS};
    use pretty_assertions::assert_eq;

    fn app(text: &str) -> App {
        App::new(
            Buffer::from_text(text, None),
            sanitize_keys(DEFAULT_HINT_KEYS),
            Theme::default(),
        )
    }

    fn type_keys(app: &mut App, keys: &str) {
        for ch in keys.chars() {
            app.handle_key(Key::Char(ch));
        }
    }

    #[test]
    fn typing_labels_only_the_five_nearest_matches() {
        let mut app = app("foo foo foo foo foo foo foo");
        type_keys(&mut app, "fo");
        assert_eq!(app.phase(), Phase::Search);
        assert_eq!(app.matches().len(), 7);
        assert_eq!(app.hints().len(), 5);
        assert_eq!(
            app.hints().iter().map(|hint| hint.key).collect::<Vec<_>>(),
            ['a', 's', 'd', 'g', 'h']
        );
        assert_eq!(app.message(), "");
    }

    #[test]
    fn a_hint_key_jumps_and_wins_over_query_text() {
        let mut app = app("alpha beta gamma");
        type_keys(&mut app, "a");
        assert_eq!(app.hints().len(), 5);
        // 'd' is a live hint key; it jumps instead of extending the query.
        app.handle_key(Key::Char('d'));
        assert_eq!(app.phase(), Phase::Cursor);
        assert_eq!(app.query(), "a");
        assert_eq!(app.cursor_position(), (0, 9));
    }

    #[test]
    fn enter_jumps_to_the_nearest_match() {
        let mut app = app("one\ntwo");
        type_keys(&mut app, "o");
        app.handle_key(Key::Enter);
        assert_eq!(app.phase(), Phase::Cursor);
        assert_eq!(app.cursor_position(), (1, 2));
    }

    #[test]
    fn a_missing_match_is_reported_and_backspace_widens() {
        let mut app = app("alpha");
        type_keys(&mut app, "zz");
        assert_eq!(app.matches().len(), 0);
        assert_eq!(app.message(), "no match for \"zz\"");
        app.handle_key(Key::Backspace);
        assert_eq!(app.query(), "z");
    }

    #[test]
    fn escape_cancels_the_picker() {
        let mut app = app("alpha");
        assert_eq!(app.handle_key(Key::Esc), Outcome::Cancel);
        assert_eq!(app.handle_key(Key::Ctrl('c')), Outcome::Cancel);
    }

    #[test]
    fn charwise_selection_yanks_the_range_inclusive() {
        let mut app = app("hello world");
        type_keys(&mut app, "wor");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.phase(), Phase::Cursor);
        app.handle_key(Key::Char('v'));
        assert_eq!(app.phase(), Phase::Select);
        type_keys(&mut app, "ee");
        assert_eq!(
            app.handle_key(Key::Char('y')),
            Outcome::Copy("world".into())
        );
    }

    #[test]
    fn linewise_selection_covers_the_whole_line() {
        let mut app = app("first line\nsecond line\nthird");
        type_keys(&mut app, "rst");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 2));
        app.handle_key(Key::Char('V'));
        app.handle_key(Key::Char('j'));
        assert_eq!(
            app.handle_key(Key::Enter),
            Outcome::Copy("first line\nsecond line".into())
        );
    }

    #[test]
    fn selection_swaps_anchor_with_o() {
        let mut app = app("abcdef");
        type_keys(&mut app, "cd");
        app.handle_key(Key::Char('a'));
        app.handle_key(Key::Char('v'));
        type_keys(&mut app, "ll");
        app.handle_key(Key::Char('o'));
        assert_eq!(app.selection_range(), Some((2, 4)));
    }

    #[test]
    fn escape_clears_the_selection_before_leaving() {
        let mut app = app("abcdef");
        type_keys(&mut app, "cd");
        app.handle_key(Key::Char('a'));
        app.handle_key(Key::Char('v'));
        assert_eq!(app.handle_key(Key::Esc), Outcome::Continue);
        assert_eq!(app.phase(), Phase::Cursor);
        assert_eq!(app.selection_range(), None);
        assert_eq!(app.handle_key(Key::Esc), Outcome::Cancel);
    }

    #[test]
    fn yank_without_a_selection_keeps_the_picker_open() {
        let mut app = app("abcdef");
        type_keys(&mut app, "cd");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.handle_key(Key::Char('y')), Outcome::Continue);
        assert!(app.message().contains("no selection"));
    }

    #[test]
    fn backspace_returns_to_search_with_the_query_intact() {
        let mut app = app("abcdef");
        type_keys(&mut app, "cd");
        app.handle_key(Key::Char('a'));
        app.handle_key(Key::Backspace);
        assert_eq!(app.phase(), Phase::Search);
        assert_eq!(app.query(), "cd");
        assert_eq!(app.matches().len(), 1);
    }

    #[test]
    fn horizontal_motion_crosses_soft_wraps_but_not_lines() {
        let mut app = App::new(
            Buffer::from_text("abcde\nfg", Some(3)),
            sanitize_keys(DEFAULT_HINT_KEYS),
            Theme::default(),
        );
        type_keys(&mut app, "e");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (1, 1));
        app.handle_key(Key::Char('l'));
        assert_eq!(app.cursor_position(), (1, 1));
        app.handle_key(Key::Char('h'));
        assert_eq!(app.cursor_position(), (1, 0));
    }

    #[test]
    fn vertical_motion_keeps_the_column_when_it_fits() {
        let mut app = app("abcdef\ngh\nijklmn");
        type_keys(&mut app, "cd");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 2));
        app.handle_key(Key::Char('j'));
        assert_eq!(app.cursor_position(), (1, 1));
        app.handle_key(Key::Char('j'));
        assert_eq!(app.cursor_position(), (2, 1));
        app.handle_key(Key::Char('k'));
        assert_eq!(app.cursor_position(), (1, 1));
    }

    #[test]
    fn line_and_buffer_motions() {
        let mut app = app("  indented\nplain");
        type_keys(&mut app, "den");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 4));
        app.handle_key(Key::Char('$'));
        assert_eq!(app.cursor_position(), (0, 9));
        app.handle_key(Key::Char('0'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('^'));
        assert_eq!(app.cursor_position(), (0, 2));
        app.handle_key(Key::Char('g'));
        app.handle_key(Key::Char('g'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('G'));
        assert_eq!(app.cursor_position(), (1, 0));
    }

    #[test]
    fn word_motions_step_over_words_and_punctuation() {
        let mut app = app("foo-bar baz");
        type_keys(&mut app, "fo");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('w'));
        assert_eq!(app.cursor_position(), (0, 3));
        app.handle_key(Key::Char('w'));
        assert_eq!(app.cursor_position(), (0, 4));
        app.handle_key(Key::Char('w'));
        assert_eq!(app.cursor_position(), (0, 8));
        app.handle_key(Key::Char('b'));
        assert_eq!(app.cursor_position(), (0, 4));
        app.handle_key(Key::Char('e'));
        assert_eq!(app.cursor_position(), (0, 6));
    }

    #[test]
    fn half_page_motion_uses_ten_rows() {
        let text = (0..30)
            .map(|index| format!("row{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text);
        type_keys(&mut app, "row5");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position().0, 5);
        app.handle_key(Key::Ctrl('d'));
        assert_eq!(app.cursor_position().0, 15);
        app.handle_key(Key::Ctrl('u'));
        assert_eq!(app.cursor_position().0, 5);
    }

    #[test]
    fn viewport_follows_the_cursor() {
        let text = (0..40)
            .map(|index| format!("row{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text);
        app.update_viewport(10);
        type_keys(&mut app, "row39");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position().0, 39);
        assert_eq!(app.view_offset(), 30);
    }
}
