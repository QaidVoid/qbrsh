//! A trie for O(k) key-sequence matching.
//!
//! Each path from the root to a node carrying a command represents a complete
//! keybinding. Prefix matching distinguishes partial input from complete
//! bindings; commands are stored as strings parsed by [`Command::parse`].
//!
//! [`Command::parse`]: crate::core::command::Command::parse

use std::collections::HashMap;

use crate::core::key::Key;

/// Result of looking up a key sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrieMatch {
    /// No binding starts with this sequence.
    None,
    /// Some bindings have this as a prefix, but no exact match yet.
    Partial,
    /// Exact match; carries the command string.
    Exact(String),
    /// Exact match that is also a prefix of longer bindings.
    Ambiguous(String),
}

#[derive(Debug, Default)]
struct TrieNode {
    children: HashMap<Key, TrieNode>,
    command: Option<String>,
}

/// A prefix tree mapping key sequences to command strings.
#[derive(Debug, Default)]
pub struct BindingTrie {
    root: TrieNode,
}

impl BindingTrie {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding mapping `keys` to `command`.
    pub fn insert(&mut self, keys: &[Key], command: String) {
        let mut node = &mut self.root;
        for key in keys {
            node = node.children.entry(key.clone()).or_default();
        }
        node.command = Some(command);
    }

    /// Look up a key sequence.
    pub fn lookup(&self, keys: &[Key]) -> TrieMatch {
        let mut node = &self.root;
        for key in keys {
            match node.children.get(key) {
                Some(child) => node = child,
                None => return TrieMatch::None,
            }
        }
        match (&node.command, node.children.is_empty()) {
            (Some(cmd), true) => TrieMatch::Exact(cmd.clone()),
            (Some(cmd), false) => TrieMatch::Ambiguous(cmd.clone()),
            (None, _) => TrieMatch::Partial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::key::Key;

    #[test]
    fn single_key_exact() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("j")], "scroll down".into());
        assert_eq!(
            t.lookup(&[Key::plain("j")]),
            TrieMatch::Exact("scroll down".into())
        );
    }

    #[test]
    fn multi_key_partial_then_exact() {
        let mut t = BindingTrie::new();
        t.insert(
            &[Key::plain("g"), Key::plain("g")],
            "scroll-to-perc 0".into(),
        );
        assert_eq!(t.lookup(&[Key::plain("g")]), TrieMatch::Partial);
        assert_eq!(
            t.lookup(&[Key::plain("g"), Key::plain("g")]),
            TrieMatch::Exact("scroll-to-perc 0".into())
        );
    }

    #[test]
    fn unknown_is_none() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("j")], "scroll down".into());
        assert_eq!(t.lookup(&[Key::plain("x")]), TrieMatch::None);
    }

    #[test]
    fn modifiers_distinguish_bindings() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("f")], "hint".into());
        t.insert(
            &[Key {
                sym: "f".into(),
                ctrl: true,
                alt: false,
                shift: false,
            }],
            "scroll-page down".into(),
        );
        assert_eq!(
            t.lookup(&[Key::plain("f")]),
            TrieMatch::Exact("hint".into())
        );
        assert_eq!(
            t.lookup(&[Key {
                sym: "f".into(),
                ctrl: true,
                alt: false,
                shift: false
            }]),
            TrieMatch::Exact("scroll-page down".into())
        );
    }
}
