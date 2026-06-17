//! Command-line completion.
//!
//! Candidates are computed from the command-line text and ranked with the
//! `nucleo` fuzzy matcher. Currently only command-name completion is sourced
//! here; value sources (history, bookmarks, quickmarks) plug in with their
//! subsystems. The state and ranking are pure and unit-testable.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::core::command::COMMAND_CATALOG;
use crate::core::state::State;

const MAX_ITEMS: usize = 15;

/// A single completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// Text shown in the popup.
    pub display: String,
    /// Full command-line text (including leading `:`) applied when selected.
    pub command_line: String,
}

/// Active completion candidates and the current selection.
#[derive(Debug, Default)]
pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    pub selected: Option<usize>,
    /// Suppresses recompute for the one command-line change we cause when
    /// echoing a selection back into the entry.
    pub suppress: bool,
}

impl CompletionState {
    /// Clear all completion state.
    pub fn reset(&mut self) {
        self.items.clear();
        self.selected = None;
        self.suppress = false;
    }

    /// Select the next candidate (wrapping); returns its command-line text.
    pub fn next(&mut self) -> Option<String> {
        self.step(1)
    }

    /// Select the previous candidate (wrapping); returns its command-line text.
    pub fn prev(&mut self) -> Option<String> {
        self.step(-1)
    }

    fn step(&mut self, delta: i32) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        let len = self.items.len() as i32;
        let idx = match self.selected {
            None => {
                if delta > 0 {
                    0
                } else {
                    len - 1
                }
            }
            Some(cur) => (cur as i32 + delta).rem_euclid(len),
        } as usize;
        self.selected = Some(idx);
        Some(self.items[idx].command_line.clone())
    }
}

/// Compute completion candidates for the given command-line text.
///
/// Before the first space, command names are completed. After it, argument
/// sources are used: quickmarks and bookmarks for `open`/`tabopen` and the
/// `*-load`/`*-del` commands. History candidates arrive asynchronously and are
/// merged separately.
pub fn complete(text: &str, state: &State) -> Vec<CompletionItem> {
    let stripped = text.strip_prefix(':').unwrap_or(text);
    match stripped.split_once(char::is_whitespace) {
        None => complete_commands(stripped),
        Some((word, rest)) => complete_args(word, rest.trim_start(), state),
    }
}

fn complete_commands(query: &str) -> Vec<CompletionItem> {
    let cands = COMMAND_CATALOG.iter().map(|(name, desc)| {
        (
            name.to_string(),
            CompletionItem {
                display: format!("{name:<16}{desc}"),
                command_line: format!(":{name} "),
            },
        )
    });
    fuzzy(query, cands.collect())
}

fn complete_args(word: &str, query: &str, state: &State) -> Vec<CompletionItem> {
    let mut cands: Vec<(String, CompletionItem)> = Vec::new();
    match word {
        "open" | "o" | "tabopen" | "t" => {
            for (name, url) in &state.quickmarks {
                cands.push((
                    format!("{name} {url}"),
                    CompletionItem {
                        display: format!("{name}  {url}"),
                        command_line: format!(":{word} {url}"),
                    },
                ));
            }
            for b in &state.bookmarks {
                cands.push((
                    format!("{} {}", b.title, b.url),
                    CompletionItem {
                        display: format!("{}  {}", b.title, b.url),
                        command_line: format!(":{word} {}", b.url),
                    },
                ));
            }
        }
        "quickmark-load" | "quickmark-del" => {
            for (name, url) in &state.quickmarks {
                cands.push((
                    name.clone(),
                    CompletionItem {
                        display: format!("{name}  {url}"),
                        command_line: format!(":{word} {name}"),
                    },
                ));
            }
        }
        "bookmark-load" | "bookmark-del" => {
            for b in &state.bookmarks {
                cands.push((
                    format!("{} {}", b.title, b.url),
                    CompletionItem {
                        display: format!("{}  {}", b.title, b.url),
                        command_line: format!(":{word} {}", b.url),
                    },
                ));
            }
        }
        _ => return Vec::new(),
    }
    fuzzy(query, cands)
}

/// Fuzzy-rank `candidates` (keyed by their first element) against `query`.
fn fuzzy(query: &str, candidates: Vec<(String, CompletionItem)>) -> Vec<CompletionItem> {
    if query.is_empty() {
        return candidates
            .into_iter()
            .take(MAX_ITEMS)
            .map(|(_, item)| item)
            .collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, CompletionItem)> = candidates
        .into_iter()
        .filter_map(|(key, item)| {
            pattern
                .score(Utf32Str::new(&key, &mut buf), &mut matcher)
                .map(|score| (score, item))
        })
        .collect();
    scored.sort_by_key(|(score, _)| Reverse(*score));
    scored.into_iter().take(MAX_ITEMS).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::{Config, State};

    fn state() -> State {
        State::new(Config::default())
    }

    #[test]
    fn prefix_completes_command_names() {
        let items = complete(":tab", &state());
        assert!(!items.is_empty());
        // A literal prefix match ranks ahead of looser fuzzy hits.
        assert!(items[0].command_line.starts_with(":tab"));
        assert!(items.iter().any(|i| i.command_line == ":tabopen "));
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let items = complete(":tco", &state());
        assert!(items.iter().any(|i| i.command_line == ":tab-close "));
    }

    #[test]
    fn empty_query_lists_all() {
        let items = complete(":", &state());
        assert_eq!(items.len(), COMMAND_CATALOG.len().min(15));
    }

    #[test]
    fn open_argument_completes_from_quickmarks() {
        let mut s = state();
        s.quickmarks
            .insert("gh".to_string(), "https://github.com".to_string());
        let items = complete(":open gh", &s);
        assert!(items.iter().any(|i| i.command_line == ":open https://github.com"));
    }

    #[test]
    fn cycling_wraps() {
        let mut c = CompletionState {
            items: complete(":tab", &state()),
            ..Default::default()
        };
        let n = c.items.len();
        c.next();
        assert_eq!(c.selected, Some(0));
        c.prev();
        assert_eq!(c.selected, Some(n - 1));
    }
}
