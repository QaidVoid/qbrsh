//! The single, owned application state.
//!
//! All mutable application state lives here as plain owned fields. There is no
//! `Rc<RefCell<_>>` sharing of state across subsystems; the dispatch loop holds
//! the sole `&mut State`. Subsystems are added to [`State`] as they are ported;
//! the skeleton establishes the ownership shape with the fields the core already
//! exercises (mode, tabs, input, command line, status, config).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

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
    /// Answering an interactive permission prompt.
    Prompt,
    /// Browsing the permission management list.
    Permissions,
    /// Browsing the download management list.
    Downloads,
    /// Browsing the history management list.
    History,
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
    /// Page zoom level (1.0 = 100%).
    pub zoom: f64,
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
            zoom: 1.0,
        }
    }
}

/// A recently-closed tab, retained so it can be reopened with `undo`.
#[derive(Debug, Clone)]
pub struct ClosedTab {
    pub url: String,
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

    /// Borrow the tab with the given id, if present.
    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
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

    /// The URLs of all open tabs, in order.
    pub fn urls(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.url.clone()).collect()
    }

    fn push_undo(&mut self, tab: &Tab) {
        self.undo_stack.push(ClosedTab {
            url: tab.url.clone(),
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
    /// Current in-page search position, shown until cleared.
    pub search: Option<SearchStatus>,
}

/// In-page search result for the status line: the total match count, or `None`
/// before the count arrives. WebKit's native find does not expose a reliable
/// current-match index, so only the total is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchStatus {
    pub total: Option<usize>,
}

impl SearchStatus {
    /// Format as `N matches`, `searching` before the count, or `no matches`.
    pub fn label(&self) -> String {
        match self.total {
            None => "searching".to_string(),
            Some(0) => "no matches".to_string(),
            Some(1) => "1 match".to_string(),
            Some(n) => format!("{n} matches"),
        }
    }
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

/// Page zoom configuration.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Zoom {
    /// Default zoom level applied to new tabs (1.0 = 100%).
    pub default: f64,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { default: 1.0 }
    }
}

/// Session restore configuration.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Session {
    /// Whether to reopen the autosaved tabs on startup.
    pub restore: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self { restore: true }
    }
}

/// How a site permission request is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionPolicy {
    /// Prompt the user with the interactive permission prompt.
    #[default]
    Ask,
    Allow,
    Deny,
}

impl PermissionPolicy {
    /// Parse a policy from a `:set` value.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ask" => Ok(Self::Ask),
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => Err(format!("invalid permission policy: {other}")),
        }
    }
}

/// A capability a page can request, decided independently per site.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Geolocation,
    Notifications,
    Camera,
    Microphone,
}

impl Capability {
    /// Every capability, in display order.
    pub const ALL: [Capability; 4] = [
        Capability::Geolocation,
        Capability::Notifications,
        Capability::Camera,
        Capability::Microphone,
    ];

    /// The lowercase name used in config keys and display.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Geolocation => "geolocation",
            Capability::Notifications => "notifications",
            Capability::Camera => "camera",
            Capability::Microphone => "microphone",
        }
    }

    /// Parse a capability name from a config key.
    pub fn parse(value: &str) -> Option<Self> {
        Capability::ALL.into_iter().find(|c| c.as_str() == value)
    }
}

/// A site's permission rules: either one policy for all capabilities (the
/// backward-compatible bare form) or an explicit per-capability map.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum SiteRules {
    /// One policy applied to every capability (legacy bare-string form).
    All(PermissionPolicy),
    /// An explicit policy per capability.
    PerCapability(BTreeMap<Capability, PermissionPolicy>),
}

impl SiteRules {
    /// The policy this site defines for `cap`, if any.
    fn get(&self, cap: Capability) -> Option<PermissionPolicy> {
        match self {
            SiteRules::All(p) => Some(*p),
            SiteRules::PerCapability(m) => m.get(&cap).copied(),
        }
    }

    /// Set `cap` to `policy`, expanding a bare `All` rule into a per-capability
    /// map first so other capabilities keep their effective policy.
    fn set(&mut self, cap: Capability, policy: PermissionPolicy) {
        if let SiteRules::All(p) = *self {
            let mut m: BTreeMap<Capability, PermissionPolicy> =
                Capability::ALL.into_iter().map(|c| (c, p)).collect();
            m.insert(cap, policy);
            *self = SiteRules::PerCapability(m);
        } else if let SiteRules::PerCapability(m) = self {
            m.insert(cap, policy);
        }
    }
}

/// Per-site permission policy: a default plus host-suffix-keyed, per-capability
/// overrides.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub default: PermissionPolicy,
    pub sites: BTreeMap<String, SiteRules>,
}

