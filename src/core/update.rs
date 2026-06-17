//! The sole state-mutation point.
//!
//! [`update`] is synchronous, non-blocking, and the only function that mutates
//! [`State`]. It returns the side effects to perform; it never performs them
//! inline. Results of asynchronous effects re-enter through [`update`] as new
//! messages (see [`Msg::JsResult`]).

use crate::core::command::{Command, OpenTarget, ScrollDir, YankWhat};
use crate::core::effect::{Effect, MessageLevel};
use crate::core::key::Key;
use crate::core::msg::{JsPurpose, LoadEvent, Msg};
use crate::core::state::{Mode, State};
use crate::core::trie::TrieMatch;

/// Apply a single message to the state and return the effects to perform.
pub fn update(state: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => handle_key(state, key),
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
                            effects.push(Effect::RecordHistory { uri, title });
                        }
                    }
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
            if let Some(t) = state.tabs.get_mut(tab) {
                t.progress = fraction;
                if state.tabs.active_id() == Some(tab) {
                    effects.push(Effect::RenderStatus);
                }
            }
            effects
        }

        Msg::JsResult { id, result, .. } => {
            let Some(purpose) = state.pending_js.remove(&id) else {
                return Vec::new();
            };
            match purpose {
                JsPurpose::FireAndForget => Vec::new(),
                JsPurpose::ReadScrollPercent => {
                    if let Ok(text) = result
                        && let Ok(pct) = text.trim().parse::<f64>()
                    {
                        state.status.scroll_percent = Some(pct.clamp(0.0, 100.0) as u8);
                        return vec![Effect::RenderStatus];
                    }
                    Vec::new()
                }
            }
        }

        Msg::CommandLineChanged(text) => {
            state.command_line.text = text;
            Vec::new()
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
                OpenTarget::Background => open_tab(state, uri, true),
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

        Command::ModeEnter(mode) => {
            state.mode.enter(mode);
            if mode == Mode::Command {
                state.command_line.active = true;
            }
            vec![Effect::RenderStatus]
        }
        Command::ModeLeave => {
            state.mode.leave();
            state.command_line.active = false;
            state.command_line.text.clear();
            vec![Effect::RenderStatus]
        }
        Command::SetCommandLine(prefix) => {
            let text = substitute_vars(state, &prefix);
            state.mode.enter(Mode::Command);
            state.command_line.active = true;
            state.command_line.text = text;
            vec![Effect::RenderStatus]
        }
        Command::Accept => {
            let text = std::mem::take(&mut state.command_line.text);
            state.command_line.active = false;
            state.mode.leave();
            let mut effects = vec![Effect::RenderStatus];
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return effects;
            }
            if let Some(rest) = trimmed.strip_prefix(':') {
                push_parsed(state, rest.trim(), &mut effects);
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
    let mut effects = vec![Effect::OpenTab {
        id,
        uri,
        background,
    }];
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
    use crate::core::command::{Command, OpenTarget, ScrollDir};
    use crate::core::key::Key;
    use crate::core::msg::{LoadEvent, RequestId};
    use crate::core::state::{Config, Mode, State};

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
    fn closing_last_tab_quits() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::TabClose));
        assert!(!state.running);
        assert!(effects.contains(&Effect::Quit));
    }

    #[test]
    fn quit_clears_running() {
        let mut state = state_with_tab();
        let effects = update(&mut state, Msg::Command(Command::Quit));
        assert!(!state.running);
        assert_eq!(effects, vec![Effect::Quit]);
    }
}
