//! The sole state-mutation point.
//!
//! [`update`] is synchronous, non-blocking, and the only function that mutates
//! [`State`]. It returns the side effects to perform; it never performs them
//! inline. Results of asynchronous effects re-enter through [`update`] as new
//! messages (see [`Msg::JsResult`]).

use crate::core::bindings::build_bindings;
use crate::core::command::{
    ClearScope, ClipboardSource, Command, HintTarget, JsToggle, OpenTarget, ScrollDir, YankWhat,
};
use crate::core::completion::{CompletionItem, complete};
use crate::core::effect::{Effect, MessageLevel};
use crate::core::key::{Key, display_sequence, parse_key_string};
use crate::core::msg::{JsPurpose, LoadEvent, Msg};
use crate::core::state::{
    Bookmark, Download as DownloadRecord, DownloadStatus, Mode, PermissionPolicy, PermissionPrompt,
    Search, SearchEngines, SearchStatus, SplitDir, State, TABS_COLLAPSED_WIDTH, TabId, Tabs,
};
use crate::core::trie::TrieMatch;

use std::path::PathBuf;

/// Apply a single message to the state and return the effects to perform.
pub fn update(state: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => match state.mode.current {
            Mode::Hint => handle_hint_key(state, key),
            Mode::Prompt => handle_prompt_key(state, key),
            Mode::Permissions => handle_permissions_key(state, key),
            Mode::Downloads => handle_downloads_key(state, key),
            Mode::History => handle_history_key(state, key),
            Mode::Buffer => handle_buffer_key(state, key),
            // The controller forwards keys to the page in passthrough mode, so
            // the core should never act on one even if it slips through.
            Mode::Passthrough => Vec::new(),
            _ => handle_key(state, key),
        },
        Msg::Command(cmd) => handle_command(state, cmd),

        Msg::Load { tab, event } => {
            let mut effects = Vec::new();
            if let Some(t) = state.tabs.get_mut(tab) {
                match event {
                    LoadEvent::Started => {
                        t.loading = true;
                        t.crashed = false;
                    }
                    LoadEvent::Committed => {}
                    LoadEvent::Finished => {
                        t.loading = false;
                        t.progress = 1.0;
                        let (uri, title) = (t.url.clone(), t.title.clone());
                        // Private tabs never record history.
                        if !t.private && !uri.is_empty() && uri != "about:blank" {
                            effects.push(Effect::RecordHistory {
                                uri: uri.clone(),
                                title,
                            });
                        }
                        effects.push(Effect::FireHook {
                            event: "page_load".to_string(),
                            arg: uri,
                        });
                    }
                }
            }
            // Re-apply dark mode to the tab that just finished loading.
            if event == LoadEvent::Finished && state.dark_mode {
                let id = state.alloc_request_id();
                effects.push(Effect::EvalJs {
                    id,
                    tab,
                    script: DARK_APPLY_JS.to_string(),
                    purpose: JsPurpose::FireAndForget,
                });
            }
            // Restore a remembered scroll offset: first after a back/forward
            // reload (keyed by the landing URL), otherwise the autosaved offset
            // on the restored active tab's first finished load.
            if event == LoadEvent::Finished {
                let back_forward = state.tabs.get(tab).and_then(|t| {
                    t.restore_scroll
                        .then(|| t.scroll_offsets.get(&t.url))
                        .flatten()
                });
                if let Some(t) = state.tabs.get_mut(tab) {
                    t.restore_scroll = false;
                }
                if let Some(offset) = back_forward {
                    let eff = restore_scroll_effect(state, tab, offset);
                    effects.push(eff);
                } else if state.tabs.active_id() == Some(tab)
                    && let Some(offset) = state.restored_scroll.take()
                {
                    let eff = restore_scroll_effect(state, tab, offset);
                    effects.push(eff);
                }
            }
            if state.tabs.active_id() == Some(tab) {
                effects.push(Effect::RenderStatus);
            }
            effects
        }

        Msg::TitleChanged { tab, title } => {
            let mut effects = Vec::new();
            if let Some(t) = state.tabs.get_mut(tab) {
                t.title = title;
                effects.push(Effect::RenderTabs);
                if state.tabs.active_id() == Some(tab) {
                    effects.push(Effect::RenderStatus);
                }
            }
            effects
        }

        Msg::UriChanged { tab, uri } => {
            let mut effects = Vec::new();
            if let Some(t) = state.tabs.get_mut(tab) {
                t.url = uri;
                if state.tabs.active_id() == Some(tab) {
                    effects.push(Effect::RenderStatus);
                }
            }
            effects
        }

        Msg::Progress { tab, fraction } => {
            let mut effects = Vec::new();
            let active = state.tabs.active_id() == Some(tab);
            if let Some(t) = state.tabs.get_mut(tab) {
                let was = progress_segment(t.progress);
                t.progress = fraction;
                // Re-render only when the displayed progress actually changes, so a
                // burst of fine-grained ticks does not rebuild the status bar.
                if active && progress_segment(fraction) != was {
                    effects.push(Effect::RenderStatus);
                }
            }
            effects
        }

        Msg::JsResult { id, tab, result } => {
            let Some(purpose) = state.pending_js.remove(&id) else {
                return Vec::new();
            };
            match purpose {
                JsPurpose::FireAndForget => Vec::new(),
                JsPurpose::ReadScrollPercent => {
                    // Only the active tab's scroll position belongs in the status bar.
                    if state.tabs.active_id() == Some(tab)
                        && let Ok(text) = result
                        && let Ok(pct) = text.trim().parse::<f64>()
                    {
                        state.status.scroll_percent = Some(pct.clamp(0.0, 100.0) as u8);
                        return vec![Effect::RenderStatus];
                    }
                    Vec::new()
                }
                JsPurpose::HintsShown => {
                    let labels: Vec<String> = result
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(String::from)
                        .collect();
                    if labels.is_empty() {
                        state.mode.leave();
                        state.hints.reset();
                        return vec![
                            Effect::ShowMessage {
                                level: MessageLevel::Info,
                                text: "no hints on this page".to_string(),
                            },
                            Effect::RenderStatus,
                        ];
                    }
                    state.hints.labels = labels;
                    vec![Effect::RenderStatus]
                }
                JsPurpose::HintHref => {
                    if let Ok(href) = result
                        && !href.is_empty()
                    {
                        // The href is page-controlled; only allow web schemes so a
                        // hint cannot navigate to file:// or other local schemes.
                        if !crate::core::command::is_safe_external_target(&href) {
                            return vec![Effect::ShowMessage {
                                level: MessageLevel::Error,
                                text: format!("blocked unsafe link: {href}"),
                            }];
                        }
                        // A hint opened from a private tab stays private.
                        let target = if state.tabs.get(tab).is_some_and(|t| t.private) {
                            OpenTarget::PrivateTab
                        } else {
                            OpenTarget::Tab
                        };
                        return handle_command(
                            state,
                            Command::Open {
                                target,
                                input: href,
                            },
                        );
                    }
                    Vec::new()
                }
                JsPurpose::PageRelLink => {
                    if let Ok(url) = result
                        && !url.is_empty()
                    {
                        // The URL is page-controlled; allow only web schemes, as
                        // the hint-href path does.
                        if !crate::core::command::is_safe_external_target(&url) {
                            return vec![Effect::ShowMessage {
                                level: MessageLevel::Error,
                                text: format!("blocked unsafe link: {url}"),
                            }];
                        }
                        return vec![Effect::LoadUri {
                            tab,
                            uri: normalize_target(&url, &state.config.search),
                        }];
                    }
                    Vec::new()
                }
                JsPurpose::ReadScrollOffset { url } => {
                    if let Ok(text) = result
                        && let Some(offset) = parse_offset(&text)
                        && let Some(t) = state.tabs.get_mut(tab)
                    {
                        t.scroll_offsets.record(&url, offset);
                    }
                    Vec::new()
                }
                JsPurpose::ViewSource { new_tab } => {
                    let Ok(html) = result else {
                        return Vec::new();
                    };
                    if html.is_empty() {
                        return Vec::new();
                    }
                    if new_tab {
                        let mut effects = open_tab(state, "about:blank".to_string(), false, false, false);
                        if let Some(new_id) = state.tabs.active_id() {
                            effects.push(Effect::LoadHtml { tab: new_id, html });
                        }
                        effects
                    } else {
                        vec![Effect::LoadHtml { tab, html }]
                    }
                }
            }
        }

        Msg::CommandLineChanged(text) => {
            state.command_line.text = text.clone();
            // Ignore echoes of the current query (e.g. a render re-setting the
            // entry to the same text); only a genuine edit recomputes.
            if text == state.completion.query {
                return Vec::new();
            }
            recompute_completion(state, text)
        }
        // Tab/Shift-Tab move the highlight in the list only; the command line
        // keeps the user's typed text (cycling, not completing).
        Msg::CompletionNext => {
            if state.completion.next().is_some() {
                vec![Effect::RenderCompletion]
            } else {
                Vec::new()
            }
        }
        Msg::CompletionPrev => {
            if state.completion.prev().is_some() {
                vec![Effect::RenderCompletion]
            } else {
                Vec::new()
            }
        }
        Msg::CompletionApply => {
            // Space commits the highlighted candidate into the command line
            // (without executing) and recomputes for the next position.
            let Some(committed) = state.completion.preview().map(str::to_string) else {
                return Vec::new();
            };
            state.command_line.text = committed.clone();
            let mut effects = vec![Effect::RenderStatus];
            effects.extend(recompute_completion(state, committed));
            effects
        }
        Msg::HistoryCompletion {
            generation,
            prefix,
            entries,
        } => {
            if generation != state.completion.generation {
                return Vec::new();
            }
            for (url, title) in entries {
                let command_line = format!("{prefix}{url}");
                if state
                    .completion
                    .items
                    .iter()
                    .any(|i| i.command_line == command_line)
                {
                    continue;
                }
                let display = if title.is_empty() {
                    url
                } else {
                    format!("{title}  {url}")
                };
                state.completion.items.push(CompletionItem {
                    display,
                    command_line,
                });
            }
            vec![Effect::RenderCompletion]
        }

        Msg::InputFocusChanged { tab, focused } => {
            // Auto-switch insert mode only for the active tab and only in/out of
            // Normal/Insert (never override Command mode).
            if state.tabs.active_id() != Some(tab) {
                return Vec::new();
            }
            match (focused, state.mode.current) {
                (true, Mode::Normal) => {
                    state.mode.enter(Mode::Insert);
                    vec![Effect::RenderStatus]
                }
                (false, Mode::Insert) => {
                    state.mode.leave();
                    vec![Effect::RenderStatus]
                }
                _ => Vec::new(),
            }
        }

        Msg::ConfigLoaded(config) => {
            state.config = *config;
            rebuild_bindings(state);
            vec![
                Effect::ApplyTheme,
                Effect::SyncPermissions(state.config.permissions.clone()),
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: "config reloaded".to_string(),
                },
            ]
        }

        Msg::SessionLoaded(tabs) => {
            let mut effects = Vec::new();
            let mut any_pinned = false;
            for (url, pinned) in tabs {
                any_pinned |= pinned;
                effects.extend(open_tab(state, url, true, false, pinned));
            }
            // Resort once so restored pinned tabs sort to the top.
            if any_pinned {
                state.tabs.repin_sort();
                effects.push(Effect::RenderTabs);
            }
            effects
        }

        Msg::HistoryViewResult { generation, rows } => {
            // Ignore results from a query older than the current one.
            if generation != state.history_view.generation {
                return Vec::new();
            }
            state.history_view.rows = rows;
            let len = state.history_view.rows.len();
            if len == 0 {
                state.history_view.selected = 0;
            } else if state.history_view.selected >= len {
                state.history_view.selected = len - 1;
            }
            vec![Effect::RenderHistory]
        }

        Msg::PluginMessage(text) => vec![Effect::ShowMessage {
            level: MessageLevel::Info,
            text,
        }],
        Msg::PluginEvalRequest { id, script } => match state.tabs.active_id() {
            Some(tab) => vec![Effect::PluginEval { id, tab, script }],
            // No active tab: resolve immediately so the plugin does not hang.
            None => vec![Effect::ResolvePluginEval {
                id,
                result: String::new(),
            }],
        },
        Msg::PluginEvalResult { id, result } => {
            vec![Effect::ResolvePluginEval { id, result }]
        }

        Msg::PermissionRequested {
            id,
            host,
            capability,
        } => {
            state.prompts.push_back(PermissionPrompt {
                id,
                host,
                capability,
            });
            // Show this prompt now if no other is already being answered.
            if state.prompts.len() == 1 {
                state.mode.enter(Mode::Prompt);
            }
            vec![Effect::RenderStatus]
        }

        Msg::FindResult { tab, matches } => {
            if state.tabs.active_id() != Some(tab) {
                return Vec::new();
            }
            state.status.search = Some(SearchStatus {
                total: Some(matches as usize),
            });
            vec![Effect::RenderStatus]
        }
        Msg::DownloadStarted {
            id,
            filename,
            path,
            source,
        } => {
            state.downloads.insert(
                id,
                DownloadRecord {
                    filename: filename.clone(),
                    path: PathBuf::from(path),
                    received: 0,
                    total: 0,
                    status: DownloadStatus::Active,
                    source,
                },
            );
            download_message(state, format!("downloading {filename}"), MessageLevel::Info)
        }
        Msg::DownloadProgress {
            id,
            received,
            total,
        } => {
            if let Some(dl) = state.downloads.get_mut(&id) {
                dl.received = received;
                dl.total = total;
            }
            if state.mode.current == Mode::Downloads {
                vec![Effect::RenderDownloads]
            } else {
                Vec::new()
            }
        }
        Msg::DownloadFinished { id, .. } => {
            if let Some(dl) = state.downloads.get_mut(&id) {
                dl.status = DownloadStatus::Finished;
                // Surface complete progress so the view does not stick below 100%.
                if dl.total > 0 {
                    dl.received = dl.total;
                }
            }
            let text = state
                .downloads
                .get(&id)
                .map(|dl| format!("downloaded {}: {}", dl.filename, dl.path.display()))
                .unwrap_or_default();
            download_message(state, text, MessageLevel::Info)
        }
        Msg::DownloadFailed { id, error } => {
            if let Some(dl) = state.downloads.get_mut(&id) {
                // A user-cancelled download reports its abort as a failure; keep
                // the Cancelled status rather than overwriting it with Failed.
                if dl.status != DownloadStatus::Cancelled {
                    dl.status = DownloadStatus::Failed;
                }
            }
            let text = state
                .downloads
                .get(&id)
                .map(|dl| format!("download failed {}: {error}", dl.filename))
                .unwrap_or_default();
            download_message(state, text, MessageLevel::Error)
        }
        Msg::DownloadCancelled { id } => {
            if let Some(dl) = state.downloads.get_mut(&id) {
                dl.status = DownloadStatus::Cancelled;
            }
            let text = state
                .downloads
                .get(&id)
                .map(|dl| format!("cancelled {}", dl.filename))
                .unwrap_or_default();
            download_message(state, text, MessageLevel::Info)
        }

        Msg::ClipboardRead {
            text,
            source,
            target,
        } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                let what = match source {
                    ClipboardSource::Clipboard => "clipboard",
                    ClipboardSource::Primary => "selection",
                };
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("{what} is empty"),
                }];
            }
            // Reuse the normalize-and-open path so a URL loads and other text
            // searches, exactly as a typed `:open`/`:tabopen` does.
            handle_command(
                state,
                Command::Open {
                    target,
                    input: trimmed.to_string(),
                },
            )
        }

        Msg::PageSaved(result) => match result {
            Ok(path) => {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
                    .to_string();
                vec![Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("saved page: {name}"),
                }]
            }
            Err(e) => vec![Effect::ShowMessage {
                level: MessageLevel::Error,
                text: format!("save failed: {e}"),
            }],
        },

        Msg::Crashed { tab } => {
            let mut effects = Vec::new();
            if let Some(t) = state.tabs.get_mut(tab) {
                t.crashed = true;
                t.loading = false;
            }
            effects.push(Effect::ShowMessage {
                level: MessageLevel::Error,
                text: "Web process crashed. Press r to reload.".to_string(),
            });
            if state.tabs.active_id() == Some(tab) {
                effects.push(Effect::RenderStatus);
            }
            effects
        }

        // The favicon itself lives in the GUI-side store; just redraw the rows.
        Msg::FaviconChanged => vec![Effect::RenderTabs],

        Msg::DataCleared { label, result } => {
            let (level, text) = match result {
                Ok(()) => (MessageLevel::Info, format!("cleared {label}")),
                Err(e) => (MessageLevel::Error, format!("clear {label} failed: {e}")),
            };
            vec![Effect::ShowMessage { level, text }]
        }
    }
}

/// Resolve a Normal-mode key press: accumulate a count prefix, feed the binding
/// trie, and dispatch the matched command.
fn handle_key(state: &mut State, key: Key) -> Vec<Effect> {
    if state.input.pending.is_empty() && key.is_count_digit() {
        state.input.count.push_str(&key.sym);
        return vec![Effect::RenderStatus];
    }

    state.input.pending.push(key);
    match state.bindings.lookup(&state.input.pending) {
        TrieMatch::Partial => vec![Effect::RenderStatus],
        TrieMatch::None => {
            state.input.pending.clear();
            state.input.count.clear();
            vec![Effect::RenderStatus]
        }
        // No partial-match timeout yet: an ambiguous prefix fires immediately.
        TrieMatch::Exact(cmd) | TrieMatch::Ambiguous(cmd) => {
            let count = state.input.count.parse::<u32>().ok().filter(|n| *n > 0);
            state.input.pending.clear();
            state.input.count.clear();
            match Command::parse(&cmd) {
                Ok(mut c) => {
                    if let Some(n) = count {
                        c = c.with_count(n);
                    }
                    // Refresh the status line first to clear the pending-key
                    // indicator, so a command's own notice lands last and sticks.
                    let mut effects = vec![Effect::RenderStatus];
                    effects.extend(handle_command(state, c));
                    effects
                }
                Err(text) => vec![
                    Effect::RenderStatus,
                    Effect::ShowMessage {
                        level: MessageLevel::Error,
                        text,
                    },
                ],
            }
        }
    }
}

