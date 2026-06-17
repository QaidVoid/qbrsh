//! The sole state-mutation point.
//!
//! [`update`] is synchronous, non-blocking, and the only function that mutates
//! [`State`]. It returns the side effects to perform; it never performs them
//! inline. Results of asynchronous effects re-enter through [`update`] as new
//! messages (see [`Msg::JsResult`]).

use crate::core::command::{Command, HintTarget, OpenTarget, ScrollDir, YankWhat};
use crate::core::completion::{CompletionItem, complete};
use crate::core::effect::{Effect, MessageLevel};
use crate::core::key::Key;
use crate::core::msg::{JsPurpose, LoadEvent, Msg};
use crate::core::state::{Bookmark, Mode, State};
use crate::core::trie::TrieMatch;

/// Apply a single message to the state and return the effects to perform.
pub fn update(state: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => {
            if state.mode.current == Mode::Hint {
                handle_hint_key(state, key)
            } else {
                handle_key(state, key)
            }
        }
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
                        if !uri.is_empty() && uri != "about:blank" {
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
            if let Some(t) = state.tabs.get_mut(tab) {
                t.progress = fraction;
                if state.tabs.active_id() == Some(tab) {
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
                        return handle_command(
                            state,
                            Command::Open {
                                target: OpenTarget::Tab,
                                input: href,
                            },
                        );
                    }
                    Vec::new()
                }
            }
        }

        Msg::CommandLineChanged(text) => {
            state.command_line.text = text;
            // Ignore the echo of a selection we applied ourselves.
            if state.completion.suppress {
                state.completion.suppress = false;
                return Vec::new();
            }
            state.completion.generation = state.completion.generation.wrapping_add(1);
            let items = complete(&state.command_line.text, state);
            state.completion.items = items;
            state.completion.selected = None;
            let mut effects = vec![Effect::RenderCompletion];
            effects.extend(history_query_effect(state));
            effects
        }
        Msg::CompletionNext => match state.completion.next() {
            Some(command_line) => {
                state.command_line.text = command_line;
                state.completion.suppress = true;
                vec![Effect::RenderStatus, Effect::RenderCompletion]
            }
            None => Vec::new(),
        },
        Msg::CompletionPrev => match state.completion.prev() {
            Some(command_line) => {
                state.command_line.text = command_line;
                state.completion.suppress = true;
                vec![Effect::RenderStatus, Effect::RenderCompletion]
            }
            None => Vec::new(),
        },
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
                state
                    .completion
                    .items
                    .push(CompletionItem {
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
            state.config = config;
            vec![
                Effect::ApplyTheme,
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: "config reloaded".to_string(),
                },
            ]
        }

        Msg::SessionLoaded(urls) => {
            let mut effects = Vec::new();
            for url in urls {
                effects.extend(open_tab(state, url, true));
            }
            effects
        }

        Msg::PluginMessage(text) => vec![Effect::ShowMessage {
            level: MessageLevel::Info,
            text,
        }],
        Msg::PluginEval(script) => fire_js(state, script),

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
                    let mut effects = handle_command(state, c);
                    effects.push(Effect::RenderStatus);
                    effects
                }
                Err(text) => vec![
                    Effect::ShowMessage {
                        level: MessageLevel::Error,
                        text,
                    },
                    Effect::RenderStatus,
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
    let mut effects = fire_js(state, format!("window.__qbrshHints.filter('{prefix}')"));
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
            let mut effects = fire_js(state, format!("window.__qbrshHints.followClick('{label}')"));
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
                    script: format!("window.__qbrshHints.getHref('{label}')"),
                    purpose: JsPurpose::HintHref,
                },
                Effect::RenderStatus,
            ]
        }
    }
}

