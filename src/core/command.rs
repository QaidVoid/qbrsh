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
