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
            if app.query().is_empty() {
                format!(
                    "flash · search · type a query · hints {} · esc cancel",
                    hint_keys(app)
                )
            } else {
                format!(
                    "flash · search {:?} · {} matches · hints {} · enter jump · backspace widen · esc cancel",
                    app.query(),
                    app.matches().len(),
                    hint_keys(app)
                )
            }
        }
        Phase::Cursor => {
            let (row, col) = app.cursor_position();
            format!(
                "flash · cursor {}:{} · v select · V line · y yank · backspace search · esc exit",
                row + 1,
                col + 1
            )
        }
        Phase::Select => {
            let (row, col) = app.cursor_position();
            let kind = if app.linewise() { "line" } else { "char" };
            format!(
                "flash · select ({kind}) {}:{} · y yank · o swap · esc clear",
                row + 1,
                col + 1
            )
        }
    };
    clamp(&base, width, &notice)
}

fn hint_keys(app: &App) -> String {
    app.hint_keys()
        .iter()
        .map(|key| key.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn clamp(base: &str, width: usize, notice: &str) -> (String, String) {
    if width == 0 {
        return (String::new(), String::new());
    }
    let notice_width = notice.chars().count();
    if notice_width >= width {
        return (truncate(notice, width), String::new());
    }
    let base_budget = width - notice_width;
    let base = truncate(base, base_budget);
    let notice = if base.chars().count() + notice_width <= width {
        notice.to_string()
    } else {
        truncate(notice, width.saturating_sub(base.chars().count()))
    };
    (base, notice)
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Key;
    use crate::buffer::Buffer;
    use crate::hints::{sanitize_keys, DEFAULT_HINT_KEYS};
    use pretty_assertions::assert_eq;

    fn app(text: &str) -> App {
        App::new(
            Buffer::from_text(text, None),
            sanitize_keys(DEFAULT_HINT_KEYS),
            Theme::default(),
        )
    }

    #[test]
    fn status_reports_query_matches_and_hint_keys() {
        let mut app = app("foo foo foo");
        app.handle_key(Key::Char('f'));
        let (status, notice) = status_line(&app, 120);
        assert!(status.contains("search \"f\""), "{status}");
        assert!(status.contains("3 matches"), "{status}");
        assert!(status.contains("hints a s d g h"), "{status}");
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
    fn cursor_status_reports_the_position() {
        let mut app = app("hello world");
        app.handle_key(Key::Char('w'));
        app.handle_key(Key::Char('o'));
        app.handle_key(Key::Enter);
        let (status, _) = status_line(&app, 120);
        assert!(status.contains("cursor 1:7"), "{status}");
    }
}
