//! The unified message type.
//!
//! Every source of change (commands, engine signals, async results) produces a
//! [`Msg`]. Messages are the only thing the dispatch loop consumes, and
//! [`crate::core::update::update`] is the only thing that interprets them.

use crate::core::command::{ClipboardSource, Command, OpenTarget};
use crate::core::key::Key;
use crate::core::state::{Capability, Config, HistoryRow, TabId};

/// Correlates an asynchronous request (e.g. a JS evaluation) with its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Load lifecycle stages reported by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadEvent {
    Started,
    Committed,
    Finished,
}

/// Why a JS evaluation was requested, so its result can be routed.
///
/// The engine only knows a [`RequestId`] and a raw result, so the purpose is
/// recorded in state when the effect is emitted and looked up when the result
/// arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsPurpose {
    /// No result handling needed.
    FireAndForget,
    /// Result is the vertical scroll percentage (0-100).
    ReadScrollPercent,
    /// Result is the page's `scrollX`/`scrollY`, to record against the carried
    /// URL (the page the read was requested for, so a late result still lands
    /// on the right page).
    ReadScrollOffset { url: String },
    /// Result is the space-joined list of rendered hint labels.
    HintsShown,
    /// Result is the href of a followed hint, to open in a new tab.
    HintHref,
    /// Result is the resolved next/previous page URL, to open in the current tab.
    PageRelLink,
    /// Result is a ready-to-render HTML document of the page source, to show in
    /// the current tab or a new tab.
    ViewSource { new_tab: bool },
}

/// A single unit of change entering the core.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A raw key press (Normal-mode input), resolved against the binding trie.
    Key(Key),
    /// A parsed command to execute.
    Command(Command),
    /// A load lifecycle transition for a tab.
    Load { tab: TabId, event: LoadEvent },
    /// A tab's page title changed.
    TitleChanged { tab: TabId, title: String },
    /// A tab's URI changed.
    UriChanged { tab: TabId, uri: String },
    /// A tab's estimated load progress changed (0.0-1.0).
    Progress { tab: TabId, fraction: f64 },
    /// The result of an asynchronous JS evaluation, correlated by id.
    JsResult {
        id: RequestId,
        tab: TabId,
        result: Result<String, String>,
    },
    /// The command-line input text was edited by the user.
    CommandLineChanged(String),
    /// Select the next completion candidate.
    CompletionNext,
    /// Select the previous completion candidate.
    CompletionPrev,
    /// Commit the highlighted completion candidate into the command line.
    CompletionApply,
    /// Asynchronous history completion results, tagged with the generation they
    /// were requested for and the command-line prefix to apply.
    HistoryCompletion {
        generation: u64,
        prefix: String,
        entries: Vec<(String, String)>,
    },
    /// An input element gained or lost focus in a tab (insert-mode auto switch).
    InputFocusChanged { tab: TabId, focused: bool },
    /// A tab's web content process terminated unexpectedly.
    Crashed { tab: TabId },
    /// A favicon became available or changed (the icon itself lives in the
    /// GUI-side store; this only triggers a tab-list redraw).
    FaviconChanged,
    /// A website-data clear finished; carries a human label and the outcome.
    DataCleared {
        label: String,
        result: Result<(), String>,
    },
    /// The configuration file was reloaded from disk. Boxed because `Config` is
    /// large relative to the other message variants.
    ConfigLoaded(Box<Config>),
    /// A session's tab URLs were loaded from disk.
    SessionLoaded(Vec<String>),
    /// Asynchronous results for the history management view, tagged with the
    /// generation they were requested for so stale results can be ignored.
    HistoryViewResult {
        generation: u64,
        rows: Vec<HistoryRow>,
    },
    /// A plugin requested a status-bar message.
    PluginMessage(String),
    /// A plugin awaited a JavaScript evaluation in the active tab.
    PluginEvalRequest { id: u64, script: String },
    /// The result of a plugin's awaited JS evaluation, to resume the plugin.
    PluginEvalResult { id: u64, result: String },
    /// A page requested a capability whose policy is `ask`; the engine holds the
    /// request (keyed by `id`) until the user's decision resolves it.
    PermissionRequested {
        id: u64,
        host: String,
        capability: Capability,
    },
    /// An in-page search reported its match count (0 means no matches).
    FindResult { tab: TabId, matches: u32 },
    /// A download started; `filename` is its chosen destination name, `path` the
    /// full destination, and `source` the original URL (kept for retry).
    DownloadStarted {
        id: u64,
        filename: String,
        path: String,
        source: String,
    },
    /// Live progress for a download; `total` is `0` when unknown.
    DownloadProgress { id: u64, received: u64, total: u64 },
    /// A download finished. The destination path is held on the download record.
    DownloadFinished { id: u64 },
    /// A download failed.
    DownloadFailed { id: u64, error: String },
    /// A download was cancelled by the user.
    DownloadCancelled { id: u64 },
    /// A `:save` finished: `Ok` carries the saved path, `Err` the failure.
    PageSaved(Result<String, String>),
    /// Text read from a clipboard source, with the source and the placement
    /// that requested it; an empty string means the read was empty or failed.
    ClipboardRead {
        text: String,
        source: ClipboardSource,
        target: OpenTarget,
    },
}
