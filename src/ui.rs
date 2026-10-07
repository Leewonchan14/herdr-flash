//! Ratatui rendering for the flash picker.
//!
//! The captured viewport is drawn dimmed; every match of the query is highlighted, and up to five
//! matches carry a hint label drawn on top of the match start. A bottom status line reports the
//! phase, the query, the match count, the live hint keys and any message, clamped to the pane
//! width so it never wraps.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::app::{App, Phase};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    app.update_viewport(chunks[0].height as usize);
    draw_body(frame, app, chunks[0]);
    draw_status(frame, app, chunks[1]);
}

fn draw_body(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = &app.theme();
    let offset = app.view_offset();
    let rows = area.height as usize;
    let lines: Vec<Line<'static>> = (offset..offset + rows)
        .map(|row| render_row(app, row, theme))
        .collect();
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

fn render_row(app: &App, row: usize, theme: &Theme) -> Line<'static> {
    let buffer = app.buffer();
    let Some(row_ref) = buffer.row(row) else {
        return Line::default();
    };
    let cells = buffer.cells();
    let selection = app.selection_range();
    let cursor = app.cursor();
    let show_cursor = app.phase() != Phase::Search;
    let matches = app.matches();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style: Option<Style> = None;
    let mut text = String::new();
    for (index, cell) in cells
        .iter()
        .enumerate()
        .take(row_ref.end)
        .skip(row_ref.start)
    {
        let hint = if app.phase() == Phase::Search {
            app.hint_at(index)
        } else {
            None
        };
        let cell_style = if show_cursor && index == cursor {
            theme.cursor
        } else if hint.is_some() {
            theme.hint
        } else if selection.is_some_and(|(start, end)| index >= start && index <= end) {
            theme.selection
        } else if matches
            .iter()
            .any(|matched| index >= matched.start && index < matched.end)
        {
            theme.matched
        } else {
            theme.base
        };
        if style != Some(cell_style) {
            push_span(&mut spans, style, &mut text);
            style = Some(cell_style);
        }
        match hint {
            Some(label) => {
                text.push(label);
                if cell.width == 2 {
                    // Keep the cell grid aligned when the label replaces a wide character.
                    text.push(' ');
                }
            }
            None => text.push(cell.ch),
        }
    }
    push_span(&mut spans, style, &mut text);
    Line::from(spans)
}

fn push_span(spans: &mut Vec<Span<'static>>, style: Option<Style>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    let content = std::mem::take(text);
    match style {
        Some(style) => spans.push(Span::styled(content, style)),
        None => spans.push(Span::raw(content)),
    }
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme();
    let (status, notice) = status_line(app, area.width as usize);
    let mut spans = vec![Span::styled(status, theme.status)];
    if !notice.is_empty() {
        spans.push(Span::styled(notice, theme.notice));
    }
    Paragraph::new(Line::from(spans)).render(area, frame.buffer_mut());
}

/// Status text plus an optional notice, both already clamped to `width`.
pub fn status_line(app: &App, width: usize) -> (String, String) {
    let notice = if app.message().is_empty() {
        String::new()
    } else {
        format!(" · {}", app.message())
    };
    let base = match app.phase() {
        Phase::Search => {
            let hints = hint_segment(app);
            if app.query().is_empty() {
                format!("flash · search · type a query{hints} · esc cancel")
            } else {
                format!(
                    "flash · search {:?} · {} matches{hints} · enter jump · backspace widen · esc cancel",
                    app.query(),
                    app.matches().len(),
                )
            }
        }
        Phase::Cursor => {
            let (row, col) = app.cursor_position();
            format!(
                "flash · copy {}:{} · n/N match · v select · y copy · / search · esc exit",
                row + 1,
                col + 1
            )
        }
        Phase::Select => {
            let (row, col) = app.cursor_position();
            let kind = if app.linewise() { "line" } else { "char" };
            format!(
                "flash · copy ({kind}) {}:{} · y copy · o swap · esc clear",
                row + 1,
                col + 1
            )
        }
    };
    clamp(&base, width, &notice)
}

/// ` · hints a s d`, listing only the labels actually in play — empty when nothing can be picked
/// (no matches, or every key would extend the query).
fn hint_segment(app: &App) -> String {
    if app.matches().is_empty() {
        return String::new();
    }
    let keys: Vec<String> = app
        .hints()
        .iter()
        .map(|hint| hint.key.to_string())
        .collect();
    if keys.is_empty() {
        return String::new();
    }
    format!(" · hints {}", keys.join(" "))
}

fn clamp(base: &str, width: usize, notice: &str) -> (String, String) {
    if width == 0 {
        return (String::new(), String::new());
    }
    let notice_width = display_width(notice);
    if notice_width >= width {
        return (truncate(notice, width), String::new());
    }
    let base_budget = width - notice_width;
    let base = truncate(base, base_budget);
    let notice = if display_width(&base) + notice_width <= width {
        notice.to_string()
    } else {
        truncate(notice, width.saturating_sub(display_width(&base)))
    };
    (base, notice)
}

