# Commands

Commands are run from the command line (press `:`) or bound to keys. Many also
accept a count. The command line offers fuzzy completion: `Tab` and `Shift-Tab`
move the highlight, `Space` applies it, `Enter` runs it.

## Navigation

| Command | Description |
| --- | --- |
| `open <url>` | open a URL or search term in the current tab |
| `tabopen <url>` | open in a new tab |
| `back` / `forward` | move through history |
| `reload` / `reload --force` | reload, optionally bypassing cache |
| `stop` | stop loading |

## Scrolling

| Command | Description |
| --- | --- |
| `scroll <up\|down\|left\|right>` | scroll in a direction |
| `scroll-page <up\|down> [half]` | scroll by a page or half page |
| `scroll-to-perc <n>` | scroll to a percentage of the page |

## Tabs

| Command | Description |
| --- | --- |
| `tab-close` | close the current tab |
| `tab-next` / `tab-prev` | cycle tabs |
| `tab-focus <n>` | focus tab by index |
| `tab-clone` | duplicate the current tab |
| `tab-move <offset>` | move the current tab |
| `tab-only` | close all other tabs |
| `undo` | reopen the last closed tab |

## Hints and yank

| Command | Description |
| --- | --- |
| `hint` / `hint-tab` | follow a link, or open it in a new tab |
| `yank [url\|title]` | copy the URL or title |

## Marks and sessions

| Command | Description |
| --- | --- |
| `quickmark-save <name>` | save the page as a named quickmark |
| `quickmark-load <name>` | open a quickmark |
| `quickmark-del <name>` | delete a quickmark |
| `bookmark-add` | bookmark the current page |
| `bookmark-load <url>` | open a bookmark |
| `bookmark-del <url>` | delete a bookmark |
| `session-save <name>` | save the open tabs as a session |
| `session-load <name>` | restore a session |

## Configuration and modes

| Command | Description |
| --- | --- |
| `set <key> <value>` | change a setting at runtime |
| `config-source` | reload the config file |
| `darkmode` | toggle web-content dark mode |
| `mode-enter <mode>` / `mode-leave` | switch input mode |

## Plugins and lifecycle

| Command | Description |
| --- | --- |
| `plugin-reload` | recompile and reload plugins |
| `quit` | quit the browser |
