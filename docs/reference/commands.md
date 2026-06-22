# Commands

Commands are run from the command line (press `:`) or bound to keys. Many also
accept a count. The command line offers fuzzy completion: `Tab` and `Shift-Tab`
move the highlight, `Space` applies it, `Enter` runs it.

## Navigation

| Command | Description |
| --- | --- |
| `open <url>` | open a URL or search term in the current tab |
| `tabopen <url>` | open in a new tab |
| `private [url]` | open in a new private (ephemeral) tab |
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
| `js-enable` / `js-disable` / `js-toggle` | change JavaScript for the current site |
| `tabs-toggle` | collapse or expand the tab sidebar |
| `bind <keys> <command>` | bind a key sequence to a command |
| `unbind <keys>` | remove a key binding |
| `bindings` | list the active key bindings |
| `mode-enter <mode>` / `mode-leave` | switch input mode |

`open`/`tabopen` accept a search term or a URL. A bare term goes to the default
search engine; a leading engine keyword followed by a query selects that engine
(for example `tabopen gh ripgrep`). With no argument, `tabopen` opens the
configured new-tab page. See [Configuration](/guide/configuration) for search
engines, bindings, the new-tab page, the user-agent, and per-domain JavaScript.

## Plugins and lifecycle

| Command | Description |
| --- | --- |
| `plugin-reload` | recompile and reload plugins |
| `quit` | quit the browser |
