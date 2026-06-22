# Configuration

qbrsh reads `~/.config/qbrsh/config.toml`. Every field is optional and falls
back to a default, so you only set what you want to change.

```toml
homepage = "https://duckduckgo.com"
newtab = "about:blank"
useragent = ""

[colors]
background = "#1a1a2e"
foreground = "#e0e0e0"
accent = "#ffd76e"

[font]
family = "monospace"
size = 11

[search]
default = "ddg"

[search.engines]
ddg = "https://duckduckgo.com/?q={}"
g = "https://www.google.com/search?q={}"
gh = "https://github.com/search?q={}"
w = "https://en.wikipedia.org/w/index.php?search={}"

[bindings]
j = "scroll down"
"<C-j>" = "tab-next"

[permissions]
default = "deny"

[permissions.sites]
"example.com" = "allow"
```

## Sections

### homepage and newtab

`homepage` is the page opened on startup when no URL is passed on the command
line. `newtab` is the page a blank new tab opens (for example `:tabopen` with no
argument); it defaults to `about:blank` and is independent of `homepage`.

### useragent

Override the browser's user-agent string. Leave it empty to keep the engine
default.

### search

Named search engines plus the keyword of the default. Each engine is a URL
template with a `{}` placeholder that the query is substituted into. Bare
command-line input that is not a URL goes to the default engine; a leading token
that names an engine and is followed by a query selects that engine, so
`gh rust serde` searches GitHub. If the configured `default` names no defined
engine, a built-in DuckDuckGo engine is used.

### bindings

Remap Normal-mode keys. Each entry maps a key string (the same syntax used in
the keybinding table, such as `gg` or `<C-f>`) to a command. Entries are layered
over the built-in defaults, so an entry whose key matches a default replaces it
while every other default stays in place. A key string that conflicts with an
existing multi-key binding (for example `g` while `gg` is bound) is reported and
skipped. See also the runtime `:bind`, `:unbind`, and `:bindings` commands.

### colors and font

Theme the chrome (tab bar, status bar, command line, completion popup). Colors
are any CSS color string; the font applies to the chrome, not page content.

### permissions

A default policy plus per-site overrides for geolocation, notifications, and
media requests. See [Permissions](/guide/permissions).

### per-domain JavaScript

JavaScript runs everywhere by default. Disable or enable it for the current site
at runtime with `:js-disable`, `:js-enable`, or `:js-toggle` (bound to `tj`),
which reloads the site's tabs. Rules are saved separately from `config.toml` and
restored on the next launch. Set them directly with `:set javascript.<host>
true|false` and change the global default with `:set javascript.default
true|false`.

## Changing settings at runtime

Use the `:set` command to change a value live:

```
:set colors.accent #ff5f5f
:set font.size 13
:set permissions.example.com allow
:set search.default g
:set search.engines.gh https://github.com/search?q={}
:set javascript.example.com false
```

Reload the file from disk at any time with `:config-source`. There is no file
watcher; reloading is explicit and deterministic.