impl Permissions {
    /// Resolve the policy for `host` and `cap`, matching a site rule by exact
    /// host or subdomain suffix, else the default.
    pub fn policy_for(&self, host: &str, cap: Capability) -> PermissionPolicy {
        for (site, rules) in &self.sites {
            if (host == site.as_str() || host.ends_with(&format!(".{site}")))
                && let Some(p) = rules.get(cap)
            {
                return p;
            }
        }
        self.default
    }

    /// Set the policy for a single capability on `host`.
    pub fn set_capability(&mut self, host: &str, cap: Capability, policy: PermissionPolicy) {
        self.sites
            .entry(host.to_string())
            .or_insert_with(|| SiteRules::PerCapability(BTreeMap::new()))
            .set(cap, policy);
    }

    /// Set one policy for every capability on `host`.
    pub fn set_all(&mut self, host: &str, policy: PermissionPolicy) {
        self.sites.insert(host.to_string(), SiteRules::All(policy));
    }

    /// Flatten the rules into display rows: one row per `All` site, one per
    /// capability for per-capability sites, sorted by host then capability.
    pub fn rows(&self) -> Vec<PermissionRow> {
        let mut rows = Vec::new();
        for (host, rules) in &self.sites {
            match rules {
                SiteRules::All(p) => rows.push(PermissionRow {
                    host: host.clone(),
                    capability: None,
                    policy: *p,
                }),
                SiteRules::PerCapability(m) => {
                    for (cap, p) in m {
                        rows.push(PermissionRow {
                            host: host.clone(),
                            capability: Some(*cap),
                            policy: *p,
                        });
                    }
                }
            }
        }
        rows
    }

    /// Apply a row's policy (used when cycling in the management view).
    pub fn set_row(&mut self, row: &PermissionRow, policy: PermissionPolicy) {
        match row.capability {
            Some(cap) => self.set_capability(&row.host, cap, policy),
            None => self.set_all(&row.host, policy),
        }
    }

    /// Remove a row's rule, reverting it to the default.
    pub fn revoke_row(&mut self, row: &PermissionRow) {
        match row.capability {
            None => {
                self.sites.remove(&row.host);
            }
            Some(cap) => {
                if let Some(SiteRules::PerCapability(m)) = self.sites.get_mut(&row.host) {
                    m.remove(&cap);
                    if m.is_empty() {
                        self.sites.remove(&row.host);
                    }
                }
            }
        }
    }
}

/// A single flattened permission rule for the management view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRow {
    pub host: String,
    pub capability: Option<Capability>,
    pub policy: PermissionPolicy,
}

/// A deferred permission request awaiting the user's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPrompt {
    pub id: u64,
    pub host: String,
    pub capability: Capability,
}

/// State of the permission management list view.
#[derive(Debug, Default)]
pub struct PermissionViewState {
    pub rows: Vec<PermissionRow>,
    pub selected: usize,
}

/// Lifecycle stage of a tracked download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    /// Transfer in progress.
    Active,
    /// Saved to disk successfully.
    Finished,
    /// Transfer aborted with an error.
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl DownloadStatus {
    /// The lowercase status label shown in the management view.
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadStatus::Active => "active",
            DownloadStatus::Finished => "finished",
            DownloadStatus::Failed => "failed",
            DownloadStatus::Cancelled => "cancelled",
        }
    }
}

/// A tracked download and its in-progress accounting.
#[derive(Debug, Clone)]
pub struct Download {
    pub filename: String,
    pub path: PathBuf,
    /// Bytes received so far.
    pub received: u64,
    /// Total expected bytes, or `0` when unknown.
    pub total: u64,
    pub status: DownloadStatus,
    /// The original source URL, kept for retry.
    pub source: String,
}

impl Download {
    /// Progress as a fraction in `0.0..=1.0`, or `None` when the total is unknown.
    pub fn fraction(&self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some((self.received as f64 / self.total as f64).clamp(0.0, 1.0))
        }
    }
}

/// State of the download management list view.
#[derive(Debug, Default)]
pub struct DownloadViewState {
    pub selected: usize,
}

/// A history entry shown in the history management view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Unix seconds of the most recent visit.
    pub last_visit: i64,
}

/// State of the history management list view: the rendered rows, the selection,
/// the active filter, whether the filter is being edited, and the generation
/// used to discard stale query results.
#[derive(Debug, Default)]
pub struct HistoryViewState {
    pub rows: Vec<HistoryRow>,
    pub selected: usize,
    pub filter: String,
    pub filter_edit: bool,
    pub generation: u64,
}

