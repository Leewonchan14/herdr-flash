//! Entry point: capture the focused pane, run the flash picker, yank through OSC 52.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use herdr_flash::app::{App, Key, Outcome};
use herdr_flash::buffer::Buffer;
use herdr_flash::config::{self, Settings};
use herdr_flash::herdr_client::SocketClient;
use herdr_flash::{clipboard, ui};
use serde_json::Value;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "herdr-flash — flash.nvim-style labeled jump and yank for Herdr pane copy\n\
                     \n\
                     Usage: herdr-flash\n\
                     \n\
                     Run through the Herdr plugin popup (prefix+s):\n\
                     \x20 herdr plugin action invoke Leewonchan14.herdr-flash.open\n\
                     \n\
                     Environment: HERDR_SOCKET_PATH, HERDR_PLUGIN_CONTEXT_JSON,\n\
                     \x20            HERDR_PLUGIN_CONFIG_DIR, HERDR_PLUGIN_STATE_DIR"
                );
                return Ok(());
            }
            other => bail!("unrecognized argument {other:?} (try --help)"),
        }
    }
    run()
}

fn run() -> Result<()> {
    let socket_path = socket_path()?;
    let mut client = SocketClient::connect(&socket_path)?;
    let pane_id = resolve_pane_id(&mut client)?;
    let snapshot = client.read_visible_pane(&pane_id)?;
    let wrap_width = client
        .visible_pane_width(&pane_id)
        .map(|width| width.saturating_sub(1).max(1))
        .map_err(|error| {
            log_state(&format!("pane_width_unavailable: {error:#}"));
            error
        })
        .ok();
    let settings = config::load(config_dir().as_deref())?;
    log_state(&format!(
        "start pane={pane_id} revision={} rows={} wrap_width={wrap_width:?} hint_keys={}",
        snapshot.revision,
        snapshot.text.lines().count(),
        settings
            .hint_keys
            .iter()
            .map(|key| key.to_string())
            .collect::<Vec<_>>()
            .join("")
    ));

    let buffer = Buffer::from_text(&snapshot.text, wrap_width);
    let mut app = App::new(buffer, settings.hint_keys.clone(), settings.theme.clone());
    let outcome = run_picker(&mut app, &settings)?;
    match &outcome {
        Outcome::Copy(text) => log_state(&format!("outcome=copy chars={}", text.chars().count())),
        Outcome::Cancel => log_state("outcome=cancel"),
        Outcome::Continue => log_state("outcome=continue"),
    }
    if let Outcome::Copy(text) = outcome {
        if !text.is_empty() {
            clipboard::copy_to_clipboard(&text)?;
            if settings.copy_toast {
                match client.show_notification(&toast_title(&text)) {
                    Ok(()) => {}
                    Err(error) => log_state(&format!("notification_error: {error:#}")),
                }
            }
        }
    }
    Ok(())
}

fn run_picker(app: &mut App, settings: &Settings) -> Result<Outcome> {
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        let event = event::read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        let Some(key) = map_key(key) else {
            continue;
        };
        match app.handle_key(key) {
            Outcome::Continue => {}
            Outcome::Copy(text) if !settings.exit_on_yank => {
                clipboard::copy_to_clipboard(&text)?;
                app.after_yank(&text);
            }
            other => return Ok(other),
        }
    }
}

/// Restores the terminal even when the picker exits through an error.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn map_key(key: KeyEvent) -> Option<Key> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char(ch) if control => Some(Key::Ctrl(ch.to_ascii_lowercase())),
        KeyCode::Char(_) if alt => None,
        KeyCode::Char(ch) => Some(Key::Char(ch)),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Esc),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Home => Some(Key::Home),
        KeyCode::End => Some(Key::End),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        _ => None,
    }
}

fn socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let fallback = PathBuf::from(home).join(".config/herdr/herdr.sock");
    if fallback.exists() {
        return Ok(fallback);
    }
    bail!(
        "HERDR_SOCKET_PATH is not set and {} does not exist",
        fallback.display()
    )
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from)
}

/// Pane to capture: the plugin context's focused pane, else the pane env var, else `pane.current`.
fn resolve_pane_id(client: &mut SocketClient) -> Result<String> {
    if let Some(context) = std::env::var_os("HERDR_PLUGIN_CONTEXT_JSON") {
        let parsed: Value = serde_json::from_str(&context.to_string_lossy())
            .context("HERDR_PLUGIN_CONTEXT_JSON is not valid JSON")?;
        if let Some(pane_id) = parsed["focused_pane_id"].as_str() {
            return Ok(pane_id.to_string());
        }
    }
    if let Some(pane_id) = std::env::var_os("HERDR_PANE_ID") {
        return Ok(pane_id.to_string_lossy().into_owned());
    }
    client.focused_pane_id()
}

fn toast_title(text: &str) -> String {
    let mut chars = text.chars();
    let mut preview: String = chars.by_ref().take(15).collect();
    if chars.next().is_some() {
        preview.push_str("...");
    }
    format!("Copied: {preview}")
}

/// Append a line to `$HERDR_PLUGIN_STATE_DIR/herdr-flash.log` when Herdr provides a state dir.
fn log_state(message: &str) {
    let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") else {
        return;
    };
    let path = Path::new(&dir).join("herdr-flash.log");
    let line = format!("{message}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
}
