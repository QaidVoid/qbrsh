//! Command-line completion.
//!
//! Candidates are computed from the command-line text and ranked with the
//! `nucleo` fuzzy matcher. Currently only command-name completion is sourced
//! here; value sources (history, bookmarks, quickmarks) plug in with their
//! subsystems. The state and ranking are pure and unit-testable.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use crate::core::command::COMMAND_CATALOG;

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
pub fn complete(text: &str) -> Vec<CompletionItem> {
    let stripped = text.strip_prefix(':').unwrap_or(text);
    // Once there is whitespace we are completing an argument; argument sources
    // (history/bookmarks/quickmarks) are added with those subsystems.
    if stripped.contains(char::is_whitespace) {
        return Vec::new();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(stripped, CaseMatching::Ignore, Normalization::Smart);
    let names: Vec<&str> = COMMAND_CATALOG.iter().map(|(n, _)| *n).collect();
    pattern
        .match_list(names, &mut matcher)
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(name, _)| {
            let desc = COMMAND_CATALOG
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, d)| *d)
                .unwrap_or_default();
            CompletionItem {
                display: format!("{name:<16}{desc}"),
                command_line: format!(":{name} "),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_completes_command_names() {
        let items = complete(":tab");
        assert!(!items.is_empty());
        // A literal prefix match ranks ahead of looser fuzzy hits.
        assert!(items[0].command_line.starts_with(":tab"));
        assert!(items.iter().any(|i| i.command_line == ":tabopen "));
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let items = complete(":tco");
        assert!(items.iter().any(|i| i.command_line == ":tab-close "));
    }

    #[test]
    fn empty_query_lists_all() {
        let items = complete(":");
        assert_eq!(items.len(), COMMAND_CATALOG.len().min(15));
    }

    #[test]
    fn argument_position_has_no_command_completion() {
        assert!(complete(":open foo").is_empty());
    }

    #[test]
    fn cycling_wraps() {
        let mut c = CompletionState {
            items: complete(":tab"),
            ..Default::default()
        };
        let n = c.items.len();
        c.next();
        assert_eq!(c.selected, Some(0));
        c.prev();
        assert_eq!(c.selected, Some(n - 1));
    }
}