/// User configuration, deserialized from TOML and adjustable at runtime.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub homepage: String,
    pub colors: Colors,
    pub font: Font,
    pub zoom: Zoom,
    pub session: Session,
    pub permissions: Permissions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            homepage: "https://duckduckgo.com".to_string(),
            colors: Colors::default(),
            font: Font::default(),
            zoom: Zoom::default(),
            session: Session::default(),
            permissions: Permissions::default(),
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
            "zoom.default" => {
                self.zoom.default = value
                    .parse()
                    .map_err(|_| format!("invalid zoom.default: {value}"))?
            }
            "session.restore" => {
                self.session.restore = value
                    .parse()
                    .map_err(|_| format!("invalid session.restore: {value}"))?
            }
            "permissions.default" => self.permissions.default = PermissionPolicy::parse(value)?,
            key if key.starts_with("permissions.") => {
                let rest = &key["permissions.".len()..];
                let policy = PermissionPolicy::parse(value)?;
                // A trailing capability segment sets that capability; otherwise
                // the key is a bare host and sets every capability.
                match rest
                    .rsplit_once('.')
                    .and_then(|(host, last)| Capability::parse(last).map(|cap| (host, cap)))
                {
                    Some((host, cap)) => self.permissions.set_capability(host, cap, policy),
                    None => self.permissions.set_all(rest, policy),
                }
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
    /// Whether web-content dark mode is active.
    pub dark_mode: bool,
    /// Pending permission prompts; the front item is the one being shown.
    pub prompts: VecDeque<PermissionPrompt>,
    /// State of the permission management list view.
    pub perm_view: PermissionViewState,
    /// State of the download management list view.
    pub dl_view: DownloadViewState,
    /// State of the history management list view.
    pub history_view: HistoryViewState,
    /// The last in-page search, for `n`/`N` repeat.
    pub last_search: Option<Search>,
    /// Active/recent downloads by id, retained until cleared from the view.
    pub downloads: BTreeMap<u64, Download>,
    /// Cleared to false to request shutdown.
    pub running: bool,
}

/// A remembered in-page search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub text: String,
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

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn capabilities_are_independent() {
        let mut p = Permissions::default();
        p.set_capability(
            "example.com",
            Capability::Notifications,
            PermissionPolicy::Allow,
        );
        assert_eq!(
            p.policy_for("example.com", Capability::Notifications),
            PermissionPolicy::Allow
        );
        // Granting notifications must not grant the camera; it stays at default.
        assert_eq!(
            p.policy_for("example.com", Capability::Camera),
            PermissionPolicy::default()
        );
    }

    #[test]
    fn set_all_applies_to_every_capability() {
        let mut p = Permissions::default();
        p.set_all("example.com", PermissionPolicy::Allow);
        for cap in Capability::ALL {
            assert_eq!(p.policy_for("example.com", cap), PermissionPolicy::Allow);
        }
    }

    #[test]
    fn revoke_reverts_to_default() {
        let mut p = Permissions::default();
        p.set_capability("example.com", Capability::Camera, PermissionPolicy::Allow);
        let row = p.rows().into_iter().next().unwrap();
        p.revoke_row(&row);
        assert_eq!(
            p.policy_for("example.com", Capability::Camera),
            PermissionPolicy::default()
        );
        assert!(p.rows().is_empty());
    }

    #[test]
    fn old_bare_string_config_parses_as_all() {
        let toml = "default = \"ask\"\n[sites]\n\"example.com\" = \"allow\"\n";
        let p: Permissions = toml::from_str(toml).unwrap();
        assert_eq!(p.default, PermissionPolicy::Ask);
        assert_eq!(
            p.policy_for("example.com", Capability::Camera),
            PermissionPolicy::Allow
        );
    }

    #[test]
    fn per_capability_config_parses() {
        let toml = "[sites]\n\"example.com\" = { geolocation = \"allow\", camera = \"deny\" }\n";
        let p: Permissions = toml::from_str(toml).unwrap();
        assert_eq!(
            p.policy_for("example.com", Capability::Geolocation),
            PermissionPolicy::Allow
        );
        assert_eq!(
            p.policy_for("example.com", Capability::Camera),
            PermissionPolicy::Deny
        );
    }

    #[test]
    fn permissions_round_trip_through_toml() {
        let mut p = Permissions::default();
        p.set_all("a.test", PermissionPolicy::Allow);
        p.set_capability("b.test", Capability::Geolocation, PermissionPolicy::Deny);
        let text = toml::to_string_pretty(&p).unwrap();
        let back: Permissions = toml::from_str(&text).unwrap();
        assert_eq!(p, back);
    }
}
