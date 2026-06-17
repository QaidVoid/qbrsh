
## 0.1.0 — 2026-06-17

### Bug Fixes

- **completion:** Cycle list highlight without rewriting the command line

### Documentation

- Add VitePress documentation site
- Per-site permissions, subresource adblock, IPC, completion

### Features

- **ipc:** JSON-RPC control socket and URL forwarding
- **adblock:** Subresource blocking via WebKit content filter
- **permissions:** Per-site permission policy
- **plugin:** Awaitable eval_js via effect round-trip
- **engine:** Deny permission requests by default
- **cli:** Open a URL passed as an argument
- **plugin:** Rune runtime with cold hooks and budget
- **adblock:** Native domain blocking at navigation policy
- **content:** Dark mode toggle and session save/restore
- **config:** TOML config, :set, and themed chrome
- **completion:** History-backed URL completion
- **marks:** Bookmarks and quickmarks with completion
- **completion:** Fuzzy command-line completion popup
- **tabs:** Undo-close, clone, move, and tab-only
- **hints:** Keyboard link hinting via f and F
- **history:** Record visits on a SQLite writer thread
- **engine:** Error page and auto-insert-mode on focus
- **input:** GDK key controller with mode mirror
- **core:** Key bindings, trie, and command parsing
- GTK4 window with WebKitGTK engine integration
- **core:** Add update loop and runtime dispatch
- **core:** Add TEA state, message, and effect types

### Performance

- Bound browser memory and renderer processes

### Refactor

- **core:** Drive dispatch over async-channel


