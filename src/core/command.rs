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
    /// Close the active tab.
    TabClose,
    /// Focus the next tab, wrapping, `count` times.
    TabNext(u32),
    /// Focus the previous tab, wrapping, `count` times.
    TabPrev(u32),
    /// Focus the tab at a 1-based index.
    TabSelect(usize),
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
            "tab-close" | "d" => Command::TabClose,
            "tab-next" => Command::TabNext(count(1)),
            "tab-prev" => Command::TabPrev(count(1)),
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
