//! The unified message type.
//!
//! Every source of change (commands, engine signals, async results) produces a
//! [`Msg`]. Messages are the only thing the dispatch loop consumes, and
//! [`crate::core::update::update`] is the only thing that interprets them.

use crate::core::command::Command;
use crate::core::key::Key;
use crate::core::state::TabId;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsPurpose {
    /// No result handling needed.
    FireAndForget,
    /// Result is the vertical scroll percentage (0-100).
    ReadScrollPercent,
    /// Result is the space-joined list of rendered hint labels.
    HintsShown,
    /// Result is the href of a followed hint, to open in a new tab.
    HintHref,
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
    /// An input element gained or lost focus in a tab (insert-mode auto switch).
    InputFocusChanged { tab: TabId, focused: bool },
    /// A tab's web content process terminated unexpectedly.
    Crashed { tab: TabId },
}
