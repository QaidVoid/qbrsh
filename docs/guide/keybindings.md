# Keybindings

qbrsh is modal. In Normal mode keys trigger commands; in Insert mode keys go to
the page; in Command mode you type a `:` command; in Hint mode you type a link
label. `Escape` always returns to Normal mode.

Counts work where they make sense, for example `5j` scrolls down five steps.

## Scrolling

| Key | Action |
| --- | --- |
| `h` `j` `k` `l` | scroll left, down, up, right |
| `gg` / `G` | top / bottom of page |
| `<C-f>` / `<C-b>` | page down / up |
| `<C-d>` / `<C-u>` | half page down / up |

## Navigation

| Key | Action |
| --- | --- |
| `H` / `L` | back / forward |
| `r` / `R` | reload / reload bypassing cache |
| `<C-c>` | stop loading the current page |
| `<F11>` | toggle window fullscreen |
| `gu` / `gU` | go up one path segment / to the host root |
| `]]` / `[[` | follow the next / previous page link |
| `<C-a>` / `<C-x>` | increment / decrement a number in the URL |
| `gi` | focus the first text input and enter insert mode |
| `o` | open a URL in the current tab |
| `O` | open, prefilled with the current URL |

## Hints

| Key | Action |
| --- | --- |
| `f` | follow a link in the current tab |
| `F` | open a link in a new tab |

Press the key, type the label shown on the target, and the unique match is
followed. `Escape` cancels.

## Tabs

| Key | Action |
| --- | --- |
| `t` | open a URL in a new tab |
| `,p` | open a URL in a new private tab |
| `J` / `K` | next / previous tab |
| `d` | close the tab |
| `u` | reopen the last closed tab |
| `gC` | clone the tab |
| `gJ` / `gK` | move the tab right / left |
| `co` | close all other tabs |
| `<A-1>` .. `<A-9>` | focus tab 1 to 9 |

## Yank, marks, content

| Key | Action |
| --- | --- |
| `yy` / `yt` | yank the URL / the title |
| `M` | bookmark the page |
| `m` / `b` | save / load a quickmark |
| `gb` | load a bookmark |
| `,d` | toggle dark mode |
| `,j` | toggle JavaScript for the current site |
| `,t` | collapse / expand the tab sidebar |

Toggles live under the `,` leader because `t` is itself bound (open in a new
tab) and so cannot also start a longer binding. Bindings are remappable: set them
in the `[bindings]` section of the config (see [Configuration](/guide/configuration))
or at runtime with `:bind`, `:unbind`, and `:bindings`.

## Modes and command line

| Key | Action |
| --- | --- |
| `i` | enter Insert mode |
| `<C-z>` | enter passthrough mode (every key goes to the page) |
| `Escape` | leave the current mode |
| `:` | open the command line |

In passthrough mode every key is delivered to the page and no binding fires, so
web apps that need keys like `j` or `/` receive them. The status bar shows
`-- PASS THROUGH --` while it is active. Press `Escape` to return to Normal mode.

In the command line, `Tab` and `Shift-Tab` move the highlight through the
completion list (your typed text stays put), `Space` applies the highlighted
item so you can continue with an argument, and `Enter` runs it. See the
[command reference](/reference/commands).
