# Repository Guidelines

`herdr-flash` is a Herdr plugin: a full-size popup that captures the focused pane's **visible**
buffer (`pane.read` `source = "visible"`), labels up to five matches with one-key hints, lands its
own cursor on the picked target, and yanks a vim selection through OSC 52.

## Non-negotiable constraints

- **Never call `pane.copy_mode_jump`.** No released Herdr implements it (herdrdev/herdr#2249, closed
  as not planned). Copy mode is client-side; the public API has no cursor placement. The popup's own
  cursor is the product, and the README explains why.
- **The source pane is read-only.** Only `pane.read` and `pane.layout` touch the pane. No
  `pane.send_keys` / `pane.send_text` / `pane.scroll` / `pane.focus` — the picker must not move,
  type into, or scroll the pane it captured.
- **At most five hints** (`hints::MAX_HINTS`). The configured alphabet is truncated to five keys and
  only the five nearest matches are labelled. Do not add a two-key label fallback. Labels never use
  a key that would extend the current query (`hints::blocked_keys`): typing must not double as a
  jump.
- Clipboard writes are OSC 52 only (`clipboard::write_osc52`), never platform tools.
- Keep the lineage credit to `RooseveltAdvisors/herdr-leap`, `IngoMeyer441/tmux-easy-motion`,
  `ddzero2c/tmux-easymotion` and `schasse/tmux-jump` in README and LICENSE notes.

## Project shape

- The documented default binding is `prefix+shift+s` (`prefix+s` is Herdr's Settings overlay and
  must stay free). README, manifest description and `scripts/lab-e2e.py` must agree on it.
- `herdr-plugin.toml` — plugin manifest. Keep the plugin id (`Leewonchan14.herdr-flash`), the action
  command, the pane entrypoint and the binary name in sync. `open` → `scripts/open-flash` →
  `herdr plugin pane open --placement popup --width 100% --height 100%` → entrypoint `flash`.
- `src/buffer.rs` — the cell-accurate capture model: re-wraps logical lines at the pane width,
  tracks soft wraps (`Row::starts_line`), maps cells ⇄ `(row, col)`, and extracts text (soft-wrapped
  rows join without a newline). `visible` returns hard screen rows, so `merge_soft_wraps` recovers
  the real wraps by confirming whole logical lines against the pane's unwrapped read, bottom up. The
  load-bearing module; keep it covered by unit tests.
- `src/matcher.rs` — literal smart-case matching per logical line.
- `src/hints.rs` — hint keys (sanitized to five) and nearest-first label assignment.
- `src/app.rs` — the pure state machine (`Search` → `Cursor`/`Select` → `Copy`/`Cancel`), motions
  and selection extraction. No terminal or socket I/O, so `handle_key` is unit-testable.
- `src/ui.rs` — ratatui rendering: dimmed buffer, match highlight, hint labels over match starts,
  cursor, selection, and the width-clamped status line.
- `src/config.rs` / `src/theme.rs` — `$HERDR_PLUGIN_CONFIG_DIR/config.toml`, colors as named values,
  `#rrggbb`, or `0..255`.
- `src/herdr_client.rs` — Unix-socket JSON-RPC (`pane.read` `visible` + `recent_unwrapped`,
  `pane.layout`, `pane.current`, `notification.show`), covered with `UnixListener` fixtures.
- `src/main.rs` — thin entry point: resolve the pane from `HERDR_PLUGIN_CONTEXT_JSON`, capture, run
  the picker, emit OSC 52, log state to `$HERDR_PLUGIN_STATE_DIR/herdr-flash.log`.
- `scripts/lab-e2e.py` — the isolated end-to-end proof. Named non-default session only.

## Development process

TDD for behaviour changes: write the failing unit test first, then the smallest change that passes.
Keep tests on the pure modules (`buffer`, `matcher`, `hints`, `app`, `ui`, `config`, `theme`) and on
the socket client's request shaping.

```bash
cargo fmt -- --check
cargo test
cargo build --release --locked
cargo clippy --all-targets -- -D warnings
```

Local Herdr loop:

```bash
cargo build --release --locked
herdr plugin link .
herdr server reload-config
herdr plugin action invoke Leewonchan14.herdr-flash.open
```

End-to-end lab (never the default session):

```bash
python3 scripts/lab-e2e.py
```

## Notes

- Do not commit `target/`, logs, or local editor files.
- Keep the settle delay after the OSC 52 write: Herdr forwards the sequence asynchronously, so an
  exit that races the forward can drop the yank over SSH.
- A yank's success is only provable end-to-end: assert the clipboard (local) or the forwarded
  OSC 52 (SSH) after a real popup run, not just the unit-level `Outcome::Copy`.
- Update this file only for durable repository-wide guidance.
