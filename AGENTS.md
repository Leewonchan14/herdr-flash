# Repository Guidelines

`herdr-flash` is a Herdr plugin: a full-size popup that captures the focused pane's **visible**
buffer (`pane.read` `source = "visible"`), labels up to five matches with one-key hints, lands its
own cursor on the picked target, and yanks a vim selection through OSC 52.

## Non-negotiable constraints

- **`pane.copy_mode_jump` only when the server implements it.** Probe once
  (`SocketClient::supports_copy_mode_jump`: an empty pane id answers `unknown variant` without the
  method and `pane_not_found` with it) and keep the picker's own copy cursor on every other build —
  the plugin must stay usable on a stock Herdr. The picker's grid *is* the pane's viewport grid
  (`Buffer` re-wraps the visible capture at the pane width), so a pick's `(row, col)` goes out as
  `viewport_row`/`viewport_col` with the captured row text as `expected_row`; never send coordinates
  from a different grid.
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
- `src/app.rs` — the pure state machine (`Search` → `Cursor`/`Select` → `Copy`/`Jump`/`Cancel`),
  motions and selection extraction. A pick returns `Outcome::Jump` when `set_handoff(true)` was
  called (Herdr's copy mode takes it), else it moves the picker's own cursor. The `Cursor`/`Select`
  phases are the picker's copy mode: `n`/`N` walk the matches, `y` copies the selection or the match
  under the cursor, and `/` goes back to search. No terminal or socket I/O, so `handle_key` is
  unit-testable.
- `src/jump.rs` — the stale-jump retry policy: Herdr rejects a jump whose captured revision or
  scroll identity moved, so a jump is re-issued against a fresh read when the picked row still holds
  the same text (compared in the picker's wrapped grid).
- `docs/herdr-copy-mode-jump.patch` — the Herdr 0.9.3 patch that adds `pane.copy_mode_jump`, the
  server→client push and the `copy_mode_key` binding. Keep it applying to stock `v0.9.3`.
- `src/ui.rs` — ratatui rendering: dimmed buffer, match highlight, hint labels over match starts,
  cursor, selection, and the width-clamped status line.
- `src/config.rs` / `src/theme.rs` — `$HERDR_PLUGIN_CONFIG_DIR/config.toml`, colors as named values,
  `#rrggbb`, or `0..255`.
- `src/herdr_client.rs` — Unix-socket JSON-RPC (`pane.read` `visible` + `recent_unwrapped`,
  `pane.layout`, `pane.current`, `pane.get`, `notification.show`, `pane.copy_mode_jump`), covered
  with `UnixListener` fixtures.
- `src/main.rs` — thin entry point: resolve the pane from `HERDR_PLUGIN_CONTEXT_JSON`, capture,
  probe the handoff, run the picker, emit OSC 52, deliver a jump, log state to
  `$HERDR_PLUGIN_STATE_DIR/herdr-flash.log`.
- `scripts/lab-e2e.py` — the isolated end-to-end proof for a stock Herdr (the picker's own copy
  mode). `scripts/lab-e2e-handoff.py` — the same for a patched Herdr (the handoff, the copy-mode
  key, and the fallback on a stock binary as case C). Named non-default sessions only.

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

End-to-end labs (never the default session):

```bash
HERDR_BIN=herdr python3 scripts/lab-e2e.py            # stock Herdr: the picker's own copy cursor
HERDR_BIN=herdr python3 scripts/lab-e2e-handoff.py    # patched Herdr: the copy-mode handoff
```

The handoff needs a Herdr built with `docs/herdr-copy-mode-jump.patch` (applies to `v0.9.3`; the
release build needs Zig 0.16 for the vendored libghostty-vt). Restart Herdr after swapping the
binary: the client and the server are the same binary and the patch bumps the wire protocol, so a
stock client cannot talk to a patched server. Keep the stock binary for case C
(`HERDR_STABLE_BIN=…`).

## Notes

- Do not commit `target/`, logs, or local editor files.
- Keep the settle delay after the OSC 52 write: Herdr forwards the sequence asynchronously, so an
  exit that races the forward can drop the yank over SSH.
- A yank's success is only provable end-to-end: assert the clipboard (local) or the forwarded
  OSC 52 (SSH) after a real popup run, not just the unit-level `Outcome::Copy`.
- Judge the E2E on the plugin's state log (`jump=delivered`, `outcome=copy`), not on status-line
  text: the client redraws the status cell by cell, so a phrase is not reliably contiguous in the
  stream. Assert screen text only for fragments that survive a full-line draw.
- Never print or assert the fallback wording before a query is typed: the empty-query status line
  carries neither `enter jump` nor the handoff note.
- Update this file only for durable repository-wide guidance.
