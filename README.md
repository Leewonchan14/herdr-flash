# herdr-flash

`prefix+shift+s` → **flash.nvim-style jump and yank for the focused Herdr pane**: type a query, every
match lights up, at most **five one-key hints** appear on the nearest hits, one key lands the
cursor, vim motions extend a selection, `y` copies it through OSC 52.

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

herdr-flash therefore stops pretending: it brings the copy cursor **into its own full-size popup**,
which captures the pane's visible text, labels matches, and yanks for real. You get the workflow you
wanted (jump to what you see, take it) without depending on an API that does not exist.

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

| Phase | Key | Action |
|---|---|---|
| search | any character | extend the query (a live hint key jumps instead) |
| search | `Backspace` | widen the query |
| search | `Enter` | jump to the nearest match |
| search | `Esc` / `Ctrl-C` | cancel |
| cursor | `h j k l` / arrows | move by cell |
| cursor | `w b e` | word forward / back / end |
| cursor | `0 ^ $` | line start / first non-blank / line end |
| cursor | `g g` / `G` | first line / last line |
| cursor | `Ctrl-u` `Ctrl-d`, `PageUp` `PageDown` | half page / page |
| cursor | `v` / `V` | start charwise / linewise selection |
| select | `o` | swap cursor and anchor |
| select | `y` or `Enter` | yank the selection to the clipboard |
| cursor / select | `Backspace` | back to the search, query intact |
| cursor / select | `Esc` | clear the selection, or leave |

The yank closes the picker (set `exit_on_yank = false` to keep it open for several grabs). Copying
uses OSC 52, so it works locally and over SSH; Herdr forwards it (or converts it to the native
clipboard).

## Hints: at most five

- Labels come from `hint_keys` (default `asdgh` — the head of tmux-easy-motion's default alphabet).
- **Only the first five keys are used, and only five targets are labelled.** More matches stay
  highlighted but unlabelled; type more of the query to bring a distant hit into range.
- Targets are ranked by distance from the bottom of the screen (where the prompt and Herdr's copy
  cursor start), and labelled nearest-first, so `a` is always the closest hit.
- Matching is literal and smart-case (any uppercase character makes the search case-sensitive),
  the same rule as Herdr's own copy-mode search. A query may span a soft wrap, never a hard line.

## Configuration

`herdr plugin config-dir Leewonchan14.herdr-flash` prints the directory; create `config.toml` there:

```toml
hint_keys = "asdgh"      # at most five keys are used
exit_on_yank = true      # close the picker after a yank
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
4. A hint key (or `Enter`) lands the picker's cursor on the match; motions and selections run on the
   captured grid.
5. `y` writes the selected text as an OSC 52 sequence and exits. Soft-wrapped rows are joined
   without a newline, so a wrapped URL or path yanks as one line.

The plugin never writes to the source pane: no PTY input, no scroll changes, no clipboard mutation
unless you press `y`.

## Limitations

- **The cursor is the popup's, not Herdr's copy-mode cursor** — see "Why this exists". Nothing in
  the 0.9.x API can move the latter.
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

End-to-end proof in an isolated named session (never the default session):

```bash
python3 scripts/lab-e2e.py
```

It starts `herdr --session herdr-flash-lab`, types a fixture into a pane, drives
`prefix+shift+s` → query → hint → `v` → `e` → `y`, then asserts that the yank landed (native clipboard
locally, OSC 52 over SSH) and that the plugin's state log recorded `outcome=copy`.

## 한국어 요약

- `prefix+shift+s` 를 누르면 현재 pane의 **보이는 화면**을 캡처한 flash 피커가 전체화면 팝업으로 열립니다.
- 검색어를 입력하면 일치하는 위치가 강조되고, **가장 가까운 최대 5개**에만 `a s d g h` 한 글자 힌트가 붙습니다.
- 힌트를 누르면 커서가 그 위치로 이동하고, `v`/`V` 로 선택한 뒤 `y` 로 클립보드에 복사합니다(OSC 52).
- 기존 `herdr-leap` 이 실패한 이유: 릴리스된 Herdr에 존재하지 않는 `pane.copy_mode_jump` API를 호출합니다
  ([herdrdev/herdr#2249](https://github.com/herdrdev/herdr/issues/2249), not planned). Herdr의 copy mode는
  클라이언트에 있고 플러그인이 커서를 옮길 공개 API가 없어서, 이 플러그인은 팝업 안에 자체 커서를 둡니다.
- 힌트는 최대 5개로 제한됩니다(요구사항). 그보다 많은 일치 항목은 강조만 되고 라벨이 없으니 검색어를 더
  입력해 가까이 오게 하면 됩니다.

## License

MIT — see [LICENSE](LICENSE) for the third-party lineage notes.
