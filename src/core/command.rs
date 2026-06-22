//! The user-facing command vocabulary.
//!
//! Every browser action is a [`Command`]. Keybindings and the command line both
//! resolve to a `Command`, which enters the core as [`crate::core::msg::Msg::Command`].
//! This is the representable-as-a-message core the architecture is built around.

use crate::core::state::Mode;

/// Where an opened URL should be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    /// Load in the current tab.
    Current,
    /// Open in a new foreground tab.
    Tab,
    /// Open in a new foreground private (ephemeral-session) tab.
    PrivateTab,
}

/// Scroll direction for a `scroll` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

/// What `yank` copies to the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YankWhat {
    Url,
    Title,
}

/// How a JavaScript site-preference command changes the current site's rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsToggle {
    Enable,
    Disable,
    Toggle,
}

/// What website data a `:clear` command removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearScope {
    /// Every category, across all sites.
    All,
    /// Cookies (and HSTS state), across all sites.
    Cookies,
    /// Caches (memory, disk, DOM, offline app), across all sites.
    Cache,
    /// Local and session storage, across all sites.
    Storage,
    /// Every category, but only for the active tab's site.
    Site,
}

/// What following a hint does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HintTarget {
    /// Click the element in the current tab.
    #[default]
    Current,
    /// Open the element's link in a new tab.
    Tab,
}

/// A browser action.
///
/// This is the representative core set established by the TEA skeleton; the full
/// command registry and parser are ported with the input/command subsystems.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Open a URL or search term, normalized at execution time.
    Open { target: OpenTarget, input: String },
    /// Navigate back `count` entries.
    Back(u32),
    /// Navigate forward `count` entries.
    Forward(u32),
    /// Reload the active tab, optionally bypassing the cache.
    Reload { bypass_cache: bool },
    /// Stop loading the active tab.
    Stop,
    /// Scroll the active page in a direction `count` steps.
    Scroll(ScrollDir, u32),
    /// Scroll by a page (or half page) up or down.
    ScrollPage { down: bool, half: bool },
    /// Scroll to a vertical percentage of the active page.
    ScrollToPercent(u8),
    /// Enter hint mode with the given follow action.
    Hint(HintTarget),
    /// Close the active tab.
    TabClose,
    /// Focus the next tab, wrapping, `count` times.
    TabNext(u32),
    /// Focus the previous tab, wrapping, `count` times.
    TabPrev(u32),
    /// Focus the tab at a 1-based index.
    TabSelect(usize),
    /// Reopen the most recently closed tab.
    Undo,
    /// Duplicate the active tab.
    TabClone,
    /// Move the active tab by a signed offset.
    TabMove(i32),
    /// Close all tabs except the active one.
    TabOnly,
    /// Enter a specific input mode.
    ModeEnter(Mode),
    /// Leave the current mode, returning to Normal.
    ModeLeave,
    /// Enter command mode with the given prefilled text (e.g. ":" or ":open ").
    SetCommandLine(String),
    /// Execute the current command-line text.
    Accept,
    /// Copy a property of the active tab to the clipboard.
    Yank(YankWhat),
    /// Save the active page as a named quickmark.
    QuickmarkSave(String),
    /// Open the quickmark with the given name.
    QuickmarkLoad(String),
    /// Delete the quickmark with the given name.
    QuickmarkDel(String),
    /// Bookmark the active page.
    BookmarkAdd,
    /// Open the bookmark with the given URL.
    BookmarkLoad(String),
    /// Delete the bookmark with the given URL.
    BookmarkDel(String),
    /// Set a configuration value at runtime (`:set key value`).
    Set { key: String, value: String },
    /// Reload the configuration file.
    ConfigSource,
    /// Toggle web-content dark mode.
    DarkMode,
    /// Save the current tabs as a named session.
    SessionSave(String),
    /// Restore a named session's tabs.
    SessionLoad(String),
    /// Recompile and reload all plugins.
    PluginReload,
    /// Report current resident memory and live view count.
    Memory,
    /// Move to the next in-page search match.
    FindNext,
    /// Move to the previous in-page search match (best-effort; see `EngineView`).
    FindPrev,
    /// Increase the active tab's zoom.
    ZoomIn,
    /// Decrease the active tab's zoom.
    ZoomOut,
    /// Reset the active tab's zoom to the configured default.
    ZoomReset,
    /// Set the active tab's zoom to a percentage.
    ZoomSet(u32),
    /// Open the per-site permission management view.
    Permissions,
    /// Open the download management view.
    Downloads,
    /// Open the history management view, optionally pre-filtered.
    History(Option<String>),
    /// Split the focused pane into two stacked panes (top/bottom), opening a new
    /// tab in the new pane.
    Split,
    /// Split the focused pane into two side-by-side panes, opening a new tab in
    /// the new pane.
    Vsplit,
    /// Close the focused pane (its tab becomes a background tab).
    ClosePane,
    /// Close every pane except the focused one.
    OnlyPane,
    /// Cycle focus to the next (`next`) or previous pane.
    FocusPane { next: bool },
    /// Change the current site's JavaScript preference.
    SiteJavascript(JsToggle),
    /// Toggle the tab sidebar between expanded and collapsed.
    TabsToggle,
    /// Clear website data of the given scope.
    ClearData(ClearScope),
    /// Bind a key sequence to a command for the running session.
    Bind { keys: String, command: String },
    /// Remove the binding for a key sequence.
    Unbind(String),
    /// List the active key bindings.
    Bindings,
    /// Quit the browser.
    Quit,
    /// Do nothing (used to disable a default binding).
    Nop,
}