/// If the command line is completing an `open`/`tabopen` argument, build the
/// effect that queries history for matching URLs.
fn history_query_effect(state: &State) -> Option<Effect> {
    let stripped = state.command_line.text.strip_prefix(':')?;
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
            let uri = normalize_target(&input);
            match target {
                OpenTarget::Current => match state.tabs.active_id() {
                    Some(tab) => vec![Effect::LoadUri { tab, uri }],
                    None => open_tab(state, uri, false),
                },
                OpenTarget::Tab => open_tab(state, uri, false),
            }
        }

        Command::Back(count) => with_active(state, |tab| {
            (0..count.max(1)).map(|_| Effect::GoBack { tab }).collect()
        }),
        Command::Forward(count) => with_active(state, |tab| {
            (0..count.max(1))
                .map(|_| Effect::GoForward { tab })
                .collect()
        }),
        Command::Reload { bypass_cache } => {
            with_active(state, |tab| vec![Effect::Reload { tab, bypass_cache }])
        }
        Command::Stop => with_active(state, |tab| vec![Effect::Stop { tab }]),

        Command::Scroll(dir, count) => scroll(state, scroll_script(dir, count.max(1))),
        Command::ScrollPage { down, half } => {
            let frac = if half { 0.5 } else { 0.9 } * if down { 1.0 } else { -1.0 };
            scroll(state, format!("window.scrollBy(0, window.innerHeight * {frac});"))
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
                    script: format!("window.__qbrshHints.show('{HINT_CHARS}')"),
                    purpose: JsPurpose::HintsShown,
                },
                Effect::RenderStatus,
            ]
        }

        Command::TabClose => match state.tabs.close_active() {
            Some((closed, next)) => {
                let mut effects = vec![Effect::CloseTab { tab: closed }, Effect::RenderTabs];
                match next {
                    Some(next_id) => {
                        effects.push(Effect::FocusTab { tab: next_id });
                        effects.push(Effect::RenderStatus);
                    }
                    None => {
                        state.running = false;
                        effects.push(Effect::Quit);
                    }
                }
                effects
            }
            None => Vec::new(),
        },
        Command::TabNext(count) => match state.tabs.next(count) {
            Some(id) => vec![
                Effect::FocusTab { tab: id },
                Effect::RenderTabs,
                Effect::RenderStatus,
            ],
            None => Vec::new(),
        },
        Command::TabPrev(count) => match state.tabs.prev(count) {
            Some(id) => vec![
                Effect::FocusTab { tab: id },
                Effect::RenderTabs,
                Effect::RenderStatus,
            ],
            None => Vec::new(),
        },
        Command::TabSelect(index) => match state.tabs.focus_index_1based(index) {
            Some(id) => vec![
                Effect::FocusTab { tab: id },
                Effect::RenderTabs,
                Effect::RenderStatus,
            ],
            None => vec![Effect::ShowMessage {
                level: MessageLevel::Error,
                text: format!("no tab at index {index}"),
            }],
        },
        Command::Undo => match state.tabs.undo() {
            Some(closed) => open_tab(state, closed.url, false),
            None => vec![Effect::ShowMessage {
                level: MessageLevel::Info,
                text: "no closed tabs to reopen".to_string(),
            }],
        },
        Command::TabClone => match state.tabs.active().map(|t| t.url.clone()) {
            Some(url) => open_tab(state, url, false),
            None => Vec::new(),
        },
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
            state.command_line.text = text;
            state.completion.generation = state.completion.generation.wrapping_add(1);
            let items = complete(&state.command_line.text, state);
            state.completion.items = items;
            state.completion.selected = None;
            let mut effects = vec![Effect::RenderStatus, Effect::RenderCompletion];
            effects.extend(history_query_effect(state));
            effects
        }
        Command::Accept => {
            let text = std::mem::take(&mut state.command_line.text);
            state.command_line.active = false;
            state.completion.reset();
            state.mode.leave();
            let mut effects = vec![Effect::RenderStatus, Effect::RenderCompletion];
            let trimmed = text.trim();
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
            } else if trimmed.starts_with('/') || trimmed.starts_with('?') {
                effects.push(Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: "in-page search lands in a later milestone".to_string(),
                });
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
            let Some((url, title)) = state.tabs.active().map(|t| (t.url.clone(), t.title.clone()))
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

        Command::Set { key, value } => match state.config.set(&key, &value) {
            Ok(()) => vec![
                Effect::ApplyTheme,
                Effect::ShowMessage {
                    level: MessageLevel::Info,
                    text: format!("{key} = {value}"),
                },
            ],
            Err(text) => vec![Effect::ShowMessage {
                level: MessageLevel::Error,
                text,
            }],
        },
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
                    urls: state.tabs.urls(),
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

        Command::Quit => {
            state.running = false;
            vec![Effect::Quit]
        }
        Command::Nop => Vec::new(),
    }
}

/// Run `f` with the active tab id, or produce no effects if there is none.
fn with_active(state: &State, f: impl FnOnce(crate::core::state::TabId) -> Vec<Effect>) -> Vec<Effect> {
    match state.tabs.active_id() {
        Some(tab) => f(tab),
        None => Vec::new(),
    }
}

/// Open a new tab in state and emit the effect to realize its web view.
fn open_tab(state: &mut State, uri: String, background: bool) -> Vec<Effect> {
    let id = state.tabs.open(&uri);
    let mut effects = vec![
        Effect::OpenTab {
            id,
            uri: uri.clone(),
            background,
        },
        Effect::FireHook {
            event: "tab_open".to_string(),
            arg: uri,
        },
    ];
    if background {
        effects.push(Effect::RenderTabs);
    } else {
        state.tabs.focus_last();
        effects.push(Effect::FocusTab { tab: id });
        effects.push(Effect::RenderTabs);
        effects.push(Effect::RenderStatus);
    }
    effects
}

/// Emit a fire-and-forget scroll plus a follow-up read of the scroll percentage,
/// demonstrating the async result round-trip.
fn scroll(state: &mut State, script: String) -> Vec<Effect> {
    let Some(tab) = state.tabs.active_id() else {
        return Vec::new();
    };
    let scroll_id = state.alloc_request_id();
    state.pending_js.insert(scroll_id, JsPurpose::FireAndForget);

    let read_id = state.alloc_request_id();
    state
        .pending_js
        .insert(read_id, JsPurpose::ReadScrollPercent);

    vec![
        Effect::EvalJs {
            id: scroll_id,
            tab,
            script,
            purpose: JsPurpose::FireAndForget,
        },
        Effect::EvalJs {
            id: read_id,
            tab,
            script: SCROLL_PERCENT_SCRIPT.to_string(),
            purpose: JsPurpose::ReadScrollPercent,
        },
    ]
}

