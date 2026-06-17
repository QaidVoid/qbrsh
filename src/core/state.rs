//! The single, owned application state.
//!
//! All mutable application state lives here as plain owned fields. There is no
//! `Rc<RefCell<_>>` sharing of state across subsystems; the dispatch loop holds
//! the sole `&mut State`. Subsystems are added to [`State`] as they are ported;
//! the skeleton establishes the ownership shape with the fields the core already
//! exercises (mode, tabs, input, command line, status, config).

use std::collections::{BTreeMap, HashMap};

use crate::core::bindings::default_bindings;
use crate::core::command::HintTarget;
use crate::core::completion::CompletionState;
use crate::core::key::Key;
use crate::core::msg::{JsPurpose, RequestId};
use crate::core::trie::BindingTrie;

/// Input modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Hint,
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

/// A recently-closed tab, retained so it can be reopened with `undo`.
#[derive(Debug, Clone)]
pub struct ClosedTab {
    pub url: String,
    pub title: String,
}

/// Maximum number of closed tabs retained for undo.
const UNDO_LIMIT: usize = 100;

/// The ordered set of open tabs plus the active selection.
#[derive(Debug, Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    undo_stack: Vec<ClosedTab>,
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

    /// Close the active tab, retaining it for undo. Returns its id and the id to
    /// focus next, if any.
    pub fn close_active(&mut self) -> Option<(TabId, Option<TabId>)> {
        if self.tabs.is_empty() {
            return None;
        }
        let closed = self.tabs.remove(self.active);
        self.push_undo(&closed);
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
        let next = self.active_id();
        Some((closed.id, next))
    }

    /// Close all tabs except the active one, retaining them for undo. Returns the
    /// ids of the closed tabs.
    pub fn close_others(&mut self) -> Vec<TabId> {
        if self.tabs.len() < 2 {
            return Vec::new();
        }
        let kept = self.tabs.swap_remove(self.active);
        let removed = std::mem::take(&mut self.tabs);
        let closed_ids = removed.iter().map(|t| t.id).collect();
        for tab in &removed {
            self.push_undo(tab);
        }
        self.tabs = vec![kept];
        self.active = 0;
        closed_ids
    }

    /// Move the active tab by `delta` positions, clamped to the ends. Returns
    /// true if the order changed.
    pub fn move_active(&mut self, delta: i32) -> bool {
        let len = self.tabs.len();
        if len < 2 {
            return false;
        }
        let target = (self.active as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if target == self.active {
            return false;
        }
        let tab = self.tabs.remove(self.active);
        self.tabs.insert(target, tab);
        self.active = target;
        true
    }

    /// Pop the most recently closed tab for reopening.
    pub fn undo(&mut self) -> Option<ClosedTab> {
        self.undo_stack.pop()
    }

    fn push_undo(&mut self, tab: &Tab) {
        self.undo_stack.push(ClosedTab {
            url: tab.url.clone(),
            title: tab.title.clone(),
        });
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
    }
}

/// The command-line input state.
#[derive(Debug, Default)]
pub struct CommandLine {
    pub text: String,
    pub active: bool,
}

/// Pending key input: the partial key sequence and the count prefix.
#[derive(Debug, Default)]
pub struct InputState {
    pub pending: Vec<Key>,
    pub count: String,
}

/// Transient status-bar state not derived directly from the active tab.
#[derive(Debug, Default)]
pub struct StatusLine {
    pub scroll_percent: Option<u8>,
}

/// Active hint-mode state: the follow action, the available labels, and the
/// label characters typed so far.
#[derive(Debug, Default)]
pub struct HintState {
    pub target: HintTarget,
    pub labels: Vec<String>,
    pub input: String,
}

impl HintState {
    /// Labels that still match the typed prefix.
    pub fn matching(&self) -> impl Iterator<Item = &String> {
        self.labels.iter().filter(|l| l.starts_with(&self.input))
    }

    /// Reset to an empty, inactive hint state.
    pub fn reset(&mut self) {
        self.labels.clear();
        self.input.clear();
    }
}

/// A saved bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

/// Chrome colors (CSS color strings).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Colors {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: "#1a1a2e".to_string(),
            foreground: "#e0e0e0".to_string(),
            accent: "#ffd76e".to_string(),
        }
    }
}

/// Chrome font.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Font {
    pub family: String,
    pub size: u32,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: "monospace".to_string(),
            size: 11,
        }
    }
}

/// User configuration, deserialized from TOML and adjustable at runtime.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub homepage: String,
    pub colors: Colors,
    pub font: Font,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            homepage: "https://duckduckgo.com".to_string(),
            colors: Colors::default(),
            font: Font::default(),
        }
    }
}

impl Config {
    /// Set a configuration value by dotted key at runtime. Returns an error for
    /// unknown keys or invalid values.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "homepage" | "general.homepage" => self.homepage = value.to_string(),
            "colors.background" => self.colors.background = value.to_string(),
            "colors.foreground" => self.colors.foreground = value.to_string(),
            "colors.accent" => self.colors.accent = value.to_string(),
            "font.family" => self.font.family = value.to_string(),
            "font.size" => {
                self.font.size = value
                    .parse()
                    .map_err(|_| format!("invalid font.size: {value}"))?
            }
            _ => return Err(format!("unknown setting: {key}")),
        }
        Ok(())
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
    pub hints: HintState,
    pub completion: CompletionState,
    /// Named shortcuts to URLs (name → url).
    pub quickmarks: BTreeMap<String, String>,
    /// Saved bookmarks.
    pub bookmarks: Vec<Bookmark>,
    pub config: Config,
    /// Normal-mode key bindings.
    pub bindings: BindingTrie,
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
            bindings: default_bindings(),
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
