//! Side effects as inspectable data.
//!
//! [`update`](crate::core::update::update) never performs side effects inline; it
//! returns [`Effect`] values that the [`EffectRunner`](crate::core::runtime::EffectRunner)
//! executes afterwards. Because effects are plain data, `update` can be unit-tested
//! by asserting on the returned effects without any GTK or engine present.

use crate::core::msg::{JsPurpose, RequestId};
use crate::core::state::{Permissions, TabId};
use std::path::PathBuf;

/// Severity of a user-facing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    Info,
    Error,
}

/// A side effect to be carried out by the effect runner.
///
/// Effects that produce a value (currently [`Effect::EvalJs`]) deliver it back to
/// the queue as a new [`Msg`](crate::core::msg::Msg) correlated by [`RequestId`].
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Load a URI in the given tab.
    LoadUri { tab: TabId, uri: String },
    /// Reload the given tab.
    Reload { tab: TabId, bypass_cache: bool },
    /// Stop loading the given tab.
    Stop { tab: TabId },
    /// Navigate the given tab back.
    GoBack { tab: TabId },
    /// Navigate the given tab forward.
    GoForward { tab: TabId },
    /// Evaluate JavaScript in the given tab; the result returns as a `JsResult` message.
    EvalJs {
        id: RequestId,
        tab: TabId,
        script: String,
        purpose: JsPurpose,
    },
    /// Create a web view for a newly opened tab (state already holds the model).
    OpenTab {
        id: TabId,
        uri: String,
        background: bool,
    },
    /// Destroy the web view for a closed tab.
    CloseTab { tab: TabId },
    /// Make the given tab the visible one.
    FocusTab { tab: TabId },
    /// Write text to the system clipboard.
    SetClipboard(String),
    /// Persist the quickmarks (name, url) to disk.
    SaveQuickmarks(Vec<(String, String)>),
    /// Persist the bookmarks (url, title) to disk.
    SaveBookmarks(Vec<(String, String)>),
    /// Query history for command-line completion; results return as a message.
    QueryHistory {
        query: String,
        prefix: String,
        generation: u64,
    },
    /// Record a visited page in history.
    RecordHistory { uri: String, title: String },
    /// Re-render the status bar from current state.
    RenderStatus,
    /// Re-render the tab bar from current state.
    RenderTabs,
    /// Re-render the completion popup from current state.
    RenderCompletion,
    /// Re-render the permission management list from current state.
    RenderPermissions,
    /// Re-render the download management list from current state.
    RenderDownloads,
    /// Apply the current theme (colors, font) to the chrome.
    ApplyTheme,
    /// Push the current per-site permission policy to the engine.
    SyncPermissions(Permissions),
    /// Resolve a deferred permission request (held by the engine) by `id`.
    ResolvePermission { id: u64, allow: bool },
    /// Set a tab's page zoom level.
    SetZoom { tab: TabId, level: f64 },
    /// Search a tab's page for `text`, moving to the first match.
    Find { tab: TabId, text: String },
    /// Move to the next in-page match, wrapping at the end.
    FindNext { tab: TabId },
    /// Move to the previous in-page match (best-effort; see `EngineView`).
    FindPrev { tab: TabId },
    /// Clear the in-page search highlight.
    FindClear { tab: TabId },
    /// Persist the per-site permission rules to the data-dir store.
    SavePermissions(Permissions),
    /// Cancel an in-flight download by id.
    CancelDownload { id: u64 },
    /// Retry a failed download by id; the source URL is resolved from state.
    RetryDownload { id: u64 },
    /// Open a finished download's file through the safe external-open path.
    OpenPath { path: PathBuf },
    /// Open the folder containing a finished download's file.
    RevealPath { path: PathBuf },
    /// Reload the configuration file from disk.
    ReloadConfig,
    /// Persist a named session's tab URLs.
    SaveSession { name: String, urls: Vec<String> },
    /// Load a named session; its URLs return as `Msg::SessionLoaded`.
    LoadSession { name: String },
    /// Fire a cold-event plugin hook with a string argument.
    FireHook { event: String, arg: String },
    /// Recompile and reload all plugins.
    ReloadPlugins,
    /// Evaluate JavaScript for an awaiting plugin; result returns as `PluginEvalResult`.
    PluginEval { id: u64, tab: TabId, script: String },
    /// Deliver an awaited JS result back to the suspended plugin.
    ResolvePluginEval { id: u64, result: String },
    /// Display a transient message to the user.
    ShowMessage { level: MessageLevel, text: String },
    /// Report current resident memory and live view count to the status bar.
    ReportMemory,
    /// Tear down the application.
    Quit,
}
