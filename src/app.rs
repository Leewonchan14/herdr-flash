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
        // A capture that ends with a blank row has that row's head one past the last cell; keep the
        // cursor on a real cell so the motions that index `cells[cursor]` stay in bounds.
        let last_cell = app.buffer.cells().len().saturating_sub(1);
        app.cursor = app
            .buffer
            .row_head(app.buffer.last_row())
            .unwrap_or(0)
            .min(last_cell);
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
                self.refresh();
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
                    self.anchor.get_or_insert(self.cursor);
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
                    self.anchor.get_or_insert(self.cursor);
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
            Key::Char(ch @ ('w' | 'W' | 'b' | 'B' | 'e' | 'E')) => {
                let big = ch.is_ascii_uppercase();
                let target = match ch.to_ascii_lowercase() {
                    'w' => self.word_forward_start(big),
                    'b' => self.word_backward_start(big),
                    _ => self.word_forward_end(big),
                };
                self.move_to(target);
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
        let step = delta.signum();
        if step == 0 {
            return;
        }
        let last = self.buffer.last_row() as isize;
        let requested = (row as isize + delta).clamp(0, last) as usize;
        // Blank rows carry no cells, so the cursor cannot land on one: keep going in the direction
        // of travel and fall back the other way, which is what lets a page motion clamped into a
        // blank tail still reach the last row with text.
        let Some(target) = self
            .nearest_row_with_cells(requested, step)
            .or_else(|| self.nearest_row_with_cells(requested, -step))
        else {
            return;
        };
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

    /// First row with cells reached from `start` walking by `step`.
    fn nearest_row_with_cells(&self, start: usize, step: isize) -> Option<usize> {
        let last = self.buffer.last_row() as isize;
        let mut index = start as isize;
        while (0..=last).contains(&index) {
            if self.buffer.row_cells(index as usize) > 0 {
                return Some(index as usize);
            }
            index += step;
        }
        None
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

    fn word_forward_start(&self, big: bool) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if class_of(cells[index].ch, big) != Class::Blank {
            let class = class_of(cells[index].ch, big);
            // A soft wrap keeps the word going; a hard line break ends it like whitespace does.
            while let Some(next) = self.buffer.next_char(index) {
                if class_of(cells[next].ch, big) != class {
                    break;
                }
                index = next;
            }
        }
        while index + 1 < cells.len() && class_of(cells[index + 1].ch, big) == Class::Blank {
            index += 1;
        }
        if index + 1 < cells.len() {
            index + 1
        } else {
            index
        }
    }

    fn word_backward_start(&self, big: bool) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if index == 0 {
            return 0;
        }
        index -= 1;
        while index > 0 && class_of(cells[index].ch, big) == Class::Blank {
            index -= 1;
        }
        let class = class_of(cells[index].ch, big);
        while let Some(previous) = self.buffer.previous_char(index) {
            if class_of(cells[previous].ch, big) != class {
                break;
            }
            index = previous;
        }
        index
    }

    fn word_forward_end(&self, big: bool) -> usize {
        let cells = self.buffer.cells();
        let mut index = self.cursor;
        if index + 1 >= cells.len() {
            return index;
        }
        index += 1;
        while index < cells.len() && class_of(cells[index].ch, big) == Class::Blank {
            index += 1;
        }
        if index >= cells.len() {
            return cells.len().saturating_sub(1);
        }
        let class = class_of(cells[index].ch, big);
        while let Some(next) = self.buffer.next_char(index) {
            if class_of(cells[next].ch, big) != class {
                break;
            }
            index = next;
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

/// `big` collapses the word classes the way vim's `W`, `B` and `E` do: a WORD is any run of
/// non-blank characters, punctuation included.
fn class_of(ch: char, big: bool) -> Class {
    if ch.is_whitespace() {
        Class::Blank
    } else if !(big || ch.is_alphanumeric() || ch == '_') {
        Class::Punctuation
    } else {
        Class::Word
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
    fn big_word_motions_step_over_whitespace_delimited_words() {
        let mut app = app("foo-bar.baz qux-quux quux");
        type_keys(&mut app, "foo");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 0));
        // W treats the whole punctuation run as part of the WORD.
        app.handle_key(Key::Char('W'));
        assert_eq!(app.cursor_position(), (0, 12));
        app.handle_key(Key::Char('W'));
        assert_eq!(app.cursor_position(), (0, 21));
        app.handle_key(Key::Char('B'));
        assert_eq!(app.cursor_position(), (0, 12));
        app.handle_key(Key::Char('B'));
        assert_eq!(app.cursor_position(), (0, 0));
        // E lands on the last character of the WORD.
        app.handle_key(Key::Char('E'));
        assert_eq!(app.cursor_position(), (0, 10));
    }

    #[test]
    fn big_word_motions_cross_lines() {
        let mut app = app("alpha-beta\n  gamma");
        type_keys(&mut app, "alpha");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('E'));
        assert_eq!(app.cursor_position(), (0, 9));
        app.handle_key(Key::Char('W'));
        assert_eq!(app.cursor_position(), (1, 2));
    }

    #[test]
    fn backward_word_motions_never_land_on_blanks() {
        // `b`/`B` from mid-word go to the start of that word, not one cell past it into the space.
        let mut app = app("foo baz");
        type_keys(&mut app, "z");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 6));
        app.handle_key(Key::Char('b'));
        assert_eq!(app.cursor_position(), (0, 4));
        app.handle_key(Key::Char('b'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('W'));
        assert_eq!(app.cursor_position(), (0, 4));
        app.handle_key(Key::Char('l'));
        app.handle_key(Key::Char('B'));
        assert_eq!(app.cursor_position(), (0, 4));
    }

    #[test]
    fn word_motions_treat_a_hard_line_break_as_a_separator() {
        // A line break separates words like whitespace: a run must not continue into the row above,
        // while `w`/`W` still cross to the next row's first word.
        let mut app = app("alpha-beta.gamma delta\nnext");
        type_keys(&mut app, "del");
        app.handle_key(Key::Enter);
        assert_eq!(app.cursor_position(), (0, 17));
        app.handle_key(Key::Char('B'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('E'));
        assert_eq!(app.cursor_position(), (0, 15));
        app.handle_key(Key::Char('w'));
        assert_eq!(app.cursor_position(), (0, 17));
        app.handle_key(Key::Char('W'));
        assert_eq!(app.cursor_position(), (1, 0));
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

    #[test]
    fn vertical_motion_crosses_blank_rows() {
        // Blank screen rows arrive as empty capture lines, and the cursor cannot sit on a row
        // without cells, so j/k must keep going to the nearest row that has some.
        let mut app = app("abc\n\nxyz");
        type_keys(&mut app, "abc");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::Char('j'));
        assert_eq!(app.cursor_position(), (2, 0));
        app.handle_key(Key::Char('k'));
        assert_eq!(app.cursor_position(), (0, 0));
    }

    #[test]
    fn page_motion_reaches_the_last_row_with_text() {
        let mut app = app("row0\nrow1\n\n\n");
        type_keys(&mut app, "row0");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.cursor_position(), (0, 0));
        app.handle_key(Key::PageDown);
        assert_eq!(app.cursor_position(), (1, 0));
    }

    #[test]
    fn backspace_from_cursor_restores_the_hint_labels() {
        let mut app = app("alpha beta");
        type_keys(&mut app, "al");
        app.handle_key(Key::Char('a'));
        assert_eq!(app.phase(), Phase::Cursor);
        app.handle_key(Key::Backspace);
        assert_eq!(app.phase(), Phase::Search);
        assert_eq!(app.query(), "al");
        assert_eq!(app.hints().len(), 1);
        assert_eq!(app.hints()[0].key, 'a');
    }

    #[test]
    fn switching_between_v_and_v_keeps_the_anchor() {
        // vim switches mode without dropping the visual anchor: `V` after `v` (and `v` after
        // `V`) must keep the selection, not collapse it to the cursor.
        let mut app = app("abcd\nefgh");
        type_keys(&mut app, "abcd");
        app.handle_key(Key::Char('a'));
        app.handle_key(Key::Char('v'));
        app.handle_key(Key::Char('j'));
        assert_eq!(app.selection_range(), Some((0, 4)));
        app.handle_key(Key::Char('V'));
        assert_eq!(app.selection_range(), Some((0, 7)));
        app.handle_key(Key::Char('v'));
        assert_eq!(app.selection_range(), Some((0, 4)));
    }
}
