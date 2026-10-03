//! Colors for the flash picker.

use anyhow::{bail, Context, Result};
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Pane text that is neither a match nor a hint.
    pub base: Style,
    /// Every occurrence of the query.
    pub matched: Style,
    /// The hint label drawn on top of a match start.
    pub hint: Style,
    /// The cursor in the cursor and selection phases.
    pub cursor: Style,
    /// The active selection.
    pub selection: Style,
    /// The status line.
    pub status: Style,
    /// Error or hint text on the status line.
    pub notice: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            base: Style::default().add_modifier(Modifier::DIM),
            matched: Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            hint: Style::default()
                .fg(Color::Black)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            cursor: Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
            selection: Style::default().fg(Color::Black).bg(Color::Cyan),
            status: Style::default().fg(Color::Gray),
            notice: Style::default().fg(Color::Yellow),
        }
    }
}

/// Parse `black`, `red`, `#rrggbb`, or `0..=255` into a ratatui color.
pub fn parse_color(value: &str) -> Result<Color> {
    let value = value.trim();
    let named = match value.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        _ => None,
    };
    if let Some(color) = named {
        return Ok(color);
    }
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 {
            bail!("expected #rrggbb, got {value:?}");
        }
        let parsed =
            u32::from_str_radix(hex, 16).with_context(|| format!("invalid color {value:?}"))?;
        return Ok(Color::Rgb(
            ((parsed >> 16) & 0xff) as u8,
            ((parsed >> 8) & 0xff) as u8,
            (parsed & 0xff) as u8,
        ));
    }
    if let Ok(index) = value.parse::<u8>() {
        return Ok(Color::Indexed(index));
    }
    bail!("unknown color {value:?}")
}

/// Apply `fg`/`bg` overrides to a base style.
pub fn styled(base: Style, fg: Option<&str>, bg: Option<&str>) -> Result<Style> {
    let mut style = base;
    if let Some(fg) = fg {
        style = style.fg(parse_color(fg)?);
    }
    if let Some(bg) = bg {
        style = style.bg(parse_color(bg)?);
    }
    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_named_hex_and_indexed_colors() {
        assert_eq!(parse_color("magenta").unwrap(), Color::Magenta);
        assert_eq!(parse_color("#3e68d7").unwrap(), Color::Rgb(0x3e, 0x68, 0xd7));
        assert_eq!(parse_color("42").unwrap(), Color::Indexed(42));
        assert!(parse_color("nope").is_err());
        assert!(parse_color("#12345").is_err());
    }

    #[test]
    fn overrides_only_the_given_channels() {
        let base = Style::default().add_modifier(Modifier::DIM);
        let style = styled(base, Some("black"), Some("#ff007c")).unwrap();
        assert_eq!(style.fg, Some(Color::Black));
        assert_eq!(style.bg, Some(Color::Rgb(0xff, 0x00, 0x7c)));
        assert!(style.add_modifier.contains(Modifier::DIM));
    }
}
