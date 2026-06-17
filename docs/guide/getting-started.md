# Getting Started

## Requirements

qbrsh targets Linux with GTK 4 and WebKitGTK 6. You need the development
libraries for both, plus GStreamer plugins for media playback.

On Arch or Artix:

```sh
sudo pacman -S --needed gtk4 webkitgtk-6.0 gst-plugins-good gst-libav gst-plugins-bad
```

`gst-plugins-good` is important: without it WebKit's media pipeline can crash on
pages that use audio or video.

## Build and run

```sh
cargo run
```

Open a specific page at startup:

```sh
cargo run -- https://example.com
```

If an instance is already running, launching with a URL forwards it to that
instance instead of opening a second window. See [Automation](/guide/automation).

## First steps

When the window opens you are in Normal mode. A few things to try:

- Press `f` to light up link hints, then type a label to follow a link.
- Press `j` and `k` to scroll, `gg` and `G` for top and bottom.
- Press `o`, type a URL or search term, and press Enter.
- Press `:` to open the command line, then type a command such as `tabopen`.
- Press `J` and `K` to move between tabs, `d` to close one, `u` to reopen it.

When you click into a text field the browser switches to Insert mode
automatically; press `Escape` to return to Normal mode.

Next, learn the full [keybindings](/guide/keybindings) or set up your
[configuration](/guide/configuration).