impl Command {
    /// Parse a bare command string (no leading `:`) into a [`Command`].
    ///
    /// This covers the currently-implemented command set; the registry expands as
    /// subsystems land. Returns an error message for unknown commands.
    pub fn parse(input: &str) -> Result<Command, String> {
        // `cmd-set-text` takes the rest of the line verbatim (spaces preserved).
        if let Some(text) = input.strip_prefix("cmd-set-text ") {
            return Ok(Command::SetCommandLine(text.to_string()));
        }
        if input.trim() == "cmd-set-text" {
            return Ok(Command::SetCommandLine(String::new()));
        }

        let mut parts = input.split_whitespace();
        let Some(name) = parts.next() else {
            return Err("empty command".to_string());
        };
        let rest: Vec<&str> = parts.collect();
        let arg = rest.join(" ");
        let count = |default: u32| {
            rest.first()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(default)
        };

        let cmd = match name {
            "open" | "o" => Command::Open {
                target: OpenTarget::Current,
                input: arg,
            },
            "tabopen" | "t" => Command::Open {
                target: OpenTarget::Tab,
                input: arg,
            },
            "private" | "private-open" => Command::Open {
                target: OpenTarget::PrivateTab,
                input: arg,
            },
            "back" => Command::Back(count(1)),
            "forward" => Command::Forward(count(1)),
            "reload" | "r" => Command::Reload {
                bypass_cache: rest.contains(&"--force"),
            },
            "stop" => Command::Stop,
            "scroll" => Command::Scroll(parse_dir(rest.first())?, 1),
            "scroll-page" => Command::ScrollPage {
                down: matches!(rest.first(), Some(&"down")),
                half: rest.contains(&"half"),
            },
            "scroll-to-perc" => Command::ScrollToPercent(count(100).min(100) as u8),
            "hint" => Command::Hint(HintTarget::Current),
            "hint-tab" => Command::Hint(HintTarget::Tab),
            "tab-close" | "d" => Command::TabClose,
            "tab-next" => Command::TabNext(count(1)),
            "tab-prev" => Command::TabPrev(count(1)),
            "tab-clone" => Command::TabClone,
            "tab-only" => Command::TabOnly,
            "tab-move" => Command::TabMove(
                rest.first()
                    .and_then(|s| s.parse::<i32>().ok())
                    .ok_or_else(|| format!("tab-move needs an offset: {arg}"))?,
            ),
            "undo" => Command::Undo,
            "tab-focus" | "tab-select" => {
                let n = rest
                    .first()
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| format!("tab-focus needs an index: {arg}"))?;
                Command::TabSelect(n)
            }
            "mode-enter" => Command::ModeEnter(parse_mode(rest.first())?),
            "mode-leave" => Command::ModeLeave,
            "yank" => Command::Yank(match rest.first() {
                None | Some(&"url") => YankWhat::Url,
                Some(&"title") => YankWhat::Title,
                Some(other) => return Err(format!("unknown yank target: {other}")),
            }),
            "quickmark-save" => Command::QuickmarkSave(arg),
            "quickmark-load" => Command::QuickmarkLoad(arg),
            "quickmark-del" => Command::QuickmarkDel(arg),
            "bookmark-add" => Command::BookmarkAdd,
            "bookmark-load" => Command::BookmarkLoad(arg),
            "bookmark-del" => Command::BookmarkDel(arg),
            "set" => {
                let key = rest
                    .first()
                    .ok_or_else(|| "set needs a key".to_string())?
                    .to_string();
                let value = rest[1..].join(" ");
                Command::Set { key, value }
            }
            "config-source" => Command::ConfigSource,
            "darkmode" => Command::DarkMode,
            "session-save" => Command::SessionSave(arg),
            "session-load" => Command::SessionLoad(arg),
            "plugin-reload" => Command::PluginReload,
            "memory" => Command::Memory,
            "find-next" | "search-next" => Command::FindNext,
            "find-prev" | "search-prev" => Command::FindPrev,
            "zoom-in" => Command::ZoomIn,
            "zoom-out" => Command::ZoomOut,
            "zoom-reset" => Command::ZoomReset,
            "zoom" => Command::ZoomSet(
                rest.first()
                    .and_then(|s| s.trim_end_matches('%').parse::<u32>().ok())
                    .ok_or_else(|| format!("zoom needs a percentage: {arg}"))?,
            ),
            "permissions" => Command::Permissions,
            "downloads" => Command::Downloads,
            "history" => Command::History(if arg.is_empty() { None } else { Some(arg) }),
            "split" | "sp" => Command::Split,
            "vsplit" | "vs" => Command::Vsplit,
            "close-pane" => Command::ClosePane,
            "only-pane" => Command::OnlyPane,
            "focus-pane" => Command::FocusPane { next: true },
            "focus-pane-prev" => Command::FocusPane { next: false },
            "js-enable" => Command::SiteJavascript(JsToggle::Enable),
            "js-disable" => Command::SiteJavascript(JsToggle::Disable),
            "js-toggle" => Command::SiteJavascript(JsToggle::Toggle),
            "tabs-toggle" => Command::TabsToggle,
            "clear" => Command::ClearData(match rest.first() {
                None | Some(&"all") => ClearScope::All,
                Some(&"cookies") => ClearScope::Cookies,
                Some(&"cache") => ClearScope::Cache,
                Some(&"storage") => ClearScope::Storage,
                Some(&"site") => ClearScope::Site,
                Some(other) => return Err(format!("unknown clear scope: {other}")),
            }),
            "bind" => {
                let keys = rest
                    .first()
                    .ok_or_else(|| "bind needs a key sequence".to_string())?
                    .to_string();
                let command = rest[1..].join(" ");
                if command.is_empty() {
                    return Err("bind needs a command".to_string());
                }
                Command::Bind { keys, command }
            }
            "unbind" => {
                if arg.is_empty() {
                    return Err("unbind needs a key sequence".to_string());
                }
                Command::Unbind(arg)
            }
            "bindings" => Command::Bindings,
            "quit" | "q" | "qa" => Command::Quit,
            "nop" => Command::Nop,
            other => return Err(format!("unknown command: {other}")),
        };
        Ok(cmd)
    }

    /// Apply a count prefix to the commands that honor one.
    pub fn with_count(self, count: u32) -> Command {
        match self {
            Command::Scroll(dir, _) => Command::Scroll(dir, count),
            Command::Back(_) => Command::Back(count),
            Command::Forward(_) => Command::Forward(count),
            Command::TabNext(_) => Command::TabNext(count),
            Command::TabPrev(_) => Command::TabPrev(count),
            other => other,
        }
    }
}