/// Characters used to generate hint labels.
const HINT_CHARS: &str = "asdfghjkl";

/// Inject a dark-mode style that inverts the page and re-inverts media.
const DARK_APPLY_JS: &str = "(function(){var s=document.getElementById('qbrsh-dark');if(!s){s=document.createElement('style');s.id='qbrsh-dark';s.textContent='html{filter:invert(1) hue-rotate(180deg) !important;background:#fff !important}img,video,iframe,canvas,[style*=\"background-image\"]{filter:invert(1) hue-rotate(180deg) !important}';(document.head||document.documentElement).appendChild(s);}})()";

/// Remove the dark-mode style.
const DARK_REMOVE_JS: &str =
    "(function(){var s=document.getElementById('qbrsh-dark');if(s)s.remove();})()";

/// Handle a key press while answering a permission prompt: `y` allow once, `a`
/// always allow, `n` deny once, `d` always deny. Other keys are ignored.
fn handle_prompt_key(state: &mut State, key: Key) -> Vec<Effect> {
    let (allow, remember) = match key.sym.as_str() {
        "y" => (true, false),
        "a" => (true, true),
        "n" => (false, false),
        "d" => (false, true),
        _ => return Vec::new(),
    };
    resolve_front_prompt(state, allow, remember)
}

/// Resolve the active permission prompt, optionally persisting the choice, then
/// advance to the next queued prompt or leave prompt mode.
fn resolve_front_prompt(state: &mut State, allow: bool, remember: bool) -> Vec<Effect> {
    let Some(prompt) = state.prompts.pop_front() else {
        state.mode.leave();
        return vec![Effect::RenderStatus];
    };
    let mut effects = vec![Effect::ResolvePermission {
        id: prompt.id,
        allow,
    }];
    if remember {
        let policy = if allow {
            PermissionPolicy::Allow
        } else {
            PermissionPolicy::Deny
        };
        state
            .config
            .permissions
            .set_capability(&prompt.host, prompt.capability, policy);
        effects.push(Effect::SyncPermissions(state.config.permissions.clone()));
        effects.push(Effect::SavePermissions(state.config.permissions.clone()));
    }
    if state.prompts.is_empty() {
        state.mode.leave();
    }
    effects.push(Effect::RenderStatus);
    effects
}

/// Handle a key press in the permission management view: move the selection,
/// set the selected rule's policy, revoke it, or (via ModeLeave) close.
fn handle_permissions_key(state: &mut State, key: Key) -> Vec<Effect> {
    let len = state.perm_view.rows.len();
    match key.sym.as_str() {
        "j" | "Down" if len > 0 => {
            state.perm_view.selected = (state.perm_view.selected + 1) % len;
            vec![Effect::RenderPermissions]
        }
        "k" | "Up" if len > 0 => {
            state.perm_view.selected = (state.perm_view.selected + len - 1) % len;
            vec![Effect::RenderPermissions]
        }
        "a" | "d" | "s" if len > 0 => {
            let policy = match key.sym.as_str() {
                "a" => PermissionPolicy::Allow,
                "d" => PermissionPolicy::Deny,
                _ => PermissionPolicy::Ask,
            };
            let row = state.perm_view.rows[state.perm_view.selected].clone();
            state.config.permissions.set_row(&row, policy);
            refresh_permission_rows(state);
            persist_permission_view(state)
        }
        "x" if len > 0 => {
            let row = state.perm_view.rows[state.perm_view.selected].clone();
            state.config.permissions.revoke_row(&row);
            refresh_permission_rows(state);
            persist_permission_view(state)
        }
        _ => Vec::new(),
    }
}

/// Rebuild the management rows from config and clamp the selection.
fn refresh_permission_rows(state: &mut State) {
    state.perm_view.rows = state.config.permissions.rows();
    let len = state.perm_view.rows.len();
    if len == 0 {
        state.perm_view.selected = 0;
    } else if state.perm_view.selected >= len {
        state.perm_view.selected = len - 1;
    }
}

/// Sync and persist after a management-view change.
fn persist_permission_view(state: &State) -> Vec<Effect> {
    vec![
        Effect::SyncPermissions(state.config.permissions.clone()),
        Effect::SavePermissions(state.config.permissions.clone()),
        Effect::RenderPermissions,
    ]
}

/// Surface a download lifecycle notice, and refresh the downloads view if it is
/// open. `text` may be empty (e.g. a finish for an already-cleared record), in
/// which case no status message is shown.
fn download_message(state: &State, text: String, level: MessageLevel) -> Vec<Effect> {
    let mut effects = Vec::new();
    if !text.is_empty() {
        effects.push(Effect::ShowMessage { level, text });
    }
    if state.mode.current == Mode::Downloads {
        effects.push(Effect::RenderDownloads);
    }
    effects
}

/// The id of the selected download in newest-first order, clamped to the list.
fn selected_download_id(state: &State) -> Option<u64> {
    let len = state.downloads.len();
    if len == 0 {
        return None;
    }
    let idx = state.dl_view.selected.min(len - 1);
    state.downloads.keys().rev().nth(idx).copied()
}

/// Handle a key press in the download management view: move the selection, open
/// or reveal a finished file, cancel, retry, clear, or leave. Open and reveal
/// only act on finished downloads; cancel only on active ones; retry only on
/// failed ones; clear only on terminal states.
fn handle_downloads_key(state: &mut State, key: Key) -> Vec<Effect> {
    let len = state.downloads.len();
    match key.sym.as_str() {
        "j" | "Down" if len > 0 => {
            state.dl_view.selected = (state.dl_view.selected + 1) % len;
            vec![Effect::RenderDownloads]
        }
        "k" | "Up" if len > 0 => {
            state.dl_view.selected = (state.dl_view.selected + len - 1) % len;
            vec![Effect::RenderDownloads]
        }
        "o" => selected_file_effect(state, false),
        "r" => selected_file_effect(state, true),
        "c" => {
            let Some(id) = selected_download_id(state) else {
                return Vec::new();
            };
            let active = state
                .downloads
                .get(&id)
                .is_some_and(|dl| dl.status == DownloadStatus::Active);
            if active {
                vec![Effect::CancelDownload { id }]
            } else {
                Vec::new()
            }
        }
        "R" => {
            let Some(id) = selected_download_id(state) else {
                return Vec::new();
            };
            let failed = state
                .downloads
                .get(&id)
                .is_some_and(|dl| dl.status == DownloadStatus::Failed);
            if failed {
                vec![Effect::RetryDownload { id }]
            } else {
                Vec::new()
            }
        }
        "x" => {
            let Some(id) = selected_download_id(state) else {
                return Vec::new();
            };
            let removable = state.downloads.get(&id).is_some_and(|dl| {
                matches!(
                    dl.status,
                    DownloadStatus::Finished | DownloadStatus::Failed | DownloadStatus::Cancelled
                )
            });
            if !removable {
                return Vec::new();
            }
            state.downloads.remove(&id);
            let len = state.downloads.len();
            state.dl_view.selected = if len == 0 {
                0
            } else if state.dl_view.selected >= len {
                len - 1
            } else {
                state.dl_view.selected
            };
            vec![Effect::RenderDownloads]
        }
        "Escape" | "q" => {
            state.mode.leave();
            vec![Effect::RenderDownloads, Effect::RenderStatus]
        }
        _ => Vec::new(),
    }
}

/// Bump the view-query generation and emit a query for the current filter. Newer
/// queries carry a larger generation, so stale results are discarded on arrival.
fn refresh_history(state: &mut State) -> Vec<Effect> {
    state.history_view.generation = state.history_view.generation.wrapping_add(1);
    vec![Effect::QueryHistoryView {
        query: state.history_view.filter.clone(),
        generation: state.history_view.generation,
    }]
}

/// Handle a key press in the history management view. Browse mode moves the
/// selection, opens, or deletes; `/` enters filter editing. Filter-edit mode
/// edits the live filter and re-queries on each change.
fn handle_history_key(state: &mut State, key: Key) -> Vec<Effect> {
    if state.history_view.filter_edit {
        return handle_history_filter_key(state, key);
    }
    let len = state.history_view.rows.len();
    match key.sym.as_str() {
        "j" | "Down" if len > 0 => {
            state.history_view.selected = (state.history_view.selected + 1) % len;
            vec![Effect::RenderHistory]
        }
        "k" | "Up" if len > 0 => {
            state.history_view.selected = (state.history_view.selected + len - 1) % len;
            vec![Effect::RenderHistory]
        }
        "Return" | "o" if len > 0 => open_history_current(state),
        "t" if len > 0 => open_history_tab(state),
        "x" if len > 0 => delete_history_selected(state),
        "/" => {
            state.history_view.filter_edit = true;
            vec![Effect::RenderHistory, Effect::RenderStatus]
        }
        "Escape" | "q" => {
            state.mode.leave();
            vec![Effect::RenderHistory, Effect::RenderStatus]
        }
        _ => Vec::new(),
    }
}

/// Handle a key press while editing the history filter: printable keys and
/// Backspace edit the filter and re-query; Enter or Escape returns to browse.
fn handle_history_filter_key(state: &mut State, key: Key) -> Vec<Effect> {
    let mut finish = || -> Vec<Effect> {
        state.history_view.filter_edit = false;
        state.history_view.selected = 0;
        let mut effects = refresh_history(state);
        effects.push(Effect::RenderHistory);
        effects.push(Effect::RenderStatus);
        effects
    };
    match key.sym.as_str() {
        "Escape" | "Return" => finish(),
        "BackSpace" => {
            state.history_view.filter.pop();
            let mut effects = refresh_history(state);
            effects.push(Effect::RenderHistory);
            effects
        }
        "space" if !key.ctrl && !key.alt => {
            state.history_view.filter.push(' ');
            let mut effects = refresh_history(state);
            effects.push(Effect::RenderHistory);
            effects
        }
        _ if !key.ctrl
            && !key.alt
            && key.sym.chars().count() == 1
            && key.sym.chars().next().is_some_and(|c| !c.is_control()) =>
        {
            let c = key.sym.chars().next().unwrap();
            state.history_view.filter.push(c);
            let mut effects = refresh_history(state);
            effects.push(Effect::RenderHistory);
            effects
        }
        _ => Vec::new(),
    }
}

/// Open the selected history entry in the active tab and leave the view.
fn open_history_current(state: &mut State) -> Vec<Effect> {
    let Some(row) = state
        .history_view
        .rows
        .get(state.history_view.selected)
        .cloned()
    else {
        return Vec::new();
    };
    state.mode.leave();
    let mut effects = vec![Effect::RenderHistory, Effect::RenderStatus];
    if let Some(tab) = state.tabs.active_id() {
        effects.push(Effect::LoadUri {
            tab,
            uri: normalize_target(&row.url, &state.config.search),
        });
    }
    effects
}

/// Open the selected history entry in a new foreground tab and leave the view.
fn open_history_tab(state: &mut State) -> Vec<Effect> {
    let Some(row) = state
        .history_view
        .rows
        .get(state.history_view.selected)
        .cloned()
    else {
        return Vec::new();
    };
    state.mode.leave();
    let mut effects = vec![Effect::RenderHistory, Effect::RenderStatus];
    let uri = normalize_target(&row.url, &state.config.search);
    effects.extend(open_tab(state, uri, false, false, false));
    effects
}

/// Indices into `tabs.iter()` order of the tabs matching `query`, as a
/// case-insensitive substring of the title or URL. An empty query matches all.
pub(crate) fn filtered_tab_indices(tabs: &Tabs, query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    tabs.iter()
        .enumerate()
        .filter(|(_, t)| {
            q.is_empty() || t.title.to_lowercase().contains(&q) || t.url.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Handle a key press in the tab picker. Mirrors `handle_history_key`: `j`/`k`
/// move over the filtered list and wrap, `Enter`/`o` switch, `/` edits the
/// filter, and `Esc`/`q` leaves.
fn handle_buffer_key(state: &mut State, key: Key) -> Vec<Effect> {
    if state.buffer_view.filter_edit {
        return handle_buffer_filter_key(state, key);
    }
    let len = filtered_tab_indices(&state.tabs, &state.buffer_view.filter).len();
    match key.sym.as_str() {
        "j" | "Down" if len > 0 => {
            state.buffer_view.selected = (state.buffer_view.selected + 1) % len;
            vec![Effect::RenderBuffer]
        }
        "k" | "Up" if len > 0 => {
            state.buffer_view.selected = (state.buffer_view.selected + len - 1) % len;
            vec![Effect::RenderBuffer]
        }
        "Return" | "o" if len > 0 => switch_to_buffer_selection(state),
        "/" => {
            state.buffer_view.filter_edit = true;
            vec![Effect::RenderBuffer, Effect::RenderStatus]
        }
        "Escape" | "q" => {
            state.mode.leave();
            vec![Effect::RenderBuffer, Effect::RenderStatus]
        }
        _ => Vec::new(),
    }
}

/// Handle a key while editing the picker filter. Mirrors the history filter
/// editor; the selection resets as the filter changes.
fn handle_buffer_filter_key(state: &mut State, key: Key) -> Vec<Effect> {
    match key.sym.as_str() {
        "Escape" | "Return" => {
            state.buffer_view.filter_edit = false;
            state.buffer_view.selected = 0;
            vec![Effect::RenderBuffer, Effect::RenderStatus]
        }
        "BackSpace" => {
            state.buffer_view.filter.pop();
            state.buffer_view.selected = 0;
            vec![Effect::RenderBuffer]
        }
        "space" if !key.ctrl && !key.alt => {
            state.buffer_view.filter.push(' ');
            state.buffer_view.selected = 0;
            vec![Effect::RenderBuffer]
        }
        _ if !key.ctrl
            && !key.alt
            && key.sym.chars().count() == 1
            && key.sym.chars().next().is_some_and(|c| !c.is_control()) =>
        {
            let c = key.sym.chars().next().unwrap();
            state.buffer_view.filter.push(c);
            state.buffer_view.selected = 0;
            vec![Effect::RenderBuffer]
        }
        _ => Vec::new(),
    }
}

/// Switch to the selected picker row by its `TabId` and leave the view.
fn switch_to_buffer_selection(state: &mut State) -> Vec<Effect> {
    let matches = filtered_tab_indices(&state.tabs, &state.buffer_view.filter);
    let Some(&idx) = matches.get(state.buffer_view.selected) else {
        return Vec::new();
    };
    let Some(id) = state.tabs.iter().nth(idx).map(|t| t.id) else {
        return Vec::new();
    };
    state.mode.leave();
    let mut effects = vec![Effect::RenderBuffer, Effect::RenderStatus];
    effects.extend(focus_tab(state, id));
    effects
}

/// Delete the selected history entry locally and from the store.
fn delete_history_selected(state: &mut State) -> Vec<Effect> {
    let Some(row) = state
        .history_view
        .rows
        .get(state.history_view.selected)
        .cloned()
    else {
        return Vec::new();
    };
    state.history_view.rows.remove(state.history_view.selected);
    let len = state.history_view.rows.len();
    state.history_view.selected = if len == 0 {
        0
    } else if state.history_view.selected >= len {
        len - 1
    } else {
        state.history_view.selected
    };
    vec![
        Effect::DeleteHistory { url: row.url },
        Effect::RenderHistory,
    ]
}

/// Build an open/reveal effect for the selected download when it has finished;
/// `reveal` opens the containing folder instead of the file.
fn selected_file_effect(state: &State, reveal: bool) -> Vec<Effect> {
    let Some(id) = selected_download_id(state) else {
        return Vec::new();
    };
    let Some(dl) = state.downloads.get(&id) else {
        return Vec::new();
    };
    if dl.status != DownloadStatus::Finished {
        return Vec::new();
    }
    let path = dl.path.clone();
    if reveal {
        vec![Effect::RevealPath { path }]
    } else {
        vec![Effect::OpenPath { path }]
    }
}

/// Handle a key press while in hint mode: type into the label filter, follow a
/// uniquely-matched hint, or remove a character.
fn handle_hint_key(state: &mut State, key: Key) -> Vec<Effect> {
    if key.sym == "BackSpace" {
        state.hints.input.pop();
        return hint_filter_effects(state);
    }
    if key.ctrl || key.alt || key.sym.chars().count() != 1 {
        return Vec::new();
    }

    let candidate = format!("{}{}", state.hints.input, key.sym.to_lowercase());
    let matches: Vec<String> = state
        .hints
        .labels
        .iter()
        .filter(|l| l.starts_with(&candidate))
        .cloned()
        .collect();

    match matches.as_slice() {
        // No label has this prefix: ignore the keystroke.
        [] => Vec::new(),
        [only] => {
            let label = only.clone();
            follow_hint(state, &label)
        }
        _ => {
            state.hints.input = candidate;
            hint_filter_effects(state)
        }
    }
}

/// Dim the hints that no longer match the typed prefix.
fn hint_filter_effects(state: &mut State) -> Vec<Effect> {
    let prefix = state.hints.input.clone();
    let mut effects = fire_js(
        state,
        format!("window.__qbrshHints.filter({})", js_arg(&prefix)),
    );
    effects.push(Effect::RenderStatus);
    effects
}

/// Follow the element for `label` per the active hint target, then leave hint mode.
fn follow_hint(state: &mut State, label: &str) -> Vec<Effect> {
    let target = state.hints.target;
    state.mode.leave();
    state.hints.reset();
    match target {
        HintTarget::Current => {
            let mut effects = fire_js(
                state,
                format!("window.__qbrshHints.followClick({})", js_arg(label)),
            );
            effects.push(Effect::RenderStatus);
            effects
        }
        HintTarget::Tab => {
            let Some(tab) = state.tabs.active_id() else {
                return vec![Effect::RenderStatus];
            };
            let id = state.alloc_request_id();
            state.pending_js.insert(id, JsPurpose::HintHref);
            vec![
                Effect::EvalJs {
                    id,
                    tab,
                    script: format!("window.__qbrshHints.getHref({})", js_arg(label)),
                    purpose: JsPurpose::HintHref,
                },
                Effect::RenderStatus,
            ]
        }
    }
}

/// Recompute the completion list from `query` and reset the selection.
fn recompute_completion(state: &mut State, query: String) -> Vec<Effect> {
    state.completion.query = query;
    state.completion.selected = None;
    state.completion.generation = state.completion.generation.wrapping_add(1);
    let items = complete(&state.completion.query, state);
    state.completion.items = items;
    let mut effects = vec![Effect::RenderCompletion];
    effects.extend(history_query_effect(state));
    effects
}

/// If the command line is completing an `open`/`tabopen` argument, build the
/// effect that queries history for matching URLs.
fn history_query_effect(state: &State) -> Option<Effect> {
    let stripped = state.completion.query.strip_prefix(':')?;
    let (word, rest) = stripped.split_once(char::is_whitespace)?;
    if !matches!(word, "open" | "o" | "tabopen" | "t") {
        return None;
    }
    Some(Effect::QueryHistory {
        query: rest.trim_start().to_string(),
        prefix: format!(":{word} "),
        generation: state.completion.generation,
    })
}

/// Snapshot the quickmarks for persistence.
fn save_quickmarks(state: &State) -> Effect {
    Effect::SaveQuickmarks(
        state
            .quickmarks
            .iter()
            .map(|(name, url)| (name.clone(), url.clone()))
            .collect(),
    )
}

/// Snapshot the bookmarks for persistence.
fn save_bookmarks(state: &State) -> Effect {
    Effect::SaveBookmarks(
        state
            .bookmarks
            .iter()
            .map(|b| (b.url.clone(), b.title.clone()))
            .collect(),
    )
}

/// Build a fire-and-forget JS evaluation against the active tab.
fn fire_js(state: &mut State, script: String) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let id = state.alloc_request_id();
    vec![Effect::EvalJs {
        id,
        tab,
        script,
        purpose: JsPurpose::FireAndForget,
    }]
}

/// Apply a pure URL transform to the active tab's URL and navigate the current
/// tab to the result. No effect when there is no active tab or the transform
/// yields nothing (for example `gu` already at the root, or `<C-a>` on a URL
/// with no number).
fn navigate_derived(state: &State, f: impl FnOnce(&str) -> Option<String>) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let Some(url) = state.tabs.active().map(|t| t.url.clone()) else {
        return Vec::new();
    };
    match f(&url) {
        Some(derived) => vec![Effect::LoadUri {
            tab,
            uri: normalize_target(&derived, &state.config.search),
        }],
        None => Vec::new(),
    }
}

/// Resolve and follow the page's next or previous link by evaluating JS in the
/// active tab; the result returns as a `PageRelLink` message.
fn rel_link_command(state: &mut State, next: bool) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let id = state.alloc_request_id();
    state.pending_js.insert(id, JsPurpose::PageRelLink);
    vec![Effect::EvalJs {
        id,
        tab,
        script: rel_link_script(next),
        purpose: JsPurpose::PageRelLink,
    }]
}

