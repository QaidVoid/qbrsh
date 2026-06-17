# What is qbrsh?

qbrsh is a fast, keyboard-driven web browser written in Rust. It renders with
WebKitGTK 6 on GTK 4 and is navigated entirely from the keyboard, in the spirit
of qutebrowser, but built on a small, predictable core.

## Why it exists

Most browsers assume a mouse. qbrsh assumes your hands stay on the home row:
you follow links with letter hints, scroll with `hjkl`, manage tabs with single
keys, and reach everything else through a `:` command line with fuzzy
completion.

Under the hood it uses a hand-rolled Elm-style (TEA) architecture. All state
lives in one owned value, every input becomes a message, and a single consumer
applies those messages through a pure update function that returns effects.
There is no shared mutable state, no re-entrancy, and no polling. The result is
a browser that stays responsive and is straightforward to reason about.

## What you get

- **Hint mode** for following links, opening tabs, and yanking URLs by keyboard.
- **Modal input** with Normal, Insert, Command, and Hint modes.
- **Tabs** with open, close, undo-close, clone, move, and tab-only.
- **Fuzzy completion** in the command line, backed by history, bookmarks, and
  quickmarks.
- **Native ad blocking** at both the navigation and subresource level.
- **Per-site permissions** for geolocation, notifications, and media.
- **A sandboxed Rune plugin runtime** with cold-event hooks.
- **A JSON-RPC control socket** for scripting the browser from any process.

## What it is not

qbrsh does not run Firefox or Chrome extensions. Extensibility is native:
ad blocking, userscripts-style plugins, and external automation over IPC. See
[Plugins](/guide/plugins) and [Automation](/guide/automation).
