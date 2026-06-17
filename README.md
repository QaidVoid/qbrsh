# qbrsh

A fast, keyboard-driven web browser in Rust, built on WebKitGTK 6 / GTK 4 with a
hand-rolled Elm-style (TEA) core.

## Architecture

All application state lives in one owned `State`. Every source of change (key
presses, WebKit signals, IPC, worker results) produces a `Msg` drained by a
single consumer on glib's main context. The pure `update(&mut State, Msg) ->
Vec<Effect>` is the only place state mutates. It returns `Effect` values that an
effect runner carries out, and results return as new messages. There is no
shared interior mutability, no re-entrancy, and no polling timers.

```
sources ─▶ Msg queue ─▶ update(&mut State) ─▶ [Effect] ─▶ runner ─▶ results back as Msg
```

- `src/core/`: the engine-agnostic TEA core (state, msg, effect, update,
  command, key/trie/bindings, completion), fully unit-tested without GTK.
- `src/engine/`: the `EngineView` trait and the WebKitGTK 6 backend (the only
  place `webkit6` types appear).
- `src/input/`, `src/ui/`, `src/app.rs`: GDK key translation, the window, and
  the effect runner that drives GTK/WebKit.
- `src/history.rs`: browsing history on a dedicated SQLite writer thread.
- `src/plugin.rs`: the Rune plugin runtime.

## Build & run

Requires `gtk4` and `webkitgtk-6.0` (and `gst-plugins-good` + `gst-libav` for
media). Then:

```
cargo run
```

## Keybindings (Normal mode)

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| `h j k l` | scroll | `f` / `F` | hint: follow / open in tab |
| `gg` / `G` | top / bottom | `H` / `L` | back / forward |
| `<C-f>`/`<C-b>` | page down/up | `r` / `R` | reload / hard reload |
| `<C-d>`/`<C-u>` | half page | `J` / `K` | next / prev tab |
| `o` / `O` | open / open current URL | `d` / `u` | close / reopen tab |
| `t` | open in new tab | `gC` `gJ` `gK` | clone / move tab |
| `:` | command line | `<A-1>`..`<A-9>` | focus tab N |
| `i` / `Esc` | insert / leave mode | `yy` / `yt` | yank url / title |
| `m` / `b` | save / load quickmark | `M` / `gb` | bookmark / load |
| `td` | toggle dark mode | `co` | close other tabs |

Counts work (e.g. `5j`). In command mode, `Tab`/`Shift-Tab` cycle completion.

## Commands

`:open`, `:tabopen`, `:back`, `:forward`, `:reload`, `:tab-close/next/prev/focus`,
`:tab-clone/move/only`, `:undo`, `:hint`, `:yank`, `:quickmark-save/load/del`,
`:bookmark-add/load/del`, `:set`, `:config-source`, `:darkmode`,
`:session-save/load`, `:plugin-reload`, `:quit`.

## Configuration

`~/.config/qbrsh/config.toml` (all fields optional):

```toml
homepage = "https://duckduckgo.com"

[colors]
background = "#1a1a2e"
foreground = "#e0e0e0"
accent = "#ffd76e"

[font]
family = "monospace"
size = 11
```

Change settings live with `:set colors.accent "#ff0000"` and reload the file
with `:config-source`.

## Extensibility

qbrsh does not run Firefox/Chrome extensions. Extensibility is native:

- **Ad blocking**: built-in domain blocking (extend via
  `~/.local/share/qbrsh/adblock`, one domain per line).
- **Plugins**: Rune scripts in `~/.local/share/qbrsh/plugins/*.rn`. See
  `examples/plugins/example.rn`. The `qbrsh` API: `command`, `open`, `message`,
  `eval_js`, and `on(event, handler)` for cold-event hooks (`page_load`,
  `tab_open`, `command`). Plugins are sandboxed and run under an instruction
  budget. Reload with `:plugin-reload`.
- **Automation**: drive the browser from an external process over the IPC
  control interface (intended escape hatch for heavy/untrusted automation).