/// Substitute command-line variables (`{url}`, `{title}`) from the active tab.
fn substitute_vars(state: &State, text: &str) -> String {
    let (url, title) = state
        .tabs
        .active()
        .map(|t| (t.url.clone(), t.title.clone()))
        .unwrap_or_default();
    text.replace("{url}", &url).replace("{title}", &title)
}

fn handle_command(state: &mut State, cmd: Command) -> Vec<Effect> {
    match cmd {
        Command::Open { target, input } => {
            // An empty target opens the configured new-tab page.
            let raw = if input.trim().is_empty() {
                state.config.newtab.clone()
            } else {
                input
            };
            let uri = normalize_target(&raw, &state.config.search);
            match target {
                OpenTarget::Current => match state.tabs.active_id() {
                    Some(tab) => {
                        // Capture the leaving page's offset before it is replaced.
                        let mut effects = capture_scroll(state);
                        effects.push(Effect::LoadUri { tab, uri });
                        effects
                    }
                    None => open_tab(state, uri, false, false, false),
                },
                OpenTarget::Tab => open_tab(state, uri, false, false, false),
                OpenTarget::PrivateTab => open_tab(state, uri, false, true, false),
            }
        }

        Command::Back(count) => navigate_history(state, count, true),
        Command::Forward(count) => navigate_history(state, count, false),
        Command::Reload { bypass_cache } => {
            with_active(state, |tab| vec![Effect::Reload { tab, bypass_cache }])
        }
        Command::Stop => with_active(state, |tab| vec![Effect::Stop { tab }]),
        Command::UrlUp(segments) => navigate_derived(state, |url| url_path_up(url, segments)),
        Command::UrlRoot => navigate_derived(state, url_host_root),
        Command::UrlIncrement(step) => navigate_derived(state, |url| url_step_number(url, step)),
        Command::PageNext => rel_link_command(state, true),
        Command::PagePrev => rel_link_command(state, false),
        Command::FocusInput(n) => {
            if state.tabs.active_id().is_none() {
                return Vec::new();
            }
            let mut effects = fire_js(state, focus_input_script(n));
            state.mode.enter(Mode::Insert);
            effects.push(Effect::RenderStatus);
            effects
        }
        Command::ViewSource { tab: new_tab } => {
            let Some(tab) = state.tabs.active_id() else {
                return Vec::new();
            };
            let url = state
                .tabs
                .active()
                .map(|t| t.url.clone())
                .unwrap_or_default();
            if url.is_empty() || url == "about:blank" {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: "no page source to view".to_string(),
                }];
            }
            // Read the page HTML in the view; the script returns a ready-to-render
            // escaped document. `view-source:` is not a loadable scheme here, so
            // the source is shown via a generated page instead.
            let id = state.alloc_request_id();
            state
                .pending_js
                .insert(id, JsPurpose::ViewSource { new_tab });
            vec![Effect::EvalJs {
                id,
                tab,
                script: VIEW_SOURCE_SCRIPT.to_string(),
                purpose: JsPurpose::ViewSource { new_tab },
            }]
        }
        Command::Print => with_active(state, |tab| vec![Effect::Print { tab }]),
        Command::SavePage => with_active(state, |tab| vec![Effect::SavePage { tab }]),
        Command::Inspect => with_active(state, |tab| vec![Effect::ToggleInspector { tab }]),

        Command::Scroll(dir, count) => scroll(state, scroll_script(dir, count.max(1))),
        Command::ScrollPage { down, half } => {
            let frac = if half { 0.5 } else { 0.9 } * if down { 1.0 } else { -1.0 };
            scroll(
                state,
                format!("window.scrollBy(0, window.innerHeight * {frac});"),
            )
        }
        Command::ScrollToPercent(pct) => scroll(
            state,
            format!(
                "window.scrollTo(0, (document.documentElement.scrollHeight - \
                 document.documentElement.clientHeight) * {} / 100);",
                pct.min(100)
            ),
        ),

        Command::Hint(target) => {
            let Some(tab) = state.tabs.active_id() else {
                return Vec::new();
            };
            state.mode.enter(Mode::Hint);
            state.hints.target = target;
            state.hints.reset();
            let id = state.alloc_request_id();
            state.pending_js.insert(id, JsPurpose::HintsShown);
            vec![
                Effect::EvalJs {
                    id,
                    tab,
                    script: format!("window.__qbrshHints.show({})", js_arg(HINT_CHARS)),
                    purpose: JsPurpose::HintsShown,
                },
                Effect::RenderStatus,
            ]
        }

        Command::Pin => pin_active(state, Some(true)),
        Command::Unpin => pin_active(state, Some(false)),
        Command::PinToggle => pin_active(state, None),

        Command::TabClose => {
            if state.tabs.active().is_some_and(|t| t.pinned) {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: "tab is pinned; unpin first with gp or :unpin".to_string(),
                }];
            }
            match state.tabs.close_active() {
            Some((closed, next)) => {
                let mut effects = vec![Effect::CloseTab { tab: closed }];
                // The active tab is the focused pane's tab; closing it collapses
                // that pane when other panes remain.
                if state.layout.panes.len() > 1 {
                    let _ = state.layout.close_focused();
                    effects.push(Effect::RenderLayout);
                }
                match next {
                    Some(next_id) => effects.extend(focus_tab(state, next_id)),
                    None => {
                        state.running = false;
                        effects.push(Effect::Quit);
                    }
                }
                effects
            }
            None => Vec::new(),
            }
        }
        Command::TabNext(count) => match state.tabs.peek_next(count) {
            Some(id) => focus_tab(state, id),
            None => Vec::new(),
        },
        Command::TabPrev(count) => match state.tabs.peek_prev(count) {
            Some(id) => focus_tab(state, id),
            None => Vec::new(),
        },
        Command::TabSelect(index) => match state.tabs.id_at_1based(index) {
            Some(id) => focus_tab(state, id),
            None => vec![Effect::ShowMessage {
                level: MessageLevel::Error,
                text: format!("no tab at index {index}"),
            }],
        },
        Command::Undo => match state.tabs.undo() {
            Some(closed) => {
                let mut effects = open_tab(state, closed.url, false, closed.private, closed.pinned);
                if closed.pinned {
                    state.tabs.repin_sort();
                    effects.push(Effect::RenderTabs);
                }
                effects
            }
            None => vec![Effect::ShowMessage {
                level: MessageLevel::Info,
                text: "no closed tabs to reopen".to_string(),
            }],
        },
        Command::TabClone => {
            match state.tabs.active().map(|t| (t.url.clone(), t.private, t.pinned)) {
                Some((url, private, pinned)) => {
                    let mut effects = open_tab(state, url, false, private, pinned);
                    if pinned {
                        state.tabs.repin_sort();
                        effects.push(Effect::RenderTabs);
                    }
                    effects
                }
                None => Vec::new(),
            }
        }
        Command::TabMove(delta) => {
            if state.tabs.move_active(delta) {
                vec![Effect::RenderTabs, Effect::RenderStatus]
            } else {
                Vec::new()
            }
        }
        Command::TabOnly => {
            let closed = state.tabs.close_others();
            let mut effects: Vec<Effect> = closed
                .into_iter()
                .map(|tab| Effect::CloseTab { tab })
                .collect();
            if !effects.is_empty() {
                effects.push(Effect::RenderTabs);
                effects.push(Effect::RenderStatus);
            }
            effects
        }

        Command::ModeEnter(mode) => {
            state.mode.enter(mode);
            if mode == Mode::Command {
                state.command_line.active = true;
            }
            vec![Effect::RenderStatus]
        }
        Command::ModeLeave if state.mode.current == Mode::Prompt => {
            // Dismissing the prompt denies the request without persisting.
            resolve_front_prompt(state, false, false)
        }
        Command::ModeLeave if state.mode.current == Mode::Permissions => {
            state.mode.leave();
            vec![Effect::RenderPermissions, Effect::RenderStatus]
        }
        Command::ModeLeave if state.mode.current == Mode::Downloads => {
            state.mode.leave();
            vec![Effect::RenderDownloads, Effect::RenderStatus]
        }
        Command::ModeLeave if state.mode.current == Mode::History => {
            state.mode.leave();
            vec![Effect::RenderHistory, Effect::RenderStatus]
        }
        Command::ModeLeave if state.mode.current == Mode::Buffer => {
            state.mode.leave();
            vec![Effect::RenderBuffer, Effect::RenderStatus]
        }
        Command::ModeLeave => {
            let was_hint = state.mode.current == Mode::Hint;
            state.mode.leave();
            state.command_line.active = false;
            state.command_line.text.clear();
            state.completion.reset();
            let mut effects = vec![Effect::RenderStatus, Effect::RenderCompletion];
            if was_hint {
                state.hints.reset();
                effects.extend(fire_js(state, "window.__qbrshHints.clear()".to_string()));
            }
            effects
        }
        Command::SetCommandLine(prefix) => {
            let text = substitute_vars(state, &prefix);
            state.mode.enter(Mode::Command);
            state.command_line.active = true;
            state.command_line.text = text.clone();
            let mut effects = vec![Effect::RenderStatus];
            effects.extend(recompute_completion(state, text));
            effects
        }
        Command::Accept => {
            // Execute the highlighted candidate if one is selected, else the
            // typed text (the command line is not rewritten while cycling).
            let typed = std::mem::take(&mut state.command_line.text);
            let to_run = state
                .completion
                .preview()
                .map(str::to_string)
                .unwrap_or(typed);
            state.command_line.active = false;
            state.completion.reset();
            state.mode.leave();
            let mut effects = vec![Effect::RenderStatus, Effect::RenderCompletion];
            let trimmed = to_run.trim();
            if trimmed.is_empty() {
                return effects;
            }
            if let Some(rest) = trimmed.strip_prefix(':') {
                let command = rest.trim();
                push_parsed(state, command, &mut effects);
                effects.push(Effect::FireHook {
                    event: "command".to_string(),
                    arg: command.to_string(),
                });
            } else if let Some(text) = trimmed.strip_prefix('/') {
                let text = text.trim().to_string();
                if let Some(tab) = state.tabs.active_id() {
                    if text.is_empty() {
                        state.last_search = None;
                        state.status.search = None;
                        effects.push(Effect::FindClear { tab });
                    } else {
                        state.last_search = Some(Search { text: text.clone() });
                        state.status.search = Some(SearchStatus { total: None });
                        effects.push(Effect::Find { tab, text });
                    }
                }
            } else {
                push_parsed(state, trimmed, &mut effects);
            }
            effects
        }

        Command::Yank(what) => {
            let Some(tab) = state.tabs.active() else {
                return Vec::new();
            };
            let value = match what {
                YankWhat::Url => tab.url.clone(),
                YankWhat::Title => tab.title.clone(),
            };
            vec![
                Effect::SetClipboard(value.clone()),
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("yanked: {value}"),
                },
            ]
        }

        Command::ClipboardOpen { source, target } => {
            vec![Effect::ReadClipboard { source, target }]
        }

        Command::QuickmarkSave(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text: "quickmark name required".to_string(),
                }];
            }
            let Some(url) = state.tabs.active().map(|t| t.url.clone()) else {
                return Vec::new();
            };
            state.quickmarks.insert(name.clone(), url);
            vec![
                save_quickmarks(state),
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("quickmark '{name}' saved"),
                },
            ]
        }
        Command::QuickmarkLoad(name) => match state.quickmarks.get(name.trim()) {
            Some(url) => {
                let url = url.clone();
                handle_command(
                    state,
                    Command::Open {
                        target: OpenTarget::Current,
                        input: url,
                    },
                )
            }
            None => vec![Effect::ShowMessage {
                level: MessageLevel::Error,
                text: format!("no quickmark '{}'", name.trim()),
            }],
        },
        Command::QuickmarkDel(name) => {
            if state.quickmarks.remove(name.trim()).is_some() {
                vec![
                    save_quickmarks(state),
                    Effect::ShowMessage {
                        level: MessageLevel::Info,
                        text: format!("quickmark '{}' deleted", name.trim()),
                    },
                ]
            } else {
                Vec::new()
            }
        }
        Command::BookmarkAdd => {
            let Some((url, title)) = state
                .tabs
                .active()
                .map(|t| (t.url.clone(), t.title.clone()))
            else {
                return Vec::new();
            };
            if !state.bookmarks.iter().any(|b| b.url == url) {
                state.bookmarks.push(Bookmark {
                    url: url.clone(),
                    title,
                });
            }
            vec![
                save_bookmarks(state),
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("bookmarked {url}"),
                },
            ]
        }
        Command::BookmarkLoad(url) => handle_command(
            state,
            Command::Open {
                target: OpenTarget::Current,
                input: url,
            },
        ),
        Command::BookmarkDel(url) => {
            let url = url.trim().to_string();
            let before = state.bookmarks.len();
            state.bookmarks.retain(|b| b.url != url);
            if state.bookmarks.len() != before {
                vec![save_bookmarks(state)]
            } else {
                Vec::new()
            }
        }

        Command::Set { key, value } => {
            // Per-domain JavaScript rules live in the data-dir site-preference
            // store, not the user config, so they are handled before delegating.
            if let Some(rest) = key.strip_prefix("javascript.") {
                return match parse_bool(&value) {
                    Ok(enabled) => {
                        if rest == "default" {
                            state.site_prefs.javascript_default = enabled;
                        } else {
                            state.site_prefs.set_javascript(rest, enabled);
                        }
                        vec![
                            Effect::SyncSitePrefs(state.site_prefs.clone()),
                            Effect::SaveSitePrefs(state.site_prefs.clone()),
                            Effect::ShowMessage {
                                level: MessageLevel::Info,
                                text: format!("{key} = {value}"),
                            },
                        ]
                    }
                    Err(text) => vec![Effect::ShowMessage {
                        level: MessageLevel::Error,
                        text,
                    }],
                };
            }
            match state.config.set(&key, &value) {
                Ok(()) => {
                    let mut effects = Vec::new();
                    if key.starts_with("permissions.") {
                        effects.push(Effect::SyncPermissions(state.config.permissions.clone()));
                    } else if key == "tabs.width" {
                        effects.push(Effect::SetTabWidth(state.config.tabs.width));
                    } else if !key.starts_with("search.") {
                        effects.push(Effect::ApplyTheme);
                    }
                    effects.push(Effect::ShowMessage {
                        level: MessageLevel::Info,
                        text: format!("{key} = {value}"),
                    });
                    effects
                }
                Err(text) => vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text,
                }],
            }
        }
        Command::ConfigSource => vec![Effect::ReloadConfig],
        Command::PluginReload => vec![Effect::ReloadPlugins],
        Command::DarkMode => {
            state.dark_mode = !state.dark_mode;
            let script = if state.dark_mode {
                DARK_APPLY_JS
            } else {
                DARK_REMOVE_JS
            };
            let mut effects = fire_js(state, script.to_string());
            effects.push(Effect::ShowMessage {
                level: MessageLevel::Info,
                text: format!("dark mode {}", if state.dark_mode { "on" } else { "off" }),
            });
            effects
        }
        Command::SessionSave(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text: "session name required".to_string(),
                }];
            }
            vec![
                Effect::SaveSession {
                    tabs: state.tabs.persistable_tabs(),
                    name: name.clone(),
                },
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("session '{name}' saved"),
                },
            ]
        }
        Command::SessionLoad(name) => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text: "session name required".to_string(),
                }];
            }
            vec![Effect::LoadSession { name }]
        }

        Command::Memory => vec![Effect::ReportMemory],

        Command::FindNext => find_repeat(state, true),
        Command::FindPrev => find_repeat(state, false),
        Command::ZoomIn => {
            let z = state.tabs.active().map(|t| t.zoom).unwrap_or(1.0);
            set_zoom(state, z + ZOOM_STEP)
        }
        Command::ZoomOut => {
            let z = state.tabs.active().map(|t| t.zoom).unwrap_or(1.0);
            set_zoom(state, z - ZOOM_STEP)
        }
        Command::ZoomReset => {
            let level = state.config.zoom.default;
            set_zoom(state, level)
        }
        Command::ZoomSet(pct) => set_zoom(state, pct as f64 / 100.0),

        Command::Permissions => {
            state.perm_view.rows = state.config.permissions.rows();
            state.perm_view.selected = 0;
            state.mode.enter(Mode::Permissions);
            vec![Effect::RenderPermissions, Effect::RenderStatus]
        }

        Command::Downloads => {
            state.dl_view.selected = 0;
            state.mode.enter(Mode::Downloads);
            vec![Effect::RenderDownloads, Effect::RenderStatus]
        }

        Command::History(arg) => {
            state.history_view.filter = arg.unwrap_or_default();
            state.history_view.filter_edit = false;
            state.history_view.selected = 0;
            state.mode.enter(Mode::History);
            let mut effects = refresh_history(state);
            effects.push(Effect::RenderHistory);
            effects.push(Effect::RenderStatus);
            effects
        }

        Command::Buffer(arg) => {
            state.buffer_view.filter = arg.unwrap_or_default();
            state.buffer_view.filter_edit = false;
            state.buffer_view.selected = 0;
            state.mode.enter(Mode::Buffer);
            vec![Effect::RenderBuffer, Effect::RenderStatus]
        }

        Command::Split | Command::Vsplit => {
            let dir = if matches!(cmd, Command::Split) {
                SplitDir::Horizontal
            } else {
                SplitDir::Vertical
            };
            split_focused(state, dir)
        }
        Command::ClosePane => {
            if state.layout.is_empty() {
                return Vec::new();
            }
            let mut effects = Vec::new();
            if state.layout.close_focused().is_some() {
                effects.push(Effect::RenderLayout);
                // `close_focused` set the focused pane to the survivor; sync the
                // active tab to it.
                if let Some(tab) = state.layout.focused_pane().map(|p| p.tab) {
                    effects.extend(focus_tab(state, tab));
                }
            }
            effects
        }
        Command::OnlyPane => {
            if state.layout.is_empty() {
                return Vec::new();
            }
            state.layout.only();
            vec![
                Effect::RenderLayout,
                Effect::RenderTabs,
                Effect::RenderStatus,
            ]
        }
        Command::FocusPane { next } => {
            if state.layout.is_empty() {
                return Vec::new();
            }
            let shifted = if next {
                state.layout.focus_next()
            } else {
                state.layout.focus_prev()
            };
            match shifted.and_then(|_| state.layout.focused_pane().map(|p| p.tab)) {
                Some(tab) => focus_tab(state, tab),
                None => Vec::new(),
            }
        }

        Command::ClearData(scope) => {
            // The per-site scope needs the active tab's registrable domain.
            let host = if scope == ClearScope::Site {
                match state
                    .tabs
                    .active()
                    .and_then(|t| crate::adblock::site_of(&t.url))
                {
                    Some(host) => Some(host),
                    None => {
                        return vec![Effect::ShowMessage {
                            level: MessageLevel::Error,
                            text: "no site to clear for the active tab".to_string(),
                        }];
                    }
                }
            } else {
                None
            };
            vec![Effect::ClearData { scope, host }]
        }
        Command::SiteJavascript(toggle) => site_javascript(state, toggle),
        Command::TabsToggle => {
            state.tabs_collapsed = !state.tabs_collapsed;
            let width = if state.tabs_collapsed {
                TABS_COLLAPSED_WIDTH
            } else {
                state.config.tabs.width
            };
            vec![Effect::RenderTabs, Effect::SetTabWidth(width)]
        }
        Command::Fullscreen => {
            state.fullscreen = !state.fullscreen;
            vec![Effect::ToggleFullscreen {
                fullscreen: state.fullscreen,
            }]
        }
        Command::Bind { keys, command } => {
            let parsed = parse_key_string(&keys);
            if parsed.is_empty() {
                return vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text: format!("invalid key sequence: {keys}"),
                }];
            }
            match state.bindings.insert_checked(&parsed, command.clone()) {
                Ok(()) => vec![Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("bound {keys} -> {command}"),
                }],
                Err(e) => vec![Effect::ShowMessage {
                    level: MessageLevel::Error,
                    text: format!("bind '{keys}' {e}"),
                }],
            }
        }
        Command::Unbind(keys) => {
            let parsed = parse_key_string(&keys);
            let removed = !parsed.is_empty() && state.bindings.remove(&parsed);
            vec![Effect::ShowMessage {
                level: MessageLevel::Info,
                text: if removed {
                    format!("unbound {keys}")
                } else {
                    format!("no binding for {keys}")
                },
            }]
        }
        Command::Bindings => {
            let mut listed: Vec<String> = state
                .bindings
                .bindings()
                .into_iter()
                .map(|(keys, cmd)| format!("{} -> {cmd}", display_sequence(&keys)))
                .collect();
            listed.sort();
            vec![Effect::ShowMessage {
                level: MessageLevel::Info,
                text: listed.join("  |  "),
            }]
        }

        Command::Quit => {
            let tabs = state.tabs.persistable_tabs();
            let active = state.tabs.persistable_active_index();
            // The active tab's last known offset, unless it is private (private
            // tabs are excluded from the autosave entirely).
            let scroll = state.tabs.active().and_then(|t| {
                if t.private {
                    None
                } else {
                    t.scroll_offsets.get(&t.url)
                }
            });
            state.running = false;
            vec![
                Effect::SaveAutosave {
                    tabs,
                    active,
                    scroll,
                },
                Effect::Quit,
            ]
        }
        Command::Nop => Vec::new(),
    }
}

