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
    /// Open in a new background tab.
    Background,
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
        let count = |default: u32| rest.first().and_then(|s| s.parse::<u32>().ok()).unwrap_or(default);

        let cmd = match name {
            "open" | "o" => Command::Open {
                target: OpenTarget::Current,
                input: arg,
            },
            "tabopen" | "t" => Command::Open {
                target: OpenTarget::Tab,
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
    ("quit", "Quit the browser"),
];