/// Whether `input` from an untrusted source (a page hint, IPC, or a plugin) is
/// safe to navigate to. Bare hosts and search terms are safe because they
/// normalize to `https`; only an explicit non-web scheme (such as `file:`,
/// `data:`, or `javascript:`) is rejected. `about:blank` is allowed.
pub fn is_safe_external_target(input: &str) -> bool {
    let t = input.trim();
    if t.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    match scheme_of(t) {
        Some(scheme) => matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https"),
        None => true,
    }
}

/// Whether `input` uses a scheme that must never be opened as a new tab from
/// page content (`window.open`, `target=_blank`), because the scheme executes
/// script (`javascript:`) or renders arbitrary inline content (`data:`). Other
/// schemes are allowed, including `file:`: WebKit already gates cross-origin
/// access to local files at the engine level, so blocking `file:` here would
/// only break same-context local browsing.
pub fn is_unsafe_open_target(input: &str) -> bool {
    match scheme_of(input.trim()) {
        Some(s) => matches!(s.to_ascii_lowercase().as_str(), "data" | "javascript"),
        None => false,
    }
}

/// Whether a parsed command may be invoked by an untrusted remote (IPC) client.
/// Restricts the remote surface to navigation, scrolling, and tab commands, and
/// validates the target of any open. Sensitive commands (config mutation, quit,
/// plugin reload, session/file writes) are not remotely invokable.
pub fn is_remote_safe(cmd: &Command) -> bool {
    match cmd {
        Command::Open { input, .. } => is_safe_external_target(input),
        Command::Back(_)
        | Command::Forward(_)
        | Command::Reload { .. }
        | Command::Stop
        | Command::Scroll(..)
        | Command::ScrollPage { .. }
        | Command::ScrollToPercent(_)
        | Command::TabClose
        | Command::TabNext(_)
        | Command::TabPrev(_)
        | Command::TabSelect(_)
        | Command::TabClone
        | Command::TabMove(_)
        | Command::TabOnly
        | Command::Undo => true,
        _ => false,
    }
}