/// Apply a JavaScript site-preference change to the active tab's site: update the
/// stored rule, push and persist the store, then re-apply to every open tab on
/// that site and reload them so the change takes effect.
fn site_javascript(state: &mut State, toggle: JsToggle) -> Vec<Effect> {
    let Some(url) = state.tabs.active().map(|t| t.url.clone()) else {
        return Vec::new();
    };
    let Some(host) = crate::adblock::site_of(&url) else {
        return vec![Effect::ShowMessage {
            level: MessageLevel::Error,
            text: "no site for the active tab".to_string(),
        }];
    };
    let enabled = match toggle {
        JsToggle::Enable => true,
        JsToggle::Disable => false,
        JsToggle::Toggle => !state.site_prefs.javascript_for(&host),
    };
    state.site_prefs.set_javascript(&host, enabled);

    let mut effects = vec![
        Effect::SyncSitePrefs(state.site_prefs.clone()),
        Effect::SaveSitePrefs(state.site_prefs.clone()),
    ];
    let tabs: Vec<TabId> = state
        .tabs
        .iter()
        .filter(|t| crate::adblock::site_of(&t.url).as_deref() == Some(host.as_str()))
        .map(|t| t.id)
        .collect();
    for tab in tabs {
        effects.push(Effect::SetJavascript { tab, enabled });
        effects.push(Effect::Reload {
            tab,
            bypass_cache: false,
        });
    }
    effects.push(Effect::ShowMessage {
        level: MessageLevel::Info,
        text: format!(
            "javascript {} for {host}",
            if enabled { "enabled" } else { "disabled" }
        ),
    });
    effects
}

/// Rebuild the binding trie from the current config overrides, reporting any
/// invalid entries.
fn rebuild_bindings(state: &mut State) {
    let (bindings, errors) = build_bindings(&state.config.bindings);
    state.bindings = bindings;
    for e in &errors {
        eprintln!("[qbrsh] config: {e}");
    }
}

/// Parse a boolean config value.
fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected true or false, got: {other}")),
    }
}

/// Zoom step and bounds.
const ZOOM_STEP: f64 = 0.1;
const ZOOM_MIN: f64 = 0.25;
const ZOOM_MAX: f64 = 5.0;

/// Set the active tab's zoom to `level` (clamped), updating state and the view.
fn set_zoom(state: &mut State, level: f64) -> Vec<Effect> {
    let level = level.clamp(ZOOM_MIN, ZOOM_MAX);
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    if let Some(t) = state.tabs.get_mut(tab) {
        t.zoom = level;
    }
    vec![
        Effect::SetZoom { tab, level },
        Effect::ShowMessage {
            level: MessageLevel::Info,
            text: format!("zoom {}%", (level * 100.0).round() as i64),
        },
    ]
}

/// Step to the next (`forward`) or previous match of the last in-page search, or
/// hint that there is none. Forward wraps at the end; backward is best-effort.
fn find_repeat(state: &State, forward: bool) -> Vec<Effect> {
    if state.last_search.is_none() {
        return vec![Effect::ShowMessage {
            level: MessageLevel::Info,
            text: "no active search".to_string(),
        }];
    }
    with_active(state, |tab| {
        vec![if forward {
            Effect::FindNext { tab }
        } else {
            Effect::FindPrev { tab }
        }]
    })
}

/// Run `f` with the active tab id, or produce no effects if there is none.
fn with_active(
    state: &State,
    f: impl FnOnce(crate::core::state::TabId) -> Vec<Effect>,
) -> Vec<Effect> {
    match state.tabs.active_id() {
        Some(tab) => f(tab),
        None => Vec::new(),
    }
}

/// Open a new tab in state and emit the effect to realize its web view.
/// The single place tabs and panes meet. Makes `id` the active tab and, when a
/// layout is present, focuses its pane (if `id` is mounted) or swaps it into
/// the focused pane (if it is a background tab). Maintains the invariant that
/// the active tab is the focused pane's tab.
fn focus_tab(state: &mut State, id: TabId) -> Vec<Effect> {
    state.tabs.focus_id(id);
    let mut effects = vec![Effect::RenderTabs, Effect::RenderStatus];
    if state.layout.is_empty() {
        return effects;
    }
    if let Some(pane) = state.layout.pane_with_tab(id) {
        // Already mounted: just shift focus, no content change.
        state.layout.focused = pane;
        effects.push(Effect::FocusPane { pane });
    } else {
        // Background tab: swap it into the focused pane. The pane now shows a
        // different view, so the layout must rebuild to mount it.
        let _ = state.layout.swap_focused_tab(id);
        let pane = state.layout.focused;
        effects.push(Effect::RenderLayout);
        effects.push(Effect::FocusPane { pane });
    }
    effects
}

/// Split the focused pane in `dir`, opening a new homepage tab in the new pane
/// and focusing it.
fn split_focused(state: &mut State, dir: SplitDir) -> Vec<Effect> {
    if state.layout.is_empty() || state.tabs.active_id().is_none() {
        return Vec::new();
    }
    let home = state.config.homepage.clone();
    let new_id = state.tabs.open(&home);
    if let Some(t) = state.tabs.get_mut(new_id) {
        t.zoom = state.config.zoom.default;
    }
    if state.layout.split_focused(dir, new_id).is_none() {
        return Vec::new();
    }
    let mut effects = vec![
        Effect::OpenTab {
            id: new_id,
            uri: home,
            background: true,
            private: false,
        },
        Effect::RenderLayout,
    ];
    effects.extend(focus_tab(state, new_id));
    effects
}

/// Pin (`Some(true)`), unpin (`Some(false)`), or toggle (`None`) the active
/// tab's pinned state, resorting so pinned tabs stay on top.
fn pin_active(state: &mut State, want: Option<bool>) -> Vec<Effect> {
    let changed = match want {
        Some(p) => state.tabs.set_active_pinned(p),
        None => state.tabs.toggle_active_pinned(),
    };
    if changed {
        vec![Effect::RenderTabs, Effect::RenderStatus]
    } else {
        Vec::new()
    }
}

fn open_tab(
    state: &mut State,
    uri: String,
    background: bool,
    private: bool,
    pinned: bool,
) -> Vec<Effect> {
    let id = state.tabs.open(&uri);
    let default_zoom = state.config.zoom.default;
    if let Some(t) = state.tabs.get_mut(id) {
        t.zoom = default_zoom;
        t.private = private;
        t.pinned = pinned;
    }
    let mut effects = vec![
        Effect::OpenTab {
            id,
            uri: uri.clone(),
            background,
            private,
        },
        Effect::FireHook {
            event: "tab_open".to_string(),
            arg: uri,
        },
    ];
    if background {
        effects.push(Effect::RenderTabs);
    } else {
        // Foreground tabs swap into the focused pane and become active.
        effects.extend(focus_tab(state, id));
    }
    effects
}

/// Scroll the active page and read back the new position in a single evaluation:
/// `script` performs the move and the trailing expression returns the percentage.
fn scroll(state: &mut State, script: String) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let id = state.alloc_request_id();
    state.pending_js.insert(id, JsPurpose::ReadScrollPercent);

    vec![Effect::EvalJs {
        id,
        tab,
        script: format!("{script} {SCROLL_PERCENT_SCRIPT}"),
        purpose: JsPurpose::ReadScrollPercent,
    }]
}

const SCROLL_PERCENT_SCRIPT: &str = "(function(){var e=document.documentElement;var m=e.scrollHeight-e.clientHeight;return m<=0?0:Math.round(e.scrollTop/m*100);})()";

/// Reads the page's current scroll offset as the space-joined `scrollX scrollY`.
const READ_SCROLL_OFFSET_SCRIPT: &str =
    "(function(){return Math.round(window.scrollX)+' '+Math.round(window.scrollY);})()";

/// Serializes the page's HTML and returns a ready-to-render document showing it
/// as escaped, wrapped source text. The `view-source:` scheme is not loadable in
/// this engine, so the source is shown via a generated page.
const VIEW_SOURCE_SCRIPT: &str = r#"(function(){
  var src='<!DOCTYPE '+(document.doctype?document.doctype.name:'html')+'>\n'+document.documentElement.outerHTML;
  var esc=src.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  return '<!doctype html><html><head><meta charset="utf-8"><title>Source</title></head>'+
    '<body style="margin:0"><pre style="white-space:pre-wrap;word-break:break-word;padding:1rem;font-family:monospace;font-size:13px">'+
    esc+'</pre></body></html>';
})()"#;

/// Capture the active tab's current scroll offset, keyed by its current URL, so
/// returning to that page can restore it. Emits a read-offset evaluation.
fn capture_scroll(state: &mut State) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let Some(url) = state.tabs.active().map(|t| t.url.clone()) else {
        return Vec::new();
    };
    if url.is_empty() || url == "about:blank" {
        return Vec::new();
    }
    let id = state.alloc_request_id();
    state
        .pending_js
        .insert(id, JsPurpose::ReadScrollOffset { url: url.clone() });
    vec![Effect::EvalJs {
        id,
        tab,
        script: READ_SCROLL_OFFSET_SCRIPT.to_string(),
        purpose: JsPurpose::ReadScrollOffset { url },
    }]
}

/// Navigate the active tab back or forward `count` steps. Captures the leaving
/// page's scroll offset and marks the tab so the reloaded landing page restores
/// its saved offset on load-finished.
fn navigate_history(state: &mut State, count: u32, back: bool) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let mut effects = capture_scroll(state);
    if let Some(t) = state.tabs.get_mut(tab) {
        t.restore_scroll = true;
    }
    effects.extend((0..count.max(1)).map(|_| {
        if back {
            Effect::GoBack { tab }
        } else {
            Effect::GoForward { tab }
        }
    }));
    effects
}

/// Parse the `"x y"` offset returned by [`READ_SCROLL_OFFSET_SCRIPT`].
fn parse_offset(text: &str) -> Option<(i64, i64)> {
    let mut parts = text.split_whitespace();
    let x = parts.next()?.parse::<f64>().ok()? as i64;
    let y = parts.next()?.parse::<f64>().ok()? as i64;
    Some((x, y))
}

