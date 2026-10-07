# herdr-flash

`prefix+shift+s` → **flash.nvim-style jump and yank for the focused Herdr pane**: type a query, every
match lights up, at most **five one-key hints** appear on the nearest hits, and one key lands on the
target — in **Herdr's own copy mode** when the running Herdr implements `pane.copy_mode_jump`, and in
the picker's own copy cursor on every other build (vim motions, selections, `y` through OSC 52).

A [Herdr](https://herdr.dev) plugin. MIT.

## Why this exists (the problem it solves)

The obvious tool for this is [`RooseveltAdvisors/herdr-leap`](https://github.com/RooseveltAdvisors/herdr-leap),
which promises to place Herdr's copy-mode cursor on a hinted word. **It cannot work on any released
Herdr.** Its final step calls `pane.copy_mode_jump`, a method that does not exist in the socket API:

```
error: pane.copy_mode_jump failed: Herdr API error invalid_request:
invalid request: unknown variant `pane.copy_mode_jump`
```

Herdr's maintainers closed the report as a feature request rather than a bug
([herdrdev/herdr#2249](https://github.com/herdrdev/herdr/issues/2249), *not planned*): copy mode
lives in the **client**, and the public API only exposes *read-only* copy helpers
(`pane.copy_search`, `pane.copy_motion`) that the client itself calls. `pane.send_keys` writes to
the pane's PTY, so it can never reach the client's copy mode either. There is no supported way for
a plugin to move Herdr's copy cursor today.

herdr-flash stops pretending in both directions. It ships the missing Herdr change
([`docs/herdr-copy-mode-jump.patch`](docs/herdr-copy-mode-jump.patch)) and uses
`pane.copy_mode_jump` whenever the running Herdr implements it: a pick closes the popup and Herdr's
own copy mode lands on the cell, so motions, search, selections and the yank are Herdr's. On any
other build — stock 0.9.x, where that method does not exist — the same picker keeps its own copy
cursor instead, so the workflow (jump to what you see, take it) still works everywhere.

## Install

```bash
herdr plugin install Leewonchan14/herdr-flash   # the Herdr server runs the build: cargo must be on its PATH
herdr server reload-config
```

Local development install (uses a prebuilt binary, no toolchain needed by the server):

```bash
cargo build --release --locked
herdr plugin link .
herdr server reload-config
```

Bind the action:

```toml
[[keys.command]]
key = "prefix+shift+s"
type = "plugin_action"
command = "Leewonchan14.herdr-flash.open"
description = "Flash (jump & yank)"
```

> `prefix+shift+s` is unbound in Herdr's defaults, so nothing is overridden. `prefix+s` (the
> Settings overlay) stays free.

## Usage

Press `prefix+shift+s` on any pane. The picker takes over the screen with that pane's visible text.

Picking a hint hands the cell to **Herdr's own copy mode** when the running Herdr implements
`pane.copy_mode_jump`: the popup closes and Herdr's copy cursor lands there, with Herdr's motions,
`/` search, selections and `y` — and the `copy_mode_key` binding lets you press `shift+s` inside copy
mode to flash again. On any other build the picker keeps its own copy cursor instead:

| Phase | Key | Action |
|---|---|---|
| search | any character | extend the query (labels avoid the characters that would extend it) |
| search | `Backspace` | widen the query |
| search | `Enter` | jump to the nearest match |
| search | `Esc` / `Ctrl-C` | cancel |
| cursor | `h j k l` / arrows | move by cell |
| cursor | `w b e` / `W B E` | word / WORD forward / back / end |
| cursor | `0 ^ $` | line start / first non-blank / line end |
| cursor | `g g` / `G` | first line / last line |
| cursor | `Ctrl-u` `Ctrl-d`, `PageUp` `PageDown` | half page / page |
| cursor | `v` / `V` | start charwise / linewise selection |
| cursor | `n` / `N` | next / previous match (wraps) |
| cursor | `y` or `Enter` | yank the match under the cursor |
| select | `o` | swap cursor and anchor |
| select | `y` or `Enter` | yank the selection to the clipboard |
| cursor / select | `/` `?` / `Backspace` | back to the search, query intact |
| cursor / select | `Esc` | clear the selection, or leave |

A yank in the picker's own copy mode closes the picker and leaves you back in your pane. Set
`exit_on_yank = false` to stay in the picker's copy mode instead (handy for grabbing several matches
in a row). Copying uses OSC 52, so it works locally and over SSH; Herdr forwards it (or converts it
to the native clipboard).

## Hints: at most five

- Labels come from `hint_keys` (default `asdgh` — the head of tmux-easy-motion's default alphabet).
- **Only the first five keys are used, and only five targets are labelled.** More matches stay
  highlighted but unlabelled; type more of the query to bring a distant hit into range.
- A key that could extend the current query is never a label, so typing never doubles as a jump:
  the cell right after a match is the only character a keystroke can append and still match, and
  those keys stay typeable.
- Targets are ranked by distance from the bottom of the screen (where the prompt and Herdr's copy
  cursor start), and labelled nearest-first, so `a` is always the closest hit.
- Matching is literal and smart-case (any uppercase character makes the search case-sensitive),
  the same rule as Herdr's own copy-mode search. A query may span a soft wrap, never a hard line.

## Configuration

`herdr plugin config-dir Leewonchan14.herdr-flash` prints the directory; create `config.toml` there:

```toml
hint_keys = "asdgh"      # at most five keys are used
exit_on_yank = true      # picker's own copy mode only: close after a yank (false keeps it)
copy_toast = true        # show a Herdr toast with the copied preview

[style]                  # named colors, #rrggbb, or 0..255
hint_bg = "magenta"
hint_fg = "black"
match_bg = "yellow"
match_fg = "black"
selection_bg = "cyan"
selection_fg = "black"
cursor_bg = "white"
cursor_fg = "black"
status_fg = "gray"
```

## How it works

1. `prefix+shift+s` runs the `open` action → `scripts/open-flash` → `herdr plugin pane open --placement
   popup --width 100% --height 100%`.
2. The popup reads the focused pane's **visible** buffer (`pane.read source=visible strip_ansi`),
   the pane's layout width (`pane.layout`) for wrapping, and its unwrapped output
   (`source=recent_unwrapped`): the visible read returns *screen* rows, so a wrap the terminal made
   is only joined when the unwrapped read confirms it, bottom up. The capture is frozen at open
   time, so you always land on what you saw.
3. Typing re-runs the matcher over the captured text and re-labels the five nearest hits.
4. A hint key (or `Enter`) hands the cell to Herdr's copy mode when the server implements
   `pane.copy_mode_jump` — the popup closes and the client applies the jump on the next frame — and
   otherwise lands the picker's cursor on the match, where motions and selections run on the
   captured grid.
5. `y` (in the picker's own copy mode) writes the selected text as an OSC 52 sequence and exits.
   Soft-wrapped rows are joined without a newline, so a wrapped URL or path yanks as one line.

The plugin never writes to the source pane: no PTY input, no scroll changes, no clipboard mutation
unless you press `y`.

## Limitations

- **Herdr's copy mode needs a patched Herdr.** Stock 0.9.x has no cursor-placement API, so the
  handoff is off there and the picker uses its own copy cursor instead (see "Why this exists" and
  `docs/herdr-copy-mode-jump.patch`).
- Capture is the visible viewport only; scrollback is out of scope (use Herdr's copy mode or a
  scrollback picker for that).
- Soft wraps are joined only where the unwrapped read confirms them, so a pane scrolled away from
  the bottom (or output changing between the two reads) degrades to hard screen rows.
- Labels are single keys capped at five; there is no two-key fallback.
- Wide (CJK) characters are handled by cell width; the label replaces a wide match start with
  `label + space` to keep the grid aligned.

## Lineage and credits

- Popup capture, visible-buffer model and the socket-client shape follow
  [`RooseveltAdvisors/herdr-leap`](https://github.com/RooseveltAdvisors/herdr-leap) (MIT), which in
  turn credits [`schasse/tmux-jump`](https://github.com/schasse/tmux-jump) for the jump UX. This
  repository exists because that plugin's jump cannot work on released Herdr; the credit for the
  architecture stays with its authors.
- The hint workflow — one search step, labelled targets, hint alphabet, closer matches first — is
  modelled on [`IngoMeyer441/tmux-easy-motion`](https://github.com/IngoMeyer441/tmux-easy-motion)
  (MIT) and [`ddzero2c/tmux-easymotion`](https://github.com/ddzero2c/tmux-easymotion) (MIT).
- Interaction style follows [`folke/flash.nvim`](https://github.com/folke/flash.nvim) (Apache-2.0).

## Development

```bash
cargo fmt -- --check
cargo test
cargo build --release --locked
cargo clippy --all-targets -- -D warnings
```

End-to-end proofs in isolated named sessions (never the default session):

```bash
HERDR_BIN=herdr python3 scripts/lab-e2e.py            # stock Herdr: the picker's own copy cursor
HERDR_BIN=herdr python3 scripts/lab-e2e-handoff.py    # patched Herdr: the handoff + copy-mode key
```

Both start `herdr --session herdr-flash-lab`, type a fixture into a pane, drive
`prefix+shift+s` → query → hint → `v` → `e` → `y`, and assert the yank landed (native clipboard
locally, OSC 52 over SSH) plus the plugin's state log (`outcome=copy` / `jump=delivered`). The
handoff script also proves the fallback: case C runs a stock Herdr as `HERDR_STABLE_BIN` and asserts
the pick stays in the picker's own copy mode.

### The Herdr patch

`docs/herdr-copy-mode-jump.patch` applies to Herdr 0.9.3 and adds `pane.copy_mode_jump`, the
server→client push behind it, and the `copy_mode_key` binding that lets a plugin action run inside
copy mode:

```bash
git clone https://github.com/herdrdev/herdr && cd herdr
git checkout v0.9.3
git apply /path/to/herdr-flash/docs/herdr-copy-mode-jump.patch
cargo build --release --locked        # needs Zig 0.16 for the vendored libghostty-vt
```

Put `target/release/herdr` on `PATH` and restart Herdr: the client and the server are the same
binary, and the patch bumps the wire protocol (`client protocol 23 is newer than server protocol 22`
until both are restarted). Keep the stock binary around for the fallback lab
(`HERDR_STABLE_BIN=… python3 scripts/lab-e2e-handoff.py`).

## 한국어 요약

- `prefix+shift+s` 를 누르면 현재 pane의 **보이는 화면**을 캡처한 flash 피커가 전체화면 팝업으로 열립니다.
- 검색어를 입력하면 일치하는 위치가 강조되고, **가장 가까운 최대 5개**에만 `a s d g h` 한 글자 힌트가 붙습니다.
- 힌트를 누르면 그 셀을 **Herdr 자체 copy mode** 로 넘깁니다(`pane.copy_mode_jump` 를 구현한 빌드에서: 팝업이 닫히고
  커서가 그 위치에 놓이며, 이후 이동/검색/선택/복사는 Herdr copy mode 그대로입니다). 그 API가 없는 빌드에서는 같은
  피커가 자체 copy 커서로 동작합니다(`v`/`V` 선택, `y` 복사, OSC 52).
- 기존 `herdr-leap` 이 실패한 이유: 릴리스된 Herdr에 `pane.copy_mode_jump` 가 없습니다
  ([herdrdev/herdr#2249](https://github.com/herdrdev/herdr/issues/2249), not planned). 이 저장소는 그 Herdr 패치를
  `docs/herdr-copy-mode-jump.patch` 로 동봉하고, 패치된 Herdr 에서는 API를 그대로 사용합니다.
- 힌트는 최대 5개로 제한됩니다(요구사항). 그보다 많은 일치 항목은 강조만 되고 라벨이 없으니 검색어를 더
  입력해 가까이 오게 하면 됩니다.

## License

MIT — see [LICENSE](LICENSE) for the third-party lineage notes.