const SCROLL_PERCENT_SCRIPT: &str = "(function(){var e=document.documentElement;var m=e.scrollHeight-e.clientHeight;return m<=0?0:Math.round(e.scrollTop/m*100);})()";

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

/// Normalize command-line input into a navigable URI.
///
/// A minimal heuristic for the skeleton: explicit schemes pass through, a single
/// dotted token becomes `https://`, anything else is a DuckDuckGo search. The
/// full search-engine and URL handling lands with the URL subsystem.
fn normalize_target(input: &str) -> String {
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
    format!("https://duckduckgo.com/?q={}", t.replace(' ', "+"))
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
    use crate::core::command::{Command, HintTarget, OpenTarget, ScrollDir};
    use crate::core::key::Key;
    use crate::core::msg::{JsPurpose, LoadEvent, RequestId};
    use crate::core::state::{Config, Mode, State};

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
        state.tabs.focus_last();
        assert_eq!(state.tabs.active_id(), Some(id));
        state
    }

    fn press(state: &mut State, sym: &str) -> Vec<Effect> {
        update(state, Msg::Key(Key::plain(sym)))
    }

    #[test]
    fn key_j_scrolls_down() {
        let mut state = state_with_tab();
        let effects = press(&mut state, "j");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EvalJs { script, .. } if script == "window.scrollBy(0, 60);"
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
            Effect::EvalJs { script, .. } if script == "window.scrollBy(0, 300);"
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
        update(&mut state, Msg::InputFocusChanged { tab, focused: false });
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
            Effect::EvalJs { script, .. } if script.contains("followClick('ba')")
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
            Effect::EvalJs { script, .. } if script.contains("filter('a')")
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
        assert_eq!(
            effects,
            vec![Effect::LoadUri {
                tab,
                uri: "https://rust-lang.org".to_string()
            }]
        );
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
        assert_eq!(
            effects,
            vec![Effect::LoadUri {
                tab,
                uri: "https://duckduckgo.com/?q=hello+world".to_string()
            }]
        );
    }

    #[test]
    fn open_tab_allocates_and_focuses() {
        let mut state = state_with_tab();
        let before = state.tabs.len();
        let effects = update(
            &mut state,
            Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: "https://a.test".to_string(),
            }),
        );
        assert_eq!(state.tabs.len(), before + 1);
        assert_eq!(state.tabs.active_index(), before); // newly focused
        assert!(matches!(effects[0], Effect::OpenTab { background: false, .. }));
        assert!(effects.contains(&Effect::RenderTabs));
    }

    #[test]
    fn back_respects_count() {
        let mut state = state_with_tab();
        let tab = state.tabs.active_id().unwrap();
        let effects = update(&mut state, Msg::Command(Command::Back(3)));
        assert_eq!(effects, vec![Effect::GoBack { tab }; 3]);
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

        let effects = update(&mut state, Msg::Command(Command::Scroll(ScrollDir::Down, 2)));
        // Two evals: the scroll itself and the percent read.
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
        assert_eq!(state.pending_js.get(&read), Some(&JsPurpose::ReadScrollPercent));

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
        assert_eq!(state.tabs.len(), 2);
        update(&mut state, Msg::Command(Command::TabClose));
        assert_eq!(state.tabs.len(), 1);
        let effects = update(&mut state, Msg::Command(Command::Undo));
        assert_eq!(state.tabs.len(), 2);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::OpenTab { uri, .. } if uri == "https://second.test"
        )));
    }

    #[test]
    fn tab_clone_duplicates_active_url() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::TabClone));
        assert_eq!(state.tabs.len(), 2);
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
        assert_eq!(state.tabs.len(), 3);
        let effects = update(&mut state, Msg::Command(Command::TabOnly));
        assert_eq!(state.tabs.len(), 1);
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
        assert!(state.completion.items.iter().any(|i| i.command_line == ":open https://github.com"));
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
        assert!(!state.completion.items.iter().any(|i| i.command_line.contains("stale")));
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
            Effect::SaveSession { name, urls }
                if name == "work" && urls == &vec!["https://example.com".to_string()]
        )));
    }

    #[test]
    fn session_loaded_opens_tabs() {
        let mut state = state_with_tab();
        let effects = update(
            &mut state,
            Msg::SessionLoaded(vec!["https://a.test".to_string(), "https://b.test".to_string()]),
        );
        assert_eq!(state.tabs.len(), 3);
        let opened = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .count();
        assert_eq!(opened, 2);
    }

    #[test]
    fn quit_clears_running() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::Quit));
        assert!(!state.running);
        assert_eq!(effects, vec![Effect::Quit]);
    }
}
