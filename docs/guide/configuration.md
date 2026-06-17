# Configuration

qbrsh reads `~/.config/qbrsh/config.toml`. Every field is optional and falls
back to a default, so you only set what you want to change.

```toml
homepage = "https://duckduckgo.com"

[colors]
background = "#1a1a2e"
foreground = "#e0e0e0"
accent = "#ffd76e"

[font]
family = "monospace"
size = 11

[permissions]
default = "deny"

[permissions.sites]
"example.com" = "allow"
```

## Sections

### homepage

The page opened on startup when no URL is passed on the command line.

### colors and font

Theme the chrome (tab bar, status bar, command line, completion popup). Colors
are any CSS color string; the font applies to the chrome, not page content.

### permissions

A default policy plus per-site overrides for geolocation, notifications, and
media requests. See [Permissions](/guide/permissions).

## Changing settings at runtime

Use the `:set` command to change a value live:

```
:set colors.accent #ff5f5f
:set font.size 13
:set permissions.example.com allow
```

Reload the file from disk at any time with `:config-source`. There is no file
watcher; reloading is explicit and deterministic.