/// A `scrollTo` that clamps to the reloaded page's bounds so it never scrolls
/// past the end when the page is now shorter than it was.
fn scroll_to_script(x: i64, y: i64) -> String {
    format!(
        "(function(){{var e=document.documentElement;\
var mx=Math.max(0,e.scrollWidth-e.clientWidth);\
var my=Math.max(0,e.scrollHeight-e.clientHeight);\
window.scrollTo(Math.min({x},mx),Math.min({y},my));}})()"
    )
}

/// Emit a fire-and-forget clamped `scrollTo` for the given tab.
fn restore_scroll_effect(state: &mut State, tab: TabId, offset: (i64, i64)) -> Effect {
    let id = state.alloc_request_id();
    Effect::EvalJs {
        id,
        tab,
        script: scroll_to_script(offset.0, offset.1),
        purpose: JsPurpose::FireAndForget,
    }
}

/// Encode a string as a JSON literal for safe embedding as a JavaScript argument,
/// so labels and prefixes cannot break out of the call (no string interpolation).
fn js_arg(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// The integer progress percentage the status bar displays, or `None` when no
/// progress is shown (idle or finished). Mirrors the status-bar render so the
/// progress re-render guard matches exactly what is displayed.
fn progress_segment(fraction: f64) -> Option<u32> {
    if fraction > 0.0 && fraction < 1.0 {
        Some((fraction * 100.0) as u32)
    } else {
        None
    }
}

fn scroll_script(dir: ScrollDir, count: u32) -> String {
    const STEP: f64 = 60.0;
    let (ux, uy) = match dir {
        ScrollDir::Up => (0.0, -1.0),
        ScrollDir::Down => (0.0, 1.0),
        ScrollDir::Left => (-1.0, 0.0),
        ScrollDir::Right => (1.0, 0.0),
    };
    let dist = STEP * count as f64;
    format!("window.scrollBy({}, {});", ux * dist, uy * dist)
}

/// Script that resolves the page's next or previous link to an absolute URL,
/// preferring a declared `rel` link and falling back to common link labels.
/// Evaluates to the URL string, or an empty string when none is found.
fn rel_link_script(next: bool) -> String {
    let (rel, labels) = if next {
        ("next", r#"["next","forward","older"]"#)
    } else {
        ("prev", r#"["prev","previous","back","newer"]"#)
    };
    format!(
        r#"(function(){{
  var rel={rel}, labels={labels};
  var el=document.querySelector('link[rel~="'+rel+'"], a[rel~="'+rel+'"]');
  if(el && el.href) return el.href;
  var anchors=document.querySelectorAll('a[href]');
  for(var i=0;i<anchors.length;i++){{
    var a=anchors[i];
    var t=(a.textContent||'').trim().toLowerCase();
    var aria=(a.getAttribute('aria-label')||'').trim().toLowerCase();
    for(var j=0;j<labels.length;j++){{
      if(t===labels[j]||aria===labels[j]) return a.href;
    }}
  }}
  return '';
}})()"#,
        rel = js_arg(rel),
    )
}

/// Script that focuses the `n`th (1-based) visible text-like input on the page.
/// Fire-and-forget; a no-op when no such input exists.
fn focus_input_script(n: u32) -> String {
    let index = n.max(1) - 1;
    format!(
        r#"(function(){{
  var sel='input[type=text],input[type=search],input[type=email],input[type=url],input[type=tel],input[type=password],input:not([type]),textarea,[contenteditable=true],[contenteditable=""]';
  var els=Array.prototype.filter.call(document.querySelectorAll(sel), function(e){{
    return e.offsetParent!==null || e.getClientRects().length>0;
  }});
  var el=els[{index}];
  if(el) el.focus();
}})()"#
    )
}

/// Split a full URL into its `scheme://authority` prefix and the remainder
/// (path, query, and fragment). Returns `None` when there is no `://` authority.
fn split_authority(url: &str) -> Option<(&str, &str)> {
    let after = url.find("://")? + 3;
    let rest_start = url[after..]
        .find(['/', '?', '#'])
        .map(|i| after + i)
        .unwrap_or(url.len());
    Some((&url[..rest_start], &url[rest_start..]))
}

/// Navigate `url` up `segments` path segments, dropping the query and fragment.
/// Returns the host root when already at or above the path root. `None` when the
/// URL has no `://` authority to anchor on.
fn url_path_up(url: &str, segments: u32) -> Option<String> {
    let (root, rest) = split_authority(url)?;
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for _ in 0..segments.max(1) {
        if segs.pop().is_none() {
            break;
        }
    }
    let mut out = String::from(root);
    for s in &segs {
        out.push('/');
        out.push_str(s);
    }
    Some(out)
}

/// The host root (`scheme://authority`) of `url`. `None` when the URL has no
/// `://` authority or is already at the root with nothing to drop.
fn url_host_root(url: &str) -> Option<String> {
    let (root, rest) = split_authority(url)?;
    if rest.is_empty() || rest == "/" {
        None
    } else {
        Some(root.to_string())
    }
}

/// Step the last run of ASCII digits in `url` by `step`, clamping at zero and
/// preserving the original zero-padding width. `None` when the URL has no digits.
fn url_step_number(url: &str, step: i32) -> Option<String> {
    let bytes = url.as_bytes();
    let end = url.rfind(|c: char| c.is_ascii_digit())? + 1;
    let mut start = end;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let width = end - start;
    let value: i64 = url[start..end].parse().ok()?;
    let next = (value + step as i64).max(0);
    Some(format!("{}{next:0width$}{}", &url[..start], &url[end..]))
}

/// Normalize command-line input into a navigable URI.
///
/// Explicit schemes and `about:` targets pass through; a single whitespace-free
/// dotted token becomes `https://`. Otherwise the input is a search: a leading
/// token that names a configured engine and is followed by a query selects that
/// engine, and anything else goes to the default engine.
pub(crate) fn normalize_target(input: &str, search: &SearchEngines) -> String {
    let t = input.trim();
    if t.is_empty() {
        return "about:blank".to_string();
    }
    if t.contains("://") || t.starts_with("about:") {
        return t.to_string();
    }
    if !t.contains(char::is_whitespace) && t.contains('.') {
        return format!("https://{t}");
    }
    if let Some((first, rest)) = t.split_once(char::is_whitespace) {
        let rest = rest.trim();
        if !rest.is_empty()
            && let Some(template) = search.engines.get(first)
        {
            return template.replace("{}", &encode_query(rest));
        }
    }
    search.default_template().replace("{}", &encode_query(t))
}

