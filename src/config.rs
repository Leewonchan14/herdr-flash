//! Settings loaded from `$HERDR_PLUGIN_CONFIG_DIR/config.toml`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hints::{sanitize_keys, DEFAULT_HINT_KEYS};
use crate::theme::{styled, Theme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Hint keys. At most five are used; extras are ignored.
    pub hint_keys: Vec<char>,
    /// Close the picker right after a yank.
    pub exit_on_yank: bool,
    /// Show a Herdr toast with the copied preview.
    pub copy_toast: bool,
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hint_keys: sanitize_keys(DEFAULT_HINT_KEYS),
            exit_on_yank: false,
            copy_toast: true,
            theme: Theme::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    hint_keys: Option<String>,
    exit_on_yank: Option<bool>,
    copy_toast: Option<bool>,
    style: RawStyle,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawStyle {
    hint_fg: Option<String>,
    hint_bg: Option<String>,
    match_fg: Option<String>,
    match_bg: Option<String>,
    cursor_fg: Option<String>,
    cursor_bg: Option<String>,
    selection_fg: Option<String>,
    selection_bg: Option<String>,
    status_fg: Option<String>,
}

/// Load settings from `<config_dir>/config.toml`. A missing file yields the defaults.
pub fn load(config_dir: Option<&Path>) -> Result<Settings> {
    let Some(dir) = config_dir else {
        return Ok(Settings::default());
    };
    let path = dir.join("config.toml");
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let raw: RawConfig =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    compile(raw)
}

fn compile(raw: RawConfig) -> Result<Settings> {
    let defaults = Settings::default();
    let keys = raw
        .hint_keys
        .as_deref()
        .map(sanitize_keys)
        .filter(|keys| !keys.is_empty())
        .unwrap_or(defaults.hint_keys);
    let theme = Theme {
        hint: styled(
            defaults.theme.hint,
            raw.style.hint_fg.as_deref(),
            raw.style.hint_bg.as_deref(),
        )?,
        matched: styled(
            defaults.theme.matched,
            raw.style.match_fg.as_deref(),
            raw.style.match_bg.as_deref(),
        )?,
        cursor: styled(
            defaults.theme.cursor,
            raw.style.cursor_fg.as_deref(),
            raw.style.cursor_bg.as_deref(),
        )?,
        selection: styled(
            defaults.theme.selection,
            raw.style.selection_fg.as_deref(),
            raw.style.selection_bg.as_deref(),
        )?,
        status: styled(defaults.theme.status, raw.style.status_fg.as_deref(), None)?,
        ..defaults.theme.clone()
    };
    Ok(Settings {
        hint_keys: keys,
        exit_on_yank: raw.exit_on_yank.unwrap_or(defaults.exit_on_yank),
        copy_toast: raw.copy_toast.unwrap_or(defaults.copy_toast),
        theme,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;

    #[test]
    fn defaults_use_five_hint_keys() {
        let settings = Settings::default();
        assert_eq!(settings.hint_keys, ['a', 's', 'd', 'g', 'h']);
        assert!(
            !settings.exit_on_yank,
            "a yank stays in copy mode by default"
        );
        assert!(settings.copy_toast);
    }

    #[test]
    fn config_overrides_keys_and_style() {
        let settings = compile(
            toml::from_str(
                r##"
                hint_keys = "jkl;"
                exit_on_yank = true
                copy_toast = false
                [style]
                hint_bg = "#ff007c"
                match_bg = "blue"
                "##,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(settings.hint_keys, ['j', 'k', 'l', ';']);
        assert!(settings.exit_on_yank);
        assert!(!settings.copy_toast);
        assert_eq!(settings.theme.hint.bg, Some(Color::Rgb(0xff, 0x00, 0x7c)));
        assert_eq!(settings.theme.matched.bg, Some(Color::Blue));
    }

    #[test]
    fn more_than_five_hint_keys_are_truncated() {
        let settings = compile(toml::from_str(r#"hint_keys = "asdfghjkl""#).unwrap()).unwrap();
        assert_eq!(settings.hint_keys, ['a', 's', 'd', 'f', 'g']);
    }

    #[test]
    fn an_unusable_alphabet_falls_back_to_the_default() {
        let settings = compile(toml::from_str(r#"hint_keys = "  ""#).unwrap()).unwrap();
        assert_eq!(settings.hint_keys, sanitize_keys(DEFAULT_HINT_KEYS));
    }

    #[test]
    fn a_bad_color_is_an_error() {
        let raw = toml::from_str("[style]\nhint_bg = \"nope\"\n").unwrap();
        assert!(compile(raw).is_err());
    }
}