/// Extract an explicit URL scheme if `input` clearly has one, distinguishing a
/// scheme (`file:`, `http://`, `about:`) from a bare `host:port` by requiring the
/// pre-colon token to be followed by `//` or to contain no dot.
fn scheme_of(input: &str) -> Option<String> {
    let colon = input.find(':')?;
    let scheme = &input[..colon];
    if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    {
        return None;
    }
    let after = &input[colon + 1..];
    if after.starts_with("//") || !scheme.contains('.') {
        Some(scheme.to_string())
    } else {
        None
    }
}

fn parse_dir(arg: Option<&&str>) -> Result<ScrollDir, String> {
    match arg {
        Some(&"up") => Ok(ScrollDir::Up),
        Some(&"down") => Ok(ScrollDir::Down),
        Some(&"left") => Ok(ScrollDir::Left),
        Some(&"right") => Ok(ScrollDir::Right),
        other => Err(format!("invalid scroll direction: {other:?}")),
    }
}

fn parse_mode(arg: Option<&&str>) -> Result<Mode, String> {
    match arg {
        Some(&"normal") => Ok(Mode::Normal),
        Some(&"insert") => Ok(Mode::Insert),
        Some(&"command") => Ok(Mode::Command),
        other => Err(format!("unknown mode: {other:?}")),
    }
}