/// Percent-encode a search query for use in a URL, mapping spaces to `+` and
/// leaving the URL-unreserved characters intact.
fn encode_query(query: &str) -> String {
    let mut out = String::new();
    for b in query.bytes() {
        match b {
            b' ' => out.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse a bare command string and either run it or report the error.
fn push_parsed(state: &mut State, command: &str, effects: &mut Vec<Effect>) {
    match Command::parse(command) {
        Ok(inner) => effects.extend(handle_command(state, inner)),
        Err(text) => effects.push(Effect::ShowMessage {
            level: MessageLevel::Error,
            text,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::command::{ClipboardSource, Command, HintTarget, OpenTarget, ScrollDir};
    use crate::core::key::Key;
    use crate::core::msg::{JsPurpose, LoadEvent, RequestId};
    use crate::core::state::{Config, Layout, Mode, State};

    fn pending_id(state: &State, want: JsPurpose) -> RequestId {
        *state
            .pending_js
            .iter()
            .find(|(_, p)| **p == want)
            .map(|(id, _)| id)
            .expect("a pending JS request of the wanted purpose")
    }

    fn state_with_tab() -> State {
        let mut state = State::new(Config::default());
        let id = state.tabs.open("https://example.com");
        state.tabs.focus_id(id);
        // A single-pane layout mirrors startup so tab commands exercise the
        // real focus_tab path.
        state.layout = Layout::new(id);
        assert_eq!(state.tabs.active_id(), Some(id));
        state
    }

    fn press(state: &mut State, sym: &str) -> Vec<Effect> {
        update(state, Msg::Key(Key::plain(sym)))
    }

    fn search_with(default: &str, engines: &[(&str, &str)]) -> SearchEngines {
        let mut s = SearchEngines::default();
        s.default = default.to_string();
        s.engines = engines
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s
    }

    #[test]
    fn normalize_passes_through_urls() {
        let s = SearchEngines::default();
        assert_eq!(normalize_target("https://x.test", &s), "https://x.test");
        assert_eq!(normalize_target("example.com", &s), "https://example.com");
        assert_eq!(normalize_target("about:blank", &s), "about:blank");
        assert_eq!(normalize_target("", &s), "about:blank");
    }

    #[test]
    fn normalize_uses_default_engine_for_bare_query() {
        let s = search_with("ddg", &[("ddg", "https://ddg.test/?q={}")]);
        assert_eq!(
            normalize_target("hello world", &s),
            "https://ddg.test/?q=hello+world"
        );
    }

    #[test]
    fn normalize_selects_engine_by_leading_keyword() {
        let s = search_with(
            "ddg",
            &[
                ("ddg", "https://ddg.test/?q={}"),
                ("gh", "https://github.com/search?q={}"),
            ],
        );
        assert_eq!(
            normalize_target("gh rust serde", &s),
            "https://github.com/search?q=rust+serde"
        );
        // A lone keyword with no query is itself a default-engine search.
        assert_eq!(normalize_target("gh", &s), "https://ddg.test/?q=gh");
        // A leading token that is not an engine is part of the query.
        assert_eq!(
            normalize_target("nope here", &s),
            "https://ddg.test/?q=nope+here"
        );
    }

    #[test]
    fn normalize_falls_back_when_default_missing() {
        let s = search_with("gone", &[("gh", "https://github.com/search?q={}")]);
        assert_eq!(normalize_target("a b", &s), "https://duckduckgo.com/?q=a+b");
    }

    #[test]
    fn normalize_encodes_unsafe_query_characters() {
        let s = search_with("ddg", &[("ddg", "https://ddg.test/?q={}")]);
        assert_eq!(normalize_target("a&b c", &s), "https://ddg.test/?q=a%26b+c");
    }

    #[test]
    fn open_empty_input_uses_newtab_page() {
        let mut state = state_with_tab();
        state.config.newtab = "https://start.test".to_string();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: String::new(),
            }),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "https://start.test"
        )));
    }

    #[test]
    fn open_empty_input_defaults_to_about_blank() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: String::new(),
            }),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "about:blank"
        )));
    }

    #[test]
    fn js_toggle_records_rule_and_reloads_site_tabs() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::SiteJavascript(JsToggle::Toggle)),
        );
        // The active tab is on example.com; toggling from the default (enabled)
        // disables it and records the rule.
        assert!(!state.site_prefs.javascript_for("example.com"));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetJavascript { enabled: false, .. }))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SaveSitePrefs(_)))
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::Reload { .. })));
    }

    #[test]
    fn set_javascript_rule_updates_store_and_persists() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::Set {
                key: "javascript.example.com".to_string(),
                value: "false".to_string(),
            }),
        );
        assert!(!state.site_prefs.javascript_for("example.com"));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SaveSitePrefs(_)))
        );
        // The global default is settable too.
        update(
            &mut state,
            Msg::Command(Command::Set {
                key: "javascript.default".to_string(),
                value: "false".to_string(),
            }),
        );
        assert!(!state.site_prefs.javascript_default);
    }

    #[test]
    fn clear_category_emits_effect_without_host() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::ClearData(ClearScope::Cookies)),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ClearData {
                scope: ClearScope::Cookies,
                host: None
            }
        )));
    }

    #[test]
    fn clear_site_uses_active_host() {
        let mut state = state_with_tab(); // active tab is https://example.com
        let effects = update(
            &mut state,
            Msg::Command(Command::ClearData(ClearScope::Site)),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ClearData { scope: ClearScope::Site, host: Some(h) } if h == "example.com"
        )));
    }

    #[test]
    fn clear_site_without_a_site_errors() {
        let mut state = state_with_tab();
        let id = state.tabs.active_id().unwrap();
        state.tabs.get_mut(id).unwrap().url = "about:blank".to_string();
        let effects = update(
            &mut state,
            Msg::Command(Command::ClearData(ClearScope::Site)),
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ClearData { .. }))
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn tabs_toggle_flips_flag_and_sets_width() {
        let mut state = state_with_tab();
        state.config.tabs.width = 220;
        assert!(!state.tabs_collapsed);
        let effects = update(&mut state, Msg::Command(Command::TabsToggle));
        assert!(state.tabs_collapsed);
        assert!(effects.iter().any(|e| matches!(e, Effect::RenderTabs)));
        // Collapsing sets the narrow rail width.
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::SetTabWidth(w) if *w == crate::core::state::TABS_COLLAPSED_WIDTH
        )));
        // Toggling back restores the configured width.
        let effects = update(&mut state, Msg::Command(Command::TabsToggle));
        assert!(!state.tabs_collapsed);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SetTabWidth(220)))
        );
    }

    #[test]
    fn fullscreen_toggles_flag_and_emits_effect() {
        let mut state = state_with_tab();
        assert!(!state.fullscreen);
        let effects = update(&mut state, Msg::Command(Command::Fullscreen));
        assert!(state.fullscreen);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ToggleFullscreen { fullscreen: true }))
        );
        // Toggling again returns to the original state.
        let effects = update(&mut state, Msg::Command(Command::Fullscreen));
        assert!(!state.fullscreen);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ToggleFullscreen { fullscreen: false }))
        );
    }

    // --- URL motion helpers (pure) ---

    #[test]
    fn path_up_walks_segments() {
        assert_eq!(
            url_path_up("https://example.com/a/b/c", 1).as_deref(),
            Some("https://example.com/a/b")
        );
        assert_eq!(
            url_path_up("https://example.com/a/b/c", 2).as_deref(),
            Some("https://example.com/a")
        );
        // A trailing slash is trimmed before stepping.
        assert_eq!(
            url_path_up("https://example.com/a/b/", 1).as_deref(),
            Some("https://example.com/a")
        );
        // Query and fragment are dropped.
        assert_eq!(
            url_path_up("https://example.com/a/b?q=1#x", 1).as_deref(),
            Some("https://example.com/a")
        );
        // At the root, walking up stays at the host root.
        assert_eq!(
            url_path_up("https://example.com/", 1).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn host_root_strips_path_query_fragment() {
        assert_eq!(
            url_host_root("https://example.com/a/b/c?q=1#x").as_deref(),
            Some("https://example.com")
        );
        // Already at the root: nothing to do.
        assert_eq!(url_host_root("https://example.com"), None);
        assert_eq!(url_host_root("https://example.com/"), None);
    }

    #[test]
    fn step_number_increments_decrements_and_clamps() {
        assert_eq!(
            url_step_number("https://example.com/page/7", 1).as_deref(),
            Some("https://example.com/page/8")
        );
        assert_eq!(
            url_step_number("https://example.com/page/7", -1).as_deref(),
            Some("https://example.com/page/6")
        );
        assert_eq!(
            url_step_number("https://example.com/page/7", 5).as_deref(),
            Some("https://example.com/page/12")
        );
        // Decrement clamps at zero.
        assert_eq!(
            url_step_number("https://example.com/page/0", -1).as_deref(),
            Some("https://example.com/page/0")
        );
        // Zero padding width is preserved.
        assert_eq!(
            url_step_number("https://example.com/p/008", 1).as_deref(),
            Some("https://example.com/p/009")
        );
        // No digits: nothing to do.
        assert_eq!(url_step_number("https://example.com/about", 1), None);
    }

    // --- URL motion commands ---

    fn state_at(url: &str) -> State {
        let mut state = state_with_tab();
        let id = state.tabs.active_id().unwrap();
        state.tabs.get_mut(id).unwrap().url = url.to_string();
        state
    }

    fn loaded_uri(effects: &[Effect]) -> Option<&str> {
        effects.iter().find_map(|e| match e {
            Effect::LoadUri { uri, .. } => Some(uri.as_str()),
            _ => None,
        })
    }

    #[test]
    fn url_up_and_root_navigate() {
        let mut state = state_at("https://example.com/a/b/c");
        let effects = update(&mut state, Msg::Command(Command::UrlUp(1)));
        assert_eq!(loaded_uri(&effects), Some("https://example.com/a/b"));

        let mut state = state_at("https://example.com/a/b/c?q=1#x");
        let effects = update(&mut state, Msg::Command(Command::UrlRoot));
        assert_eq!(loaded_uri(&effects), Some("https://example.com"));
    }

    #[test]
    fn url_increment_honors_count_and_emits_nothing_without_a_number() {
        let mut state = state_at("https://example.com/page/7");
        let effects = update(&mut state, Msg::Command(Command::UrlIncrement(1)));
        assert_eq!(loaded_uri(&effects), Some("https://example.com/page/8"));

        // `5<C-a>`: count sets the step magnitude, keeping the sign.
        let cmd = Command::UrlIncrement(1).with_count(5);
        let effects = update(
            &mut state_at("https://example.com/page/7"),
            Msg::Command(cmd),
        );
        assert_eq!(loaded_uri(&effects), Some("https://example.com/page/12"));

        let cmd = Command::UrlIncrement(-1).with_count(3);
        let effects = update(
            &mut state_at("https://example.com/page/7"),
            Msg::Command(cmd),
        );
        assert_eq!(loaded_uri(&effects), Some("https://example.com/page/4"));

        let mut state = state_at("https://example.com/about");
        let effects = update(&mut state, Msg::Command(Command::UrlIncrement(1)));
        assert_eq!(loaded_uri(&effects), None);
    }

    #[test]
    fn page_next_evals_js_and_result_opens_or_blocks() {
        let mut state = state_at("https://example.com/1");
        let effects = update(&mut state, Msg::Command(Command::PageNext));
        let (id, tab) = effects
            .iter()
            .find_map(|e| match e {
                Effect::EvalJs {
                    id,
                    tab,
                    purpose: JsPurpose::PageRelLink,
                    ..
                } => Some((*id, *tab)),
                _ => None,
            })
            .expect("PageNext should evaluate JS for the rel link");
        assert_eq!(state.pending_js.get(&id), Some(&JsPurpose::PageRelLink));

        // A safe resolved URL opens in the current tab.
        let effects = update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("https://example.com/2".to_string()),
            },
        );
        assert_eq!(loaded_uri(&effects), Some("https://example.com/2"));

        // An unsafe scheme is blocked, not navigated.
        let mut state = state_at("https://example.com/1");
        let effects = update(&mut state, Msg::Command(Command::PagePrev));
        let (id, tab) = effects
            .iter()
            .find_map(|e| match e {
                Effect::EvalJs { id, tab, .. } => Some((*id, *tab)),
                _ => None,
            })
            .unwrap();
        let effects = update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("file:///etc/passwd".to_string()),
            },
        );
        assert_eq!(loaded_uri(&effects), None);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn focus_input_evals_js_and_enters_insert() {
        let mut state = state_at("https://example.com");
        let effects = update(&mut state, Msg::Command(Command::FocusInput(1)));
        assert!(effects.iter().any(|e| matches!(e, Effect::EvalJs { .. })));
        assert_eq!(state.mode.current, Mode::Insert);
    }

    // --- Scroll memory ---

    #[test]
    fn back_and_forward_capture_offset_and_mark_restore() {
        let mut state = state_at("https://example.com/article");
        let tab = state.tabs.active_id().unwrap();
        let effects = update(&mut state, Msg::Command(Command::Back(1)));
        // A read-offset evaluation is emitted alongside the back navigation.
        let captured_url = effects.iter().find_map(|e| match e {
            Effect::EvalJs {
                purpose: JsPurpose::ReadScrollOffset { url },
                ..
            } => Some(url.clone()),
            _ => None,
        });
        assert_eq!(captured_url.as_deref(), Some("https://example.com/article"));
        assert!(effects.iter().any(|e| matches!(e, Effect::GoBack { .. })));
        assert!(state.tabs.get(tab).unwrap().restore_scroll);

        // Forward behaves the same.
        let mut state = state_at("https://example.com/article");
        let effects = update(&mut state, Msg::Command(Command::Forward(1)));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs {
                purpose: JsPurpose::ReadScrollOffset { .. },
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::GoForward { .. }))
        );
    }

    #[test]
    fn offset_result_is_recorded_against_captured_url() {
        let mut state = state_at("https://example.com/a");
        let tab = state.tabs.active_id().unwrap();
        let id = state.alloc_request_id();
        state.pending_js.insert(
            id,
            JsPurpose::ReadScrollOffset {
                url: "https://example.com/a".to_string(),
            },
        );
        // The tab has since navigated elsewhere; the offset must key off the
        // captured URL, not the current one.
        state.tabs.get_mut(tab).unwrap().url = "https://example.com/b".to_string();
        update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("0 640".to_string()),
            },
        );
        let store = &state.tabs.get(tab).unwrap().scroll_offsets;
        assert_eq!(store.get("https://example.com/a"), Some((0, 640)));
        assert_eq!(store.get("https://example.com/b"), None);
    }

    #[test]
    fn load_finished_restores_after_back_only() {
        let mut state = state_at("https://example.com/a");
        let tab = state.tabs.active_id().unwrap();
        {
            let t = state.tabs.get_mut(tab).unwrap();
            t.scroll_offsets.record("https://example.com/a", (0, 500));
            t.restore_scroll = true;
        }
        let effects = update(
            &mut state,
            Msg::Load {
                tab,
                event: LoadEvent::Finished,
            },
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::EvalJs { .. })));
        // The awaiting flag is cleared after restoring.
        assert!(!state.tabs.get(tab).unwrap().restore_scroll);

        // A plain reload (no back/forward, no awaiting flag) restores nothing.
        let effects = update(
            &mut state,
            Msg::Load {
                tab,
                event: LoadEvent::Finished,
            },
        );
        assert!(!effects.iter().any(|e| matches!(e, Effect::EvalJs { .. })));
    }

    #[test]
    fn scroll_store_evicts_least_recently_used() {
        let mut store = crate::core::state::ScrollStore::default();
        for i in 0..60 {
            store.record(&format!("https://example.com/{i}"), (0, i as i64));
        }
        // Bound is 50: the 10 earliest entries (0..=9) are evicted, the rest kept.
        assert_eq!(store.get("https://example.com/9"), None);
        assert_eq!(store.get("https://example.com/10"), Some((0, 10)));
        assert_eq!(store.get("https://example.com/59"), Some((0, 59)));
    }

    #[test]
    fn autosave_carries_active_tab_offset() {
        let mut state = state_at("https://example.com/a");
        let tab = state.tabs.active_id().unwrap();
        state
            .tabs
            .get_mut(tab)
            .unwrap()
            .scroll_offsets
            .record("https://example.com/a", (0, 720));
        let effects = update(&mut state, Msg::Command(Command::Quit));
        let scroll = effects.iter().find_map(|e| match e {
            Effect::SaveAutosave { scroll, .. } => Some(*scroll),
            _ => None,
        });
        assert_eq!(scroll, Some(Some((0, 720))));
    }

    // --- Page tools ---

    #[test]
    fn view_source_reads_page_then_renders_html() {
        // The command emits a JS read tagged ViewSource for the active tab.
        let mut state = state_at("https://example.com");
        let tab = state.tabs.active_id().unwrap();
        let effects = update(&mut state, Msg::Command(Command::ViewSource { tab: false }));
        let id = effects
            .iter()
            .find_map(|e| match e {
                Effect::EvalJs {
                    id,
                    purpose: JsPurpose::ViewSource { new_tab: false },
                    ..
                } => Some(*id),
                _ => None,
            })
            .expect("view-source emits a ViewSource read");

        // The returned document is rendered in the active tab via LoadHtml.
        let effects = update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("<html>source</html>".to_string()),
            },
        );
        assert!(effects.contains(&Effect::LoadHtml {
            tab,
            html: "<html>source</html>".to_string()
        }));
    }

    #[test]
    fn view_source_tab_opens_new_tab_then_renders() {
        let mut state = state_at("https://example.com");
        let tab = state.tabs.active_id().unwrap();
        let before = state.tabs.iter().count();
        let effects = update(&mut state, Msg::Command(Command::ViewSource { tab: true }));
        let id = pending_id(&state, JsPurpose::ViewSource { new_tab: true });
        assert!(effects.iter().any(|e| matches!(e, Effect::EvalJs { .. })));

        let effects = update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("<html>src</html>".to_string()),
            },
        );
        assert_eq!(state.tabs.iter().count(), before + 1);
        let new_id = state.tabs.active_id().unwrap();
        assert_ne!(new_id, tab);
        assert!(effects.contains(&Effect::LoadHtml {
            tab: new_id,
            html: "<html>src</html>".to_string()
        }));
    }

    #[test]
    fn view_source_with_no_page_is_a_notice() {
        let mut state = state_at("about:blank");
        let effects = update(&mut state, Msg::Command(Command::ViewSource { tab: false }));
        assert!(!effects.iter().any(|e| matches!(e, Effect::EvalJs { .. })));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Info,
                ..
            }
        )));
    }

    #[test]
    fn print_save_inspect_target_the_active_tab() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        assert_eq!(
            update(&mut state, Msg::Command(Command::Print)),
            vec![Effect::Print { tab }]
        );
        assert_eq!(
            update(&mut state, Msg::Command(Command::SavePage)),
            vec![Effect::SavePage { tab }]
        );
        assert_eq!(
            update(&mut state, Msg::Command(Command::Inspect)),
            vec![Effect::ToggleInspector { tab }]
        );
    }

    // --- Tab picker ---

    fn state_with_three_tabs() -> State {
        let mut state = state_with_tab(); // tab 0: https://example.com
        let id1 = state.tabs.open("https://docs.rs/foo");
        let id2 = state.tabs.open("https://github.com/bar");
        state.tabs.get_mut(id1).unwrap().title = "Foo Docs".to_string();
        state.tabs.get_mut(id2).unwrap().title = "GitHub Bar".to_string();
        state
    }

    #[test]
    fn buffer_command_enters_mode_and_seeds_filter() {
        let mut state = state_with_three_tabs();
        update(&mut state, Msg::Command(Command::Buffer(None)));
        assert_eq!(state.mode.current, Mode::Buffer);
        assert_eq!(state.buffer_view.filter, "");

        let mut state = state_with_three_tabs();
        update(
            &mut state,
            Msg::Command(Command::Buffer(Some("docs".to_string()))),
        );
        assert_eq!(state.mode.current, Mode::Buffer);
        assert_eq!(state.buffer_view.filter, "docs");
    }

    #[test]
    fn buffer_filter_matches_title_and_url_case_insensitively() {
        let state = state_with_three_tabs();
        // Empty query lists every tab.
        assert_eq!(filtered_tab_indices(&state.tabs, "").len(), 3);
        // Title match, ignoring case.
        assert_eq!(filtered_tab_indices(&state.tabs, "GITHUB").len(), 1);
        // URL match.
        assert_eq!(filtered_tab_indices(&state.tabs, "docs.rs").len(), 1);
        // No match.
        assert_eq!(filtered_tab_indices(&state.tabs, "zzz").len(), 0);
    }

    #[test]
    fn buffer_enter_switches_by_id_and_esc_leaves_unchanged() {
        let mut state = state_with_three_tabs();
        let ids: Vec<_> = state.tabs.iter().map(|t| t.id).collect();
        update(&mut state, Msg::Command(Command::Buffer(None)));
        // Move to the second row and switch.
        update(&mut state, Msg::Key(Key::plain("j")));
        update(&mut state, Msg::Key(Key::plain("Return")));
        assert_eq!(state.mode.current, Mode::Normal);
        assert_eq!(state.tabs.active_id(), Some(ids[1]));

        // Esc leaves without changing the active tab.
        let mut state = state_with_three_tabs();
        let before = state.tabs.active_id();
        update(&mut state, Msg::Command(Command::Buffer(None)));
        update(&mut state, Msg::Key(Key::plain("j")));
        update(&mut state, Msg::Key(Key::plain("Escape")));
        assert_eq!(state.mode.current, Mode::Normal);
        assert_eq!(state.tabs.active_id(), before);
    }

    // --- Tab pinning ---

    #[test]
    fn pin_toggle_sets_flag_and_sorts_to_top() {
        let mut state = state_with_three_tabs(); // tab0 active, tabs 1,2 below
        let ids: Vec<_> = state.tabs.iter().map(|t| t.id).collect();
        // Focus the last tab and pin it.
        state.tabs.focus_id(ids[2]);
        update(&mut state, Msg::Command(Command::PinToggle));
        assert!(state.tabs.get(ids[2]).unwrap().pinned);
        // It sorts to the front and stays focused.
        assert_eq!(state.tabs.iter().next().map(|t| t.id), Some(ids[2]));
        assert_eq!(state.tabs.active_id(), Some(ids[2]));
        // Toggling again unpins.
        update(&mut state, Msg::Command(Command::PinToggle));
        assert!(!state.tabs.get(ids[2]).unwrap().pinned);
    }

    #[test]
    fn close_is_refused_on_a_pinned_tab() {
        let mut state = state_with_three_tabs();
        let before = state.tabs.iter().count();
        update(&mut state, Msg::Command(Command::Pin));
        let effects = update(&mut state, Msg::Command(Command::TabClose));
        // The tab is not closed and a notice is shown.
        assert_eq!(state.tabs.iter().count(), before);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Info,
                ..
            }
        )));
        // After unpinning, close works.
        update(&mut state, Msg::Command(Command::Unpin));
        update(&mut state, Msg::Command(Command::TabClose));
        assert_eq!(state.tabs.iter().count(), before - 1);
    }

    #[test]
    fn tab_only_keeps_pinned_tabs() {
        let mut state = state_with_three_tabs();
        let ids: Vec<_> = state.tabs.iter().map(|t| t.id).collect();
        // Pin the last tab, then keep the first active and close others.
        state.tabs.focus_id(ids[2]);
        update(&mut state, Msg::Command(Command::Pin));
        state.tabs.focus_id(ids[0]);
        update(&mut state, Msg::Command(Command::TabOnly));
        // The active tab and the pinned tab both survive.
        let remaining: Vec<_> = state.tabs.iter().map(|t| t.id).collect();
        assert!(remaining.contains(&ids[0]));
        assert!(remaining.contains(&ids[2]));
        assert!(!remaining.contains(&ids[1]));
    }

    #[test]
    fn clone_and_undo_preserve_pinned() {
        let mut state = state_with_three_tabs();
        update(&mut state, Msg::Command(Command::Pin));
        // Clone the pinned active tab: the clone is pinned too.
        update(&mut state, Msg::Command(Command::TabClone));
        assert!(state.tabs.active().unwrap().pinned);
        // Close a pinned tab via undo path: unpin, close, undo restores pinned.
        update(&mut state, Msg::Command(Command::Unpin));
        update(&mut state, Msg::Command(Command::TabClone)); // active now unpinned clone
        // Pin then unpin-close-undo to check ClosedTab carries the flag.
        update(&mut state, Msg::Command(Command::Pin));
        let pinned_url = state.tabs.active().unwrap().url.clone();
        update(&mut state, Msg::Command(Command::Unpin));
        update(&mut state, Msg::Command(Command::TabClose));
        update(&mut state, Msg::Command(Command::Undo));
        // The undo helper does not re-pin once unpinned; this asserts ClosedTab
        // round-trips the flag it had when closed (unpinned here).
        assert_eq!(state.tabs.active().unwrap().url, pinned_url);
    }

    #[test]
    fn autosave_and_session_round_trip_pinned() {
        let mut state = state_with_three_tabs();
        update(&mut state, Msg::Command(Command::Pin)); // pin active (tab0)
        // Autosave carries the pinned flag.
        let effects = update(&mut state, Msg::Command(Command::Quit));
        let tabs = effects
            .iter()
            .find_map(|e| match e {
                Effect::SaveAutosave { tabs, .. } => Some(tabs.clone()),
                _ => None,
            })
            .unwrap();
        assert!(tabs.iter().any(|(_, pinned)| *pinned));

        // Named session save carries it too.
        let mut state = state_with_three_tabs();
        update(&mut state, Msg::Command(Command::Pin));
        let effects = update(
            &mut state,
            Msg::Command(Command::SessionSave("s".to_string())),
        );
        let saved = effects.iter().find_map(|e| match e {
            Effect::SaveSession { tabs, .. } => Some(tabs.clone()),
            _ => None,
        });
        assert!(saved.unwrap().iter().any(|(_, pinned)| *pinned));

        // SessionLoaded re-pins and sorts pinned first.
        let mut state = State::new(Config::default());
        let effects = update(
            &mut state,
            Msg::SessionLoaded(vec![
                ("https://a.test".to_string(), false),
                ("https://b.test".to_string(), true),
            ]),
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::RenderTabs)));
        assert!(state.tabs.iter().next().unwrap().pinned);
    }

    // --- Clipboard open ---

    #[test]
    fn clipboard_open_emits_read_for_each_source_and_placement() {
        let cases = [
            (ClipboardSource::Primary, OpenTarget::Current),
            (ClipboardSource::Primary, OpenTarget::Tab),
            (ClipboardSource::Clipboard, OpenTarget::Current),
            (ClipboardSource::Clipboard, OpenTarget::Tab),
        ];
        for (source, target) in cases {
            let mut state = state_with_tab();
            let effects = update(
                &mut state,
                Msg::Command(Command::ClipboardOpen { source, target }),
            );
            assert_eq!(
                effects,
                vec![Effect::ReadClipboard { source, target }],
                "{source:?}/{target:?}"
            );
        }
    }

    #[test]
    fn clipboard_read_opens_url_and_search_in_both_placements() {
        // A URL in the current tab loads.
        let mut state = state_at("https://example.com");
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::ClipboardRead {
                text: "  example.org  ".to_string(),
                source: ClipboardSource::Primary,
                target: OpenTarget::Current,
            },
        );
        assert!(effects.contains(&Effect::LoadUri {
            tab,
            uri: "https://example.org".to_string()
        }));

        // Non-URL text in a new tab becomes a search and opens a tab.
        let mut state = state_with_tab();
        let before = state.tabs.iter().count();
        let effects = update(
            &mut state,
            Msg::ClipboardRead {
                text: "rust lifetimes".to_string(),
                source: ClipboardSource::Clipboard,
                target: OpenTarget::Tab,
            },
        );
        assert_eq!(state.tabs.iter().count(), before + 1);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri.contains("duckduckgo.com") && uri.contains("rust")
        )));
    }

    #[test]
    fn clipboard_read_empty_shows_notice_and_opens_nothing() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::ClipboardRead {
                text: "   ".to_string(),
                source: ClipboardSource::Clipboard,
                target: OpenTarget::Current,
            },
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::LoadUri { .. } | Effect::OpenTab { .. }))
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Info,
                ..
            }
        )));
    }

    #[test]
    fn passthrough_mode_enter_leave_and_key_noop() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::ModeEnter(Mode::Passthrough)),
        );
        assert_eq!(state.mode.current, Mode::Passthrough);

        // An ordinary key in passthrough fires no Normal-mode binding and does
        // nothing in the core.
        let effects = update(&mut state, Msg::Key(Key::plain("j")));
        assert!(effects.is_empty());
        assert_eq!(state.mode.current, Mode::Passthrough);

        // Escape leaves passthrough back to Normal.
        update(&mut state, Msg::Command(Command::ModeLeave));
        assert_eq!(state.mode.current, Mode::Normal);
    }

    #[test]
    fn private_open_sets_flag_and_effect() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::PrivateTab,
                input: "example.org".to_string(),
            }),
        );
        assert!(state.tabs.active().unwrap().private);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenTab { private: true, .. }))
        );
    }

    #[test]
    fn private_tab_records_no_history() {
        let mut state = state_with_tab();
        // The initial (normal) tab records history on load.
        let normal = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::Load {
                tab: normal,
                event: LoadEvent::Finished,
            },
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RecordHistory { .. }))
        );
        // A private tab does not.
        update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::PrivateTab,
                input: "secret.test".to_string(),
            }),
        );
        let private = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::Load {
                tab: private,
                event: LoadEvent::Finished,
            },
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RecordHistory { .. }))
        );
    }

    #[test]
    fn autosave_and_clone_undo_respect_privacy() {
        let mut state = state_with_tab(); // one normal tab: https://example.com
        update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::PrivateTab,
                input: "secret.test".to_string(),
            }),
        );
        // Cloning a private tab yields a private tab.
        let effects = update(&mut state, Msg::Command(Command::TabClone));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenTab { private: true, .. }))
        );
        // Quit autosaves only the non-private tab.
        let effects = update(&mut state, Msg::Command(Command::Quit));
        let urls = effects
            .iter()
            .find_map(|e| match e {
                Effect::SaveAutosave { tabs, .. } => Some(tabs.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(urls, vec![("https://example.com".to_string(), false)]);
    }

    #[test]
    fn undo_reopens_private_tab_as_private() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::PrivateTab,
                input: "x.test".to_string(),
            }),
        );
        update(&mut state, Msg::Command(Command::TabClose));
        let effects = update(&mut state, Msg::Command(Command::Undo));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenTab { private: true, .. }))
        );
    }

    #[test]
    fn bind_and_unbind_update_live_bindings() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::Bind {
                keys: "gh".to_string(),
                command: "scroll-to-perc 0".to_string(),
            }),
        );
        assert_eq!(
            state.bindings.lookup(&parse_key_string("gh")),
            TrieMatch::Exact("scroll-to-perc 0".to_string())
        );
        update(&mut state, Msg::Command(Command::Unbind("gg".to_string())));
        assert_eq!(
            state.bindings.lookup(&parse_key_string("gg")),
            TrieMatch::None
        );
    }

    fn request_permission(state: &mut State, id: u64, host: &str) -> Vec<Effect> {
        update(
            state,
            Msg::PermissionRequested {
                id,
                host: host.to_string(),
                capability: crate::core::state::Capability::Camera,
            },
        )
    }

    #[test]
    fn slash_search_sets_last_search_and_emits_find() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine("/hello".into())),
        );
        let effects = update(&mut state, Msg::Command(Command::Accept));
        assert!(matches!(&state.last_search, Some(s) if s.text == "hello"));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Find { text, .. } if text == "hello"
        )));
    }

    #[test]
    fn find_result_sets_total_without_clearing_line() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine("/foo".into())),
        );
        update(&mut state, Msg::Command(Command::Accept));
        let tab = state.tabs.active_id().unwrap();
        let e = update(&mut state, Msg::FindResult { tab, matches: 3 });
        assert_eq!(
            state.status.search.as_ref().map(SearchStatus::label),
            Some("3 matches".to_string())
        );
        assert!(e.iter().any(|x| matches!(x, Effect::RenderStatus)));
        assert!(!e.iter().any(|x| matches!(x, Effect::ShowMessage { .. })));

        update(
            &mut state,
            Msg::Command(Command::SetCommandLine("/".into())),
        );
        update(&mut state, Msg::Command(Command::Accept));
        assert_eq!(state.status.search, None);
    }

    #[test]
    fn find_result_zero_reports_no_matches() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        update(&mut state, Msg::FindResult { tab, matches: 0 });
        assert_eq!(
            state.status.search.as_ref().map(SearchStatus::label),
            Some("no matches".to_string())
        );
    }

    #[test]
    fn find_repeat_requires_active_search() {
        let mut state = state_with_tab();
        let e = update(&mut state, Msg::Command(Command::FindNext));
        assert!(!e.iter().any(|x| matches!(x, Effect::FindNext { .. })));
        assert!(e.iter().any(|x| matches!(x, Effect::ShowMessage { .. })));

        state.last_search = Some(Search { text: "x".into() });
        let e = update(&mut state, Msg::Command(Command::FindNext));
        assert!(e.iter().any(|x| matches!(x, Effect::FindNext { .. })));
        let e = update(&mut state, Msg::Command(Command::FindPrev));
        assert!(e.iter().any(|x| matches!(x, Effect::FindPrev { .. })));
    }

    #[test]
    fn zoom_in_reset_and_clamp() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::ZoomIn));
        assert!((state.tabs.active().unwrap().zoom - 1.1).abs() < 1e-9);
        update(&mut state, Msg::Command(Command::ZoomReset));
        assert!((state.tabs.active().unwrap().zoom - 1.0).abs() < 1e-9);
        // Out-of-range set is clamped to the bounds.
        update(&mut state, Msg::Command(Command::ZoomSet(1000)));
        assert!((state.tabs.active().unwrap().zoom - ZOOM_MAX).abs() < 1e-9);
        update(&mut state, Msg::Command(Command::ZoomSet(1)));
        assert!((state.tabs.active().unwrap().zoom - ZOOM_MIN).abs() < 1e-9);
    }

    #[test]
    fn permission_request_enters_prompt_mode() {
        let mut state = state_with_tab();
        request_permission(&mut state, 1, "example.com");
        assert_eq!(state.mode.current, Mode::Prompt);
        assert_eq!(state.prompts.len(), 1);
    }

    #[test]
    fn allow_once_grants_without_persisting() {
        let mut state = state_with_tab();
        request_permission(&mut state, 7, "example.com");
        let effects = press(&mut state, "y");
        assert!(effects.contains(&Effect::ResolvePermission { id: 7, allow: true }));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SavePermissions(_)))
        );
        assert_eq!(state.mode.current, Mode::Normal);
        // Nothing persisted: the site still resolves to the default (ask).
        assert_eq!(
            state
                .config
                .permissions
                .policy_for("example.com", crate::core::state::Capability::Camera),
            PermissionPolicy::Ask
        );
    }

    #[test]
    fn always_allow_persists_rule() {
        let mut state = state_with_tab();
        request_permission(&mut state, 3, "example.com");
        let effects = press(&mut state, "a");
        assert!(effects.contains(&Effect::ResolvePermission { id: 3, allow: true }));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SavePermissions(_)))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SyncPermissions(_)))
        );
        assert_eq!(
            state
                .config
                .permissions
                .policy_for("example.com", crate::core::state::Capability::Camera),
            PermissionPolicy::Allow
        );
    }

    #[test]
    fn queued_prompt_waits_for_first() {
        let mut state = state_with_tab();
        request_permission(&mut state, 1, "a.test");
        request_permission(&mut state, 2, "b.test");
        assert_eq!(state.prompts.len(), 2);
        // Deny the first; the second stays queued and we remain in Prompt mode.
        let effects = press(&mut state, "n");
        assert!(effects.contains(&Effect::ResolvePermission {
            id: 1,
            allow: false
        }));
        assert_eq!(state.mode.current, Mode::Prompt);
        assert_eq!(state.prompts.front().map(|p| p.id), Some(2));
    }

    #[test]
    fn key_j_scrolls_down() {
        let mut state = state_with_tab();
        let effects = press(&mut state, "j");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script.starts_with("window.scrollBy(0, 60);")
        )));
        assert!(state.input.pending.is_empty());
    }

    #[test]
    fn count_prefix_multiplies_scroll() {
        let mut state = state_with_tab();
        press(&mut state, "5");
        assert_eq!(state.input.count, "5");
        let effects = press(&mut state, "j");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script.starts_with("window.scrollBy(0, 300);")
        )));
        assert!(state.input.count.is_empty());
    }

    #[test]
    fn multi_key_sequence_gg() {
        let mut state = state_with_tab();
        let first = press(&mut state, "g");
        // `g` alone is only a prefix: no scroll yet.
        assert!(!first.iter().any(|e| matches!(e, Effect::EvalJs { .. })));
        assert_eq!(state.input.pending.len(), 1);
        press(&mut state, "g");
        assert!(state.input.pending.is_empty());
    }

    #[test]
    fn unknown_key_clears_pending() {
        let mut state = state_with_tab();
        let effects = press(&mut state, "q");
        assert!(state.input.pending.is_empty());
        assert_eq!(effects, vec![Effect::RenderStatus]);
    }

    #[test]
    fn key_colon_opens_command_line() {
        let mut state = state_with_tab();
        press(&mut state, ":");
        assert_eq!(state.mode.current, Mode::Command);
        assert!(state.command_line.active);
        assert_eq!(state.command_line.text, ":");
    }

    #[test]
    fn key_i_enters_insert_mode() {
        let mut state = state_with_tab();
        press(&mut state, "i");
        assert_eq!(state.mode.current, Mode::Insert);
    }

    #[test]
    fn input_focus_toggles_insert_mode() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        update(&mut state, Msg::InputFocusChanged { tab, focused: true });
        assert_eq!(state.mode.current, Mode::Insert);
        update(
            &mut state,
            Msg::InputFocusChanged {
                tab,
                focused: false,
            },
        );
        assert_eq!(state.mode.current, Mode::Normal);
    }

    #[test]
    fn hint_key_enters_mode_and_requests_labels() {
        let mut state = state_with_tab();
        let effects = press(&mut state, "f");
        assert_eq!(state.mode.current, Mode::Hint);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs {
                purpose: JsPurpose::HintsShown,
                ..
            }
        )));
    }

    #[test]
    fn hints_shown_populates_labels() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        press(&mut state, "f");
        let id = pending_id(&state, JsPurpose::HintsShown);
        update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("aa ab ba".to_string()),
            },
        );
        assert_eq!(state.hints.labels, vec!["aa", "ab", "ba"]);
    }

    #[test]
    fn empty_hints_leaves_hint_mode() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        press(&mut state, "f");
        let id = pending_id(&state, JsPurpose::HintsShown);
        update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok(String::new()),
            },
        );
        assert_eq!(state.mode.current, Mode::Normal);
    }

    #[test]
    fn unique_hint_prefix_follows_and_exits() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::Hint);
        state.hints.target = HintTarget::Current;
        state.hints.labels = vec!["aa".into(), "ab".into(), "ba".into()];
        let effects = update(&mut state, Msg::Key(Key::plain("b")));
        assert_eq!(state.mode.current, Mode::Normal);
        assert!(state.hints.labels.is_empty());
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script.contains("followClick(\"ba\")")
        )));
    }

    #[test]
    fn ambiguous_hint_prefix_filters() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::Hint);
        state.hints.labels = vec!["aa".into(), "ab".into(), "ba".into()];
        let effects = update(&mut state, Msg::Key(Key::plain("a")));
        assert_eq!(state.mode.current, Mode::Hint);
        assert_eq!(state.hints.input, "a");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script.contains("filter(\"a\")")
        )));
    }

    #[test]
    fn hint_tab_target_opens_href_result() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        state.mode.enter(Mode::Hint);
        state.hints.target = HintTarget::Tab;
        state.hints.labels = vec!["aa".into()];
        update(&mut state, Msg::Key(Key::plain("a")));
        let id = pending_id(&state, JsPurpose::HintHref);
        let effects = update(
            &mut state,
            Msg::JsResult {
                id,
                tab,
                result: Ok("https://x.test".to_string()),
            },
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "https://x.test"
        )));
    }

    #[test]
    fn escape_clears_hints() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::Hint);
        state.hints.labels = vec!["aa".into()];
        let effects = update(&mut state, Msg::Command(Command::ModeLeave));
        assert_eq!(state.mode.current, Mode::Normal);
        assert!(state.hints.labels.is_empty());
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script.contains("clear()")
        )));
    }

    #[test]
    fn open_url_binding_substitutes_current_url() {
        let mut state = state_with_tab();
        // `O` → cmd-set-text :open {url}
        press(&mut state, "O");
        assert_eq!(state.command_line.text, ":open https://example.com");
    }

    #[test]
    fn open_current_loads_in_active_tab() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Current,
                input: "rust-lang.org".to_string(),
            }),
        );
        assert!(effects.contains(&Effect::LoadUri {
            tab,
            uri: "https://rust-lang.org".to_string()
        }));
    }

    #[test]
    fn open_search_when_not_a_url() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Current,
                input: "hello world".to_string(),
            }),
        );
        assert!(effects.contains(&Effect::LoadUri {
            tab,
            uri: "https://duckduckgo.com/?q=hello+world".to_string()
        }));
    }

    #[test]
    fn open_tab_allocates_and_focuses() {
        let mut state = state_with_tab();
        let before = state.tabs.iter().count();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: "https://a.test".to_string(),
            }),
        );
        assert_eq!(state.tabs.iter().count(), before + 1);
        assert_eq!(state.tabs.active_index(), before); // newly focused
        assert!(matches!(
            effects[0],
            Effect::OpenTab {
                background: false,
                ..
            }
        ));
        assert!(effects.contains(&Effect::RenderTabs));
    }

    #[test]
    fn back_respects_count() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(&mut state, Msg::Command(Command::Back(3)));
        let gobacks = effects
            .iter()
            .filter(|e| matches!(e, Effect::GoBack { tab: t } if *t == tab))
            .count();
        assert_eq!(gobacks, 3);
    }

    #[test]
    fn mode_transitions() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::ModeEnter(Mode::Insert)));
        assert_eq!(state.mode.current, Mode::Insert);
        update(&mut state, Msg::Command(Command::ModeLeave));
        assert_eq!(state.mode.current, Mode::Normal);
    }

    #[test]
    fn command_line_accept_parses_and_runs() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":open example.org".to_string())),
        );
        assert_eq!(state.mode.current, Mode::Command);
        let effects = update(&mut state, Msg::Command(Command::Accept));
        assert_eq!(state.mode.current, Mode::Normal);
        assert!(!state.command_line.active);
        assert!(effects.contains(&Effect::LoadUri {
            tab,
            uri: "https://example.org".to_string()
        }));
    }

    #[test]
    fn unknown_command_reports_error() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":bogus".to_string())),
        );
        let effects = update(&mut state, Msg::Command(Command::Accept));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ShowMessage {
                level: MessageLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn scroll_emits_round_trip_and_result_updates_status() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();

        let effects = update(
            &mut state,
            Msg::Command(Command::Scroll(ScrollDir::Down, 2)),
        );
        // A single eval both scrolls and reads back the percentage.
        let read = effects
            .iter()
            .find_map(|e| match e {
                Effect::EvalJs {
                    id,
                    purpose: JsPurpose::ReadScrollPercent,
                    ..
                } => Some(*id),
                _ => None,
            })
            .expect("a ReadScrollPercent eval is emitted");
        assert_eq!(
            state.pending_js.get(&read),
            Some(&JsPurpose::ReadScrollPercent)
        );

        // Simulate the engine returning the result.
        let follow = update(
            &mut state,
            Msg::JsResult {
                id: read,
                tab,
                result: Ok("42".to_string()),
            },
        );
        assert_eq!(state.status.scroll_percent, Some(42));
        assert_eq!(follow, vec![Effect::RenderStatus]);
        assert!(!state.pending_js.contains_key(&read));
    }

    #[test]
    fn stale_js_result_is_ignored() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::JsResult {
                id: RequestId(999),
                tab,
                result: Ok("10".to_string()),
            },
        );
        assert!(effects.is_empty());
        assert_eq!(state.status.scroll_percent, None);
    }

    #[test]
    fn load_finished_records_history() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::Load {
                tab,
                event: LoadEvent::Finished,
            },
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::RecordHistory { uri, .. } if uri == "https://example.com"
        )));
    }

    #[test]
    fn undo_reopens_closed_tab() {
        let mut state = state_with_tab();
        // Open a second tab, then close it.
        update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: "https://second.test".to_string(),
            }),
        );
        assert_eq!(state.tabs.iter().count(), 2);
        update(&mut state, Msg::Command(Command::TabClose));
        assert_eq!(state.tabs.iter().count(), 1);
        let effects = update(&mut state, Msg::Command(Command::Undo));
        assert_eq!(state.tabs.iter().count(), 2);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "https://second.test"
        )));
    }

    #[test]
    fn tab_clone_duplicates_active_url() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::TabClone));
        assert_eq!(state.tabs.iter().count(), 2);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "https://example.com"
        )));
    }

    #[test]
    fn tab_move_reorders() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: "https://b.test".to_string(),
            }),
        );
        // Active is now index 1 (the new tab). Move it left.
        assert_eq!(state.tabs.active_index(), 1);
        update(&mut state, Msg::Command(Command::TabMove(-1)));
        assert_eq!(state.tabs.active_index(), 0);
    }

    #[test]
    fn tab_only_closes_others() {
        let mut state = state_with_tab();
        for url in ["https://a.test", "https://b.test"] {
            update(
                &mut state,
                Msg::Command(Command::Open {
                    target: OpenTarget::Tab,
                    input: url.to_string(),
                }),
            );
        }
        assert_eq!(state.tabs.iter().count(), 3);
        let effects = update(&mut state, Msg::Command(Command::TabOnly));
        assert_eq!(state.tabs.iter().count(), 1);
        let closed = effects
            .iter()
            .filter(|e| matches!(e, Effect::CloseTab { .. }))
            .count();
        assert_eq!(closed, 2);
    }

    #[test]
    fn history_completion_merges_for_current_generation() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":open ".to_string())),
        );
        let generation = state.completion.generation;
        let effects = update(
            &mut state,
            Msg::HistoryCompletion {
                generation,
                prefix: ":open ".to_string(),
                entries: vec![("https://github.com".to_string(), "GitHub".to_string())],
            },
        );
        assert!(
            state
                .completion
                .items
                .iter()
                .any(|i| i.command_line == ":open https://github.com")
        );
        assert!(effects.contains(&Effect::RenderCompletion));
    }

    #[test]
    fn stale_history_completion_is_ignored() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":open ".to_string())),
        );
        let stale = state.completion.generation.wrapping_sub(1);
        update(
            &mut state,
            Msg::HistoryCompletion {
                generation: stale,
                prefix: ":open ".to_string(),
                entries: vec![("https://stale.test".to_string(), String::new())],
            },
        );
        assert!(
            !state
                .completion
                .items
                .iter()
                .any(|i| i.command_line.contains("stale"))
        );
    }

    #[test]
    fn quickmark_save_then_load() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::QuickmarkSave("ex".to_string())),
        );
        assert_eq!(
            state.quickmarks.get("ex").map(String::as_str),
            Some("https://example.com")
        );
        let effects = update(
            &mut state,
            Msg::Command(Command::QuickmarkLoad("ex".to_string())),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::LoadUri { uri, .. } if uri == "https://example.com"
        )));
    }

    #[test]
    fn bookmark_add_dedups_by_url() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::BookmarkAdd));
        update(&mut state, Msg::Command(Command::BookmarkAdd));
        assert_eq!(state.bookmarks.len(), 1);
        assert_eq!(state.bookmarks[0].url, "https://example.com");
    }

    #[test]
    fn closing_last_tab_quits() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::TabClose));
        assert!(!state.running);
        assert!(effects.contains(&Effect::Quit));
    }

    #[test]
    fn darkmode_toggles() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::DarkMode));
        assert!(state.dark_mode);
        update(&mut state, Msg::Command(Command::DarkMode));
        assert!(!state.dark_mode);
    }

    #[test]
    fn session_save_emits_tab_urls() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::SessionSave("work".to_string())),
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::SaveSession { name, tabs }
                if name == "work" && tabs == &vec![("https://example.com".to_string(), false)]
        )));
    }

    #[test]
    fn session_loaded_opens_tabs() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::SessionLoaded(vec![
                ("https://a.test".to_string(), false),
                ("https://b.test".to_string(), false),
            ]),
        );
        assert_eq!(state.tabs.iter().count(), 3);
        let opened = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .count();
        assert_eq!(opened, 2);
    }

    #[test]
    fn plugin_eval_request_routes_to_active_tab() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(
            &mut state,
            Msg::PluginEvalRequest {
                id: 7,
                script: "document.title".to_string(),
            },
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::PluginEval { id: 7, tab: t, .. } if *t == tab
        )));
    }

    #[test]
    fn plugin_eval_result_resolves() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::PluginEvalResult {
                id: 7,
                result: "hello".to_string(),
            },
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ResolvePluginEval { id: 7, result } if result == "hello"
        )));
    }

    #[test]
    fn completion_tab_cycles_without_changing_command_line() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":".to_string())),
        );
        let typed = state.command_line.text.clone();
        let count = state.completion.items.len();
        assert!(count > 1);
        update(&mut state, Msg::CompletionNext);
        assert_eq!(state.completion.selected, Some(0));
        // The command line keeps the typed text; only the highlight moves.
        assert_eq!(state.command_line.text, typed);
        assert_eq!(state.completion.items.len(), count);
        update(&mut state, Msg::CompletionNext);
        assert_eq!(state.completion.selected, Some(1));
        assert_eq!(state.command_line.text, typed);
    }

    #[test]
    fn completion_enter_runs_highlighted_item() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":".to_string())),
        );
        update(&mut state, Msg::CompletionNext); // highlight first command (open)
        // Enter runs the highlighted command, not the typed ":".
        let effects = update(&mut state, Msg::Command(Command::Accept));
        assert!(effects.iter().any(|e| matches!(e, Effect::LoadUri { .. })));
        // Accepting resets completion and leaves command mode.
        assert!(state.completion.items.is_empty());
        assert_eq!(state.mode.current, Mode::Normal);
    }

    #[test]
    fn completion_edit_resets_selection() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":".to_string())),
        );
        update(&mut state, Msg::CompletionNext);
        assert_eq!(state.completion.selected, Some(0));
        // A genuine edit recomputes and drops the selection.
        update(&mut state, Msg::CommandLineChanged(":qui".to_string()));
        assert_eq!(state.completion.selected, None);
        assert!(
            state
                .completion
                .items
                .iter()
                .any(|i| i.command_line == ":quit ")
        );
    }

    #[test]
    fn completion_space_applies_selection() {
        let mut state = state_with_tab();
        update(
            &mut state,
            Msg::Command(Command::SetCommandLine(":".to_string())),
        );
        update(&mut state, Msg::CompletionNext); // highlight first command (open)
        let chosen = state.completion.preview().unwrap().to_string();
        update(&mut state, Msg::CompletionApply);
        assert_eq!(state.completion.selected, None);
        assert_eq!(state.completion.query, chosen);
        assert_eq!(state.command_line.text, chosen);
    }

    #[test]
    fn quit_clears_running() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::Quit));
        assert!(!state.running);
        // Quit persists the live session before tearing down.
        assert!(effects.contains(&Effect::SaveAutosave {
            tabs: vec![("https://example.com".to_string(), false)],
            active: 0,
            scroll: None,
        }));
        assert!(effects.contains(&Effect::Quit));
    }

    // --- Downloads ---

    fn dl_state() -> State {
        State::new(Config::default())
    }

    fn start_download(state: &mut State, id: u64) {
        update(
            state,
            Msg::DownloadStarted {
                id,
                filename: format!("file-{id}"),
                path: format!("/tmp/file-{id}"),
                source: format!("https://example.test/{id}"),
            },
        );
    }

    fn open_downloads(state: &mut State) {
        update(state, Msg::Command(Command::Downloads));
    }

    #[test]
    fn download_record_lifecycle_started_progress_finished() {
        let mut state = dl_state();
        start_download(&mut state, 1);
        let dl = state.downloads.get(&1).unwrap();
        assert_eq!(dl.status, DownloadStatus::Active);
        assert_eq!(dl.filename, "file-1");
        assert_eq!(dl.source, "https://example.test/1");
        assert_eq!(dl.path, PathBuf::from("/tmp/file-1"));

        update(
            &mut state,
            Msg::DownloadProgress {
                id: 1,
                received: 500,
                total: 1000,
            },
        );
        let dl = state.downloads.get(&1).unwrap();
        assert_eq!(dl.received, 500);
        assert_eq!(dl.total, 1000);

        update(&mut state, Msg::DownloadFinished { id: 1 });
        let dl = state.downloads.get(&1).unwrap();
        assert_eq!(dl.status, DownloadStatus::Finished);
        // Finishing snaps received to the known total so progress reads complete.
        assert_eq!(dl.received, 1000);
    }

    #[test]
    fn download_record_failed_and_cancelled() {
        let mut state = dl_state();
        start_download(&mut state, 2);
        update(
            &mut state,
            Msg::DownloadFailed {
                id: 2,
                error: "boom".into(),
            },
        );
        assert_eq!(
            state.downloads.get(&2).unwrap().status,
            DownloadStatus::Failed
        );

        start_download(&mut state, 3);
        update(&mut state, Msg::DownloadCancelled { id: 3 });
        assert_eq!(
            state.downloads.get(&3).unwrap().status,
            DownloadStatus::Cancelled
        );
    }

    #[test]
    fn download_failed_does_not_overwrite_cancelled() {
        let mut state = dl_state();
        start_download(&mut state, 4);
        update(&mut state, Msg::DownloadCancelled { id: 4 });
        // The engine reports a cancel through its failure path; core keeps Cancelled.
        update(
            &mut state,
            Msg::DownloadFailed {
                id: 4,
                error: "aborted".into(),
            },
        );
        assert_eq!(
            state.downloads.get(&4).unwrap().status,
            DownloadStatus::Cancelled
        );
    }

    #[test]
    fn downloads_clear_only_removes_terminal_records() {
        let mut state = dl_state();
        open_downloads(&mut state);
        start_download(&mut state, 5); // active
        // Clearing an active download is a no-op.
        press(&mut state, "x");
        assert!(state.downloads.contains_key(&5));
        // After it finishes, clearing removes it.
        update(&mut state, Msg::DownloadFinished { id: 5 });
        press(&mut state, "x");
        assert!(!state.downloads.contains_key(&5));
    }

    #[test]
    fn downloads_open_reveal_guarded_by_finished() {
        let mut state = dl_state();
        open_downloads(&mut state);
        start_download(&mut state, 6); // active
        // Open/reveal do nothing while the download is still active.
        assert!(press(&mut state, "o").is_empty());
        assert!(press(&mut state, "r").is_empty());

        update(&mut state, Msg::DownloadFinished { id: 6 });
        let effects = press(&mut state, "o");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenPath { path } if path.as_path() == std::path::Path::new("/tmp/file-6")
        )));
        let effects = press(&mut state, "r");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RevealPath { .. }))
        );
    }

    #[test]
    fn downloads_cancel_and_retry_guards() {
        let mut state = dl_state();
        open_downloads(&mut state);
        start_download(&mut state, 7); // active
        // Cancel acts on an active download; retry does not.
        assert!(
            press(&mut state, "c")
                .iter()
                .any(|e| matches!(e, Effect::CancelDownload { id: 7 }))
        );
        assert!(press(&mut state, "R").is_empty());

        // Retry acts on a failed download; cancel does not.
        update(
            &mut state,
            Msg::DownloadFailed {
                id: 7,
                error: "x".into(),
            },
        );
        assert!(
            press(&mut state, "R")
                .iter()
                .any(|e| matches!(e, Effect::RetryDownload { id: 7 }))
        );
        assert!(press(&mut state, "c").is_empty());
    }

    // --- History view ---

    use crate::core::state::HistoryRow;

    fn row(url: &str, title: &str, visit_count: i64, last_visit: i64) -> HistoryRow {
        HistoryRow {
            url: url.to_string(),
            title: title.to_string(),
            visit_count,
            last_visit,
        }
    }

    #[test]
    fn history_command_enters_mode_and_queries() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::History(None)));
        assert_eq!(state.mode.current, Mode::History);
        assert_eq!(state.history_view.filter, "");
        let current_gen = state.history_view.generation;
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::QueryHistoryView { query, generation } if query.is_empty() && *generation == current_gen
        )));
        assert!(effects.contains(&Effect::RenderHistory));
    }

    #[test]
    fn history_command_seeds_filter_from_arg() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::Command(Command::History(Some("rust".to_string()))),
        );
        assert_eq!(state.history_view.filter, "rust");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::QueryHistoryView { query, .. } if query == "rust"
        )));
    }

    #[test]
    fn stale_history_result_is_ignored() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::History(None)));
        let current_gen = state.history_view.generation;
        // A result matching the current generation is applied.
        update(
            &mut state,
            Msg::HistoryViewResult {
                generation: current_gen,
                rows: vec![row("https://a.test", "A", 1, 1)],
            },
        );
        assert_eq!(state.history_view.rows.len(), 1);
        // A stale (older-generation) result must not overwrite it.
        update(
            &mut state,
            Msg::HistoryViewResult {
                generation: current_gen.wrapping_sub(1),
                rows: vec![],
            },
        );
        assert_eq!(state.history_view.rows.len(), 1);
    }

    #[test]
    fn history_browse_keys_move_selection() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::History);
        state.history_view.rows = vec![
            row("https://a.test", "A", 1, 1),
            row("https://b.test", "B", 2, 2),
        ];
        state.history_view.selected = 0;
        press(&mut state, "j");
        assert_eq!(state.history_view.selected, 1);
        press(&mut state, "j"); // wraps to top
        assert_eq!(state.history_view.selected, 0);
        press(&mut state, "k"); // backward wraps to bottom
        assert_eq!(state.history_view.selected, 1);
    }

    #[test]
    fn history_delete_removes_row_and_emits_effect() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::History);
        state.history_view.rows = vec![
            row("https://a.test", "A", 1, 1),
            row("https://b.test", "B", 2, 2),
        ];
        state.history_view.selected = 0;
        let effects = press(&mut state, "x");
        assert_eq!(state.history_view.rows.len(), 1);
        assert_eq!(state.history_view.rows[0].url, "https://b.test");
        assert_eq!(state.history_view.selected, 0);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::DeleteHistory { url } if url == "https://a.test"))
        );
    }

    #[test]
    fn history_filter_edit_and_open() {
        let mut state = state_with_tab();
        state.mode.enter(Mode::History);
        // `/` enters filter editing.
        press(&mut state, "/");
        assert!(state.history_view.filter_edit);
        // Typing appends to the filter and re-queries.
        let effects = press(&mut state, "r");
        assert_eq!(state.history_view.filter, "r");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::QueryHistoryView { query, .. } if query == "r"))
        );
        press(&mut state, "u");
        assert_eq!(state.history_view.filter, "ru");
        // Escape leaves filter editing (back to browse), not the view.
        update(&mut state, Msg::Key(Key::plain("Escape")));
        assert!(!state.history_view.filter_edit);
        assert_eq!(state.mode.current, Mode::History);
    }

    #[test]
    fn history_open_current_loads_active_tab() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        state.mode.enter(Mode::History);
        state.history_view.rows = vec![row("https://history.test", "H", 1, 1)];
        state.history_view.selected = 0;
        let effects = press(&mut state, "o");
        assert_eq!(state.mode.current, Mode::Normal);
        assert!(effects.contains(&Effect::LoadUri {
            tab,
            uri: "https://history.test".to_string()
        }));
    }

    // --- Split views ---

    #[test]
    fn focus_tab_swaps_background_tab_into_focused_pane() {
        let mut state = state_with_tab();
        let bg = state.tabs.open("https://b.test"); // background (not mounted)
        let effects = focus_tab(&mut state, bg);
        assert_eq!(state.tabs.active_id(), Some(bg));
        assert_eq!(state.layout.focused_pane().unwrap().tab, bg);
        // Swapping content requires a layout rebuild.
        assert!(effects.iter().any(|e| matches!(e, Effect::RenderLayout)));
    }

    #[test]
    fn focus_tab_focuses_mounted_tab_without_rebuild() {
        let mut state = state_with_tab();
        let first = state.tabs.active_id().unwrap();
        update(&mut state, Msg::Command(Command::Split));
        // The first tab is mounted in the original pane; focusing it just shifts
        // focus and must not rebuild.
        let effects = focus_tab(&mut state, first);
        assert_eq!(state.layout.focused_pane().unwrap().tab, first);
        assert!(!effects.iter().any(|e| matches!(e, Effect::RenderLayout)));
    }

    #[test]
    fn split_opens_new_tab_and_focuses_it() {
        let mut state = state_with_tab();
        let before = state.tabs.iter().count();
        let effects = update(&mut state, Msg::Command(Command::Split));
        assert_eq!(state.tabs.iter().count(), before + 1);
        assert_eq!(state.layout.panes.len(), 2);
        assert_eq!(
            state.layout.focused_pane().unwrap().tab,
            state.tabs.active_id().unwrap()
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::RenderLayout)));
    }

    #[test]
    fn vsplit_then_focus_cycle_round_trips() {
        let mut state = state_with_tab();
        let first = state.tabs.active_id().unwrap();
        update(&mut state, Msg::Command(Command::Vsplit));
        // After split the new pane is focused; cycling returns to the first.
        update(&mut state, Msg::Command(Command::FocusPane { next: true }));
        assert_eq!(state.layout.focused_pane().unwrap().tab, first);
    }

    #[test]
    fn tab_close_on_mounted_tab_collapses_pane() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::Vsplit));
        assert_eq!(state.layout.panes.len(), 2);
        let closed = state.tabs.active_id().unwrap();
        update(&mut state, Msg::Command(Command::TabClose));
        assert_eq!(state.layout.panes.len(), 1);
        assert!(state.layout.pane_with_tab(closed).is_none());
    }

    #[test]
    fn close_pane_keeps_tab_and_drops_pane() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::Vsplit));
        let kept = state.tabs.active_id().unwrap();
        update(&mut state, Msg::Command(Command::ClosePane));
        assert_eq!(state.layout.panes.len(), 1);
        // The tab stays in the tab list (became background).
        assert!(state.tabs.get(kept).is_some());
        assert!(state.layout.pane_with_tab(kept).is_none());
    }

    #[test]
    fn tab_next_keeps_active_equals_focused_pane_tab() {
        let mut state = state_with_tab();
        state.tabs.open("https://b.test");
        state.tabs.open("https://c.test");
        update(&mut state, Msg::Command(Command::TabNext(1)));
        assert_eq!(
            state.tabs.active_id(),
            state.layout.focused_pane().map(|p| p.tab)
        );
    }

    #[test]
    fn only_pane_leaves_single_pane() {
        let mut state = state_with_tab();
        update(&mut state, Msg::Command(Command::Vsplit));
        update(&mut state, Msg::Command(Command::Split));
        assert_eq!(state.layout.panes.len(), 3);
        update(&mut state, Msg::Command(Command::OnlyPane));
        assert_eq!(state.layout.panes.len(), 1);
    }
}
