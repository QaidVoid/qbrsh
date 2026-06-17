//! The single, owned application state.
//!
//! All mutable application state lives here as plain owned fields. There is no
//! `Rc<RefCell<_>>` sharing of state across subsystems; the dispatch loop holds
//! the sole `&mut State`. Subsystems are added to [`State`] as they are ported;
//! the skeleton establishes the ownership shape with the fields the core already
//! exercises (mode, tabs, input, command line, status, config).

use std::collections::HashMap;

use crate::core::msg::{JsPurpose, RequestId};

/// Input modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

/// Tracks the current mode and the one to return to on leave.
#[derive(Debug, Clone, Copy)]
pub struct ModeState {
    pub current: Mode,
    pub previous: Mode,
}

impl Default for ModeState {
    fn default() -> Self {
        Self {
            current: Mode::Normal,
            previous: Mode::Normal,
        }
    }
}

impl ModeState {
    /// Enter a new mode, remembering the current one as previous.
    pub fn enter(&mut self, mode: Mode) {
        if mode != self.current {
            self.previous = self.current;
            self.current = mode;
        }
    }

    /// Leave the current mode, returning to Normal.
    pub fn leave(&mut self) {
        self.previous = self.current;
        self.current = Mode::Normal;
    }
}

/// Stable identifier for a tab, shared between the state model and the engine's
/// web views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub u64);

/// A single tab's model. The actual web view is owned by the engine layer and
/// correlated to this model by [`TabId`].
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub progress: f64,
    pub crashed: bool,
}

impl Tab {
    fn new(id: TabId, url: &str) -> Self {
        Self {
            id,
            url: url.to_string(),
            title: String::new(),
            loading: false,
            progress: 0.0,
            crashed: false,
        }
    }
}

/// The ordered set of open tabs plus the active selection.
#[derive(Debug, Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl Tabs {
    /// Create a tab model for `url` and return its id. Does not change focus.
    pub fn open(&mut self, url: &str) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        self.tabs.push(Tab::new(id, url));
        id
    }

    /// Number of open tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether there are no open tabs. Pairs with [`Tabs::len`]; consumed by the
    /// UI layer.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The active tab, if any.
    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The id of the active tab, if any.
    pub fn active_id(&self) -> Option<TabId> {
        self.active().map(|t| t.id)
    }

    /// The zero-based index of the active tab. Consumed by the tab-bar renderer.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Mutable access to a tab by id.
    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Focus the last tab (used after opening a foreground tab).
    pub fn focus_last(&mut self) {
        if !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
    }

    /// Focus a tab by 1-based index. Returns the focused tab id, if valid.
    pub fn focus_index_1based(&mut self, index: usize) -> Option<TabId> {
        let idx = index.checked_sub(1)?;
        let tab = self.tabs.get(idx)?;
        self.active = idx;
        Some(tab.id)
    }

    /// Move focus forward `count` tabs, wrapping. Returns the new active id.
    pub fn next(&mut self, count: u32) -> Option<TabId> {
        if self.tabs.is_empty() {
            return None;
        }
        self.active = (self.active + count as usize) % self.tabs.len();
        self.active_id()
    }

    /// Move focus backward `count` tabs, wrapping. Returns the new active id.
    pub fn prev(&mut self, count: u32) -> Option<TabId> {
        if self.tabs.is_empty() {
            return None;
        }
        let len = self.tabs.len();
        let back = (count as usize) % len;
        self.active = (self.active + len - back) % len;
        self.active_id()
    }

    /// Close the active tab. Returns its id and the id to focus next, if any.
    pub fn close_active(&mut self) -> Option<(TabId, Option<TabId>)> {
        if self.tabs.is_empty() {
            return None;
        }
        let closed = self.tabs.remove(self.active);
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
        let next = self.active_id();
        Some((closed.id, next))
    }
}

/// The command-line input state.
#[derive(Debug, Default)]
pub struct CommandLine {
    pub text: String,
    pub active: bool,
}

/// Pending key buffer for the input layer. Filled once the binding trie is
/// ported; held here so all input state lives on the owned `State`.
#[derive(Debug, Default)]
pub struct InputState {
    pub pending: String,
    pub count: String,
}

/// Transient status-bar state not derived directly from the active tab.
#[derive(Debug, Default)]
pub struct StatusLine {
    pub scroll_percent: Option<u8>,
}

/// User configuration. Grows as the config subsystem is ported.
#[derive(Debug, Clone)]
pub struct Config {
    pub homepage: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            homepage: "about:blank".to_string(),
        }
    }
}

/// The complete application state.
#[derive(Debug, Default)]
pub struct State {
    pub mode: ModeState,
    pub tabs: Tabs,
    /// Filled once the binding trie is ported (input subsystem).
    pub input: InputState,
    pub command_line: CommandLine,
    pub status: StatusLine,
    pub config: Config,
    /// Purposes of in-flight JS evaluations, keyed by request id.
    pub pending_js: HashMap<RequestId, JsPurpose>,
    next_request_id: u64,
    /// Cleared to false to request shutdown.
    pub running: bool,
}

impl State {
    /// Create the initial state from configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            running: true,
            ..Self::default()
        }
    }

    /// Allocate a fresh request id for correlating an async result.
    pub fn alloc_request_id(&mut self) -> RequestId {
        let id = RequestId(self.next_request_id);
        self.next_request_id += 1;
        id
    }
}