/// Catalog of command names and one-line descriptions, used for completion.
/// New commands are added here as they become available.
pub const COMMAND_CATALOG: &[(&str, &str)] = &[
    ("open", "Open a URL or search in the current tab"),
    ("tabopen", "Open a URL or search in a new tab"),
    ("private", "Open a URL or search in a new private tab"),
    ("back", "Go back in history"),
    ("forward", "Go forward in history"),
    ("reload", "Reload the page"),
    ("stop", "Stop loading"),
    ("scroll", "Scroll in a direction"),
    ("scroll-page", "Scroll by a page"),
    ("scroll-to-perc", "Scroll to a percentage of the page"),
    ("hint", "Follow a link by keyboard"),
    ("hint-tab", "Open a hinted link in a new tab"),
    ("tab-close", "Close the current tab"),
    ("tab-next", "Focus the next tab"),
    ("tab-prev", "Focus the previous tab"),
    ("tab-focus", "Focus a tab by index"),
    ("tab-clone", "Duplicate the current tab"),
    ("tab-move", "Move the current tab"),
    ("tab-only", "Close all other tabs"),
    ("undo", "Reopen the last closed tab"),
    ("mode-enter", "Enter an input mode"),
    ("mode-leave", "Return to normal mode"),
    ("yank", "Copy the page URL or title"),
    ("quickmark-save", "Save the page as a named quickmark"),
    ("quickmark-load", "Open a quickmark by name"),
    ("quickmark-del", "Delete a quickmark"),
    ("bookmark-add", "Bookmark the current page"),
    ("bookmark-load", "Open a bookmark"),
    ("bookmark-del", "Delete a bookmark"),
    ("set", "Set a configuration value"),
    ("config-source", "Reload the configuration file"),
    ("darkmode", "Toggle web-content dark mode"),
    ("session-save", "Save the current tabs as a session"),
    ("session-load", "Restore a saved session"),
    ("plugin-reload", "Recompile and reload plugins"),
    ("memory", "Report memory use and view count"),
    ("find-next", "Jump to the next search match"),
    (
        "find-prev",
        "Jump to the previous search match (best-effort)",
    ),
    ("zoom-in", "Increase page zoom"),
    ("zoom-out", "Decrease page zoom"),
    ("zoom-reset", "Reset page zoom to the default"),
    ("zoom", "Set page zoom to a percentage"),
    ("permissions", "Manage per-site permissions"),
    ("downloads", "Manage downloads"),
    ("history", "Browse and search history"),
    ("split", "Split the focused pane top/bottom"),
    ("vsplit", "Split the focused pane side by side"),
    ("close-pane", "Close the focused pane (keep its tab)"),
    ("only-pane", "Close all panes except the focused one"),
    ("focus-pane", "Cycle focus to the next pane"),
    ("focus-pane-prev", "Cycle focus to the previous pane"),
    ("js-enable", "Enable JavaScript for the current site"),
    ("js-disable", "Disable JavaScript for the current site"),
    ("js-toggle", "Toggle JavaScript for the current site"),
    ("tabs-toggle", "Collapse or expand the tab sidebar"),
    (
        "clear",
        "Clear website data (all/cookies/cache/storage/site)",
    ),
    ("bind", "Bind a key sequence to a command"),
    ("unbind", "Remove a key binding"),
    ("bindings", "List the active key bindings"),
    ("quit", "Quit the browser"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_external_targets_allow_web_and_bare_hosts() {
        assert!(is_safe_external_target("https://example.com"));
        assert!(is_safe_external_target("http://example.com/p"));
        assert!(is_safe_external_target("example.com"));
        assert!(is_safe_external_target("example.com:8080/path"));
        assert!(is_safe_external_target("search terms here"));
        assert!(is_safe_external_target("about:blank"));
    }

    #[test]
    fn safe_external_targets_reject_dangerous_schemes() {
        assert!(!is_safe_external_target("file:///etc/passwd"));
        assert!(!is_safe_external_target("file:///home/me/.ssh/id_rsa"));
        assert!(!is_safe_external_target(
            "data:text/html,<script>1</script>"
        ));
        assert!(!is_safe_external_target("javascript:alert(1)"));
        assert!(!is_safe_external_target("about:config"));
    }

    #[test]
    fn unsafe_open_target_blocks_only_data_and_javascript() {
        assert!(is_unsafe_open_target("data:text/html,<h1>hi</h1>"));
        assert!(is_unsafe_open_target("javascript:alert(1)"));
        // Scheme match is case-insensitive, surrounding whitespace trimmed.
        assert!(is_unsafe_open_target("JavaScript:alert(1)"));
        assert!(is_unsafe_open_target("  data:text/plain,x  "));
    }

    #[test]
    fn remote_safe_allows_navigation_not_sensitive_commands() {
        assert!(is_remote_safe(&Command::Open {
            target: OpenTarget::Tab,
            input: "https://a.test".to_string(),
        }));
        assert!(is_remote_safe(&Command::TabNext(1)));
        assert!(!is_remote_safe(&Command::Open {
            target: OpenTarget::Tab,
            input: "file:///etc/passwd".to_string(),
        }));
        assert!(!is_remote_safe(&Command::Quit));
        assert!(!is_remote_safe(&Command::PluginReload));
        assert!(!is_remote_safe(&Command::Set {
            key: "x".to_string(),
            value: "y".to_string(),
        }));
        assert!(!is_remote_safe(&Command::SessionSave("s".to_string())));
    }

    #[test]
    fn clear_parses_scopes_and_rejects_unknown() {
        assert!(matches!(
            Command::parse("clear").unwrap(),
            Command::ClearData(ClearScope::All)
        ));
        assert!(matches!(
            Command::parse("clear cookies").unwrap(),
            Command::ClearData(ClearScope::Cookies)
        ));
        assert!(matches!(
            Command::parse("clear site").unwrap(),
            Command::ClearData(ClearScope::Site)
        ));
        assert!(Command::parse("clear bogus").is_err());
    }

    #[test]
    fn pane_commands_parse() {
        assert!(matches!(Command::parse("split").unwrap(), Command::Split));
        assert!(matches!(Command::parse("sp").unwrap(), Command::Split));
        assert!(matches!(Command::parse("vsplit").unwrap(), Command::Vsplit));
        assert!(matches!(Command::parse("vs").unwrap(), Command::Vsplit));
        assert!(matches!(
            Command::parse("close-pane").unwrap(),
            Command::ClosePane
        ));
        assert!(matches!(
            Command::parse("only-pane").unwrap(),
            Command::OnlyPane
        ));
        assert!(matches!(
            Command::parse("focus-pane").unwrap(),
            Command::FocusPane { next: true }
        ));
        assert!(matches!(
            Command::parse("focus-pane-prev").unwrap(),
            Command::FocusPane { next: false }
        ));
    }
}