/// Cell width of `text`, so wide characters do not push the status line past the pane edge.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn truncate(text: &str, width: usize) -> String {
    let mut kept = String::new();
    let mut cells = 0;
    for ch in text.chars() {
        let advance = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cells + advance > width {
            break;
        }
        cells += advance;
        kept.push(ch);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Key;
    use crate::buffer::Buffer;
    use crate::hints::{sanitize_keys, DEFAULT_HINT_KEYS};
    use pretty_assertions::assert_eq;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app(text: &str) -> App {
        App::new(
            Buffer::from_text(text, None),
            sanitize_keys(DEFAULT_HINT_KEYS),
            Theme::default(),
        )
    }

    /// Render into a test terminal and return the screen rows as plain text.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        cells(app, width, height)
            .into_iter()
            .map(|row| row.concat())
            .collect()
    }

    /// Render into a test terminal and return one string per display cell.
    fn cells(app: &mut App, width: u16, height: u16) -> Vec<Vec<String>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        buffer
                            .cell((x, y))
                            .map(|cell| cell.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn hint_labels_are_drawn_over_the_match_starts() {
        let mut app = app("alpha beta gamma");
        app.handle_key(Key::Char('a'));
        let rows = screen(&mut app, 120, 4);
        assert_eq!(rows[0].trim_end(), "alphs betd ggmmh");
        assert!(rows[3].contains("hints a s d g h"), "{:?}", rows[3]);
    }

    #[test]
    fn a_wide_match_start_keeps_the_cell_grid_aligned() {
        let mut app = app("한글abc");
        app.handle_key(Key::Char('글'));
        let rows = cells(&mut app, 20, 3);
        // The label replaces both cells of the wide 글; the rest of the row keeps its columns.
        // ('a' follows the match in the text, so it stays typeable and the label is 's'.)
        assert_eq!(rows[0][0], "한");
        assert_eq!(rows[0][2], "s");
        assert_eq!(rows[0][3], " ");
        assert_eq!(rows[0][4], "a");
        assert_eq!(rows[0][5], "b");
        assert_eq!(rows[0][6], "c");
    }

    #[test]
    fn an_empty_capture_and_a_one_row_terminal_render_without_panicking() {
        let mut app = app("");
        let rows = screen(&mut app, 20, 3);
        assert_eq!(rows.len(), 3);
        app.handle_key(Key::Char('x'));
        let rows = screen(&mut app, 3, 1);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn the_cursor_phase_marks_the_cursor_cell() {
        let mut app = app("hello world");
        app.handle_key(Key::Char('w'));
        app.handle_key(Key::Char('o'));
        app.handle_key(Key::Enter);
        let rows = screen(&mut app, 40, 3);
        // The cursor sits on the 'o' of "world" (1-based 1:7 in the status line).
        assert!(rows[2].contains("copy 1:7"), "{:?}", rows[2]);
    }

    #[test]
    fn status_reports_query_matches_and_hint_keys() {
        let mut app = app("foo foo foo");
        app.handle_key(Key::Char('f'));
        let (status, notice) = status_line(&app, 120);
        assert!(status.contains("search \"f\""), "{status}");
        assert!(status.contains("3 matches"), "{status}");
        assert!(status.contains("hints a s d"), "{status}");
        assert!(notice.is_empty());
    }

    #[test]
    fn status_reports_no_match_as_a_notice() {
        let mut app = app("foo");
        app.handle_key(Key::Char('z'));
        let (_, notice) = status_line(&app, 120);
        assert_eq!(notice, " · no match for \"z\"");
    }

    #[test]
    fn status_never_exceeds_the_pane_width() {
        let mut app = app("foo foo foo");
        app.handle_key(Key::Char('f'));
        for width in [0, 1, 10, 40, 60, 200] {
            let (status, notice) = status_line(&app, width);
            assert!(
                status.chars().count() + notice.chars().count() <= width.max(1),
                "width {width}: {status:?} {notice:?}"
            );
        }
    }

    #[test]
    fn status_shows_only_the_labels_in_play() {
        // The 'd' that follows every "a" can extend the query, so it is not a label; the status
        // line lists just the two labels the two matches carry.
        let mut app = app("add dad");
        app.handle_key(Key::Char('a'));
        let (status, _) = status_line(&app, 120);
        assert!(status.contains("hints a s"), "{status}");
        assert!(!status.contains("hints a s d"), "{status}");
    }

    #[test]
    fn status_clamps_wide_characters_by_display_width() {
        let mut app = app("한글 한글 한글");
        app.handle_key(Key::Char('한'));
        for width in [16, 24, 40] {
            let (status, notice) = status_line(&app, width);
            let cells = display_width(&status) + display_width(&notice);
            assert!(cells <= width, "width {width}: {status:?} {notice:?}");
        }
    }

    #[test]
    fn cursor_status_reports_the_position() {
        let mut app = app("hello world");
        app.handle_key(Key::Char('w'));
        app.handle_key(Key::Char('o'));
        app.handle_key(Key::Enter);
        let (status, _) = status_line(&app, 120);
        assert!(status.contains("copy 1:7"), "{status}");
    }
}
