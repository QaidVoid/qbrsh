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
| `/` | find in page | `n` / `N` | next / prev match |
| `zi` / `zo` | zoom in / out | `zz` | reset zoom |
| `<C-w>s`/`v` | split stacked / side by side | `<C-w>c` / `o` | close / only pane |
| `<C-w>w` / `W` | next / prev pane | | |

Counts work (e.g. `5j`). Type `/text` on the command line to search the page,
then `n` to step forward through matches (wrapping at the end). `N` steps
backward, but is best-effort: WebKitGTK's backward search cannot reach
hidden/collapsed content (so it can't step back into a closed accordion) and
corrupts the find controller's heap, which surfaces as a harmless
`free(): corrupted unsorted chunks` message when the browser exits. Prefer `n`
(it wraps to every match) if you want to avoid that. In command mode, `Tab`/`Shift-Tab`
move the highlight through the completion list (your typed text stays in the
command line), `Space` applies the highlighted item so you can continue with an
argument, and `Enter` runs the highlighted item (or the typed text if none is
selected).

## Commands

`:open`, `:tabopen`, `:back`, `:forward`, `:reload`, `:tab-close/next/prev/focus`,
`:tab-clone/move/only`, `:undo`, `:hint`, `:yank`, `:quickmark-save/load/del`,
`:bookmark-add/load/del`, `:find-next/prev`, `:zoom-in/out/reset`, `:zoom <pct>`,
`:set`, `:config-source`, `:darkmode`, `:session-save/load`, `:plugin-reload`,
`:permissions`, `:downloads`, `:history`, `:split`, `:vsplit`, `:close-pane`,
`:only-pane`, `:focus-pane`, `:focus-pane-prev`, `:quit`.

Panes show multiple tabs at once. `<C-w>s` (or `:split`) divides the focused
pane top/bottom; `<C-w>v` (or `:vsplit`) divides it side by side. Each split
opens a new tab in the new pane and focuses it. The focused pane holds the
active tab, so navigation and commands always target the focused pane. Selecting
a background tab (`J`/`K`, `<A-n>`, `:tab-focus`) swaps it into the focused pane;
selecting an already-visible tab focuses its pane. `<C-w>w`/`W` cycle focus,
`<C-w>c` closes the focused pane (its tab becomes a background tab), and
`<C-w>o` closes every pane except the focused one. Pane layout is not restored
across restarts (only tabs are).

Downloads are saved to your downloads directory (XDG `Downloads`, or
`~/.local/share/qbrsh/downloads` as a fallback) with a safe, non-colliding
filename. Start, progress, completion, and failure are reported in the status
bar. `:downloads` opens a management view (newest first): `j`/`k` move the
selection, `o` opens a finished file, `r` reveals it in its folder, `c` cancels
an active transfer, `R` retries a failed one, and `x` clears a
finished/failed/cancelled entry. `Esc` or `q` leaves the view.

`:history` opens a history list (newest first) with each visit's title, URL,
visit count, and time. `j`/`k` move the selection, `Enter` or `o` opens the
entry in the current tab, `t` opens it in a new tab, and `x` deletes it. Press
`/` to filter by URL or title (type to refine, `Backspace` edits, `Enter` or
`Esc` returns to the list); a second `Esc` or `q` leaves the view.

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

[zoom]
# Default page zoom for new tabs (1.0 = 100%).
default = 1.0

[session]
# Reopen the tabs that were open when qbrsh last quit (true by default).
restore = true

[permissions]
# Default policy for a capability with no rule: ask, allow, or deny.
# "ask" shows an interactive prompt (allow once / deny once / always / deny-always).
default = "ask"

[permissions.sites]
# Per-site overrides, matched by exact host or subdomain suffix. A bare policy
# applies to every capability; a table sets capabilities (geolocation,
# notifications, camera, microphone) independently.
"example.com" = "allow"
"maps.example.org" = { geolocation = "allow", camera = "deny" }
```

Permissions are decided per capability. When a capability's policy is `ask`, a
prompt appears: `y` allow once, `n` deny once, `a` always allow, `d` always
deny (Esc denies). "Always" choices and edits made in the management view
(`:permissions`, where `a`/`d`/`s` set and `x` revokes the selected rule)
persist to a data-dir store, separate from your hand-written `config.toml`.

Change settings live with `:set colors.accent "#ff0000"`,
`:set permissions.example.com allow` (all capabilities), or
`:set permissions.example.com.camera deny` (one capability), and reload the file
with `:config-source`.

## Extensibility

qbrsh does not run Firefox/Chrome extensions. Extensibility is native:

- **Ad blocking**: built-in domain blocking, applied both at navigation (frames,
  popups) and as a WebKit content filter for subresources (images, scripts,
  XHR). Extend via `~/.local/share/qbrsh/adblock`, one domain per line.
- **Plugins**: Rune scripts in `~/.local/share/qbrsh/plugins/*.rn`. See
  `examples/plugins/example.rn`. The `qbrsh` API: `command`, `open`, `message`,
  `eval_js(s).await` (suspends the plugin and returns the page result), and
  `on(event, handler)` for cold-event hooks (`page_load`, `tab_open`,
  `command`). The Rune sandbox blocks ambient host access (no filesystem,
  network, or process) and plugins run under an instruction budget. Note,
  though, that **plugins are trusted code**: `eval_js` reads the active page's
  DOM and `open`/`command` can navigate, which together suffice to exfiltrate
  data. Run untrusted automation over the IPC interface, not as a plugin. Reload
  with `:plugin-reload`.
- **Automation**: drive the browser from an external process over the IPC
  control interface, a JSON-RPC socket at `$XDG_RUNTIME_DIR/qbrsh/ipc.sock`.
  Send newline-delimited requests, for example:

  ```sh
  printf '{"method":"run_command","params":{"command":"tabopen https://x"}}\n' \
    | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/qbrsh/ipc.sock
  ```

  Launching `qbrsh <url>` while an instance is running forwards the URL to it.

## Resource use

To balance memory against isolation, tabs of the same site share a WebKit web
process, while different sites run in separate processes. A renderer crash
therefore affects only the same-site tabs sharing that process; each shows a
recoverable error page and can be reloaded with `r`. This uses more memory than
forcing every tab into one process, which is the intended trade: one site cannot
read another site's data. The in-memory back/forward page cache is disabled, so
navigating back or forward reloads the page (from the resource cache where
possible) rather than restoring it instantly. Run `:memory` to see current
resident memory and the number of live views.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
