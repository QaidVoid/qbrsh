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

    /// Insert a binding, rejecting a sequence that is a strict prefix of, or
    /// strictly extends, an existing binding (which would make the trie
    /// ambiguous). Replacing an existing exact binding is allowed.
    pub fn insert_checked(&mut self, keys: &[Key], command: String) -> Result<(), String> {
        if keys.is_empty() {
            return Err("empty key sequence".to_string());
        }
        let mut node = &self.root;
        for (i, key) in keys.iter().enumerate() {
            if i > 0 && node.command.is_some() {
                return Err("conflicts with a shorter existing binding".to_string());
            }
            match node.children.get(key) {
                Some(child) => node = child,
                None => {
                    self.insert(keys, command);
                    return Ok(());
                }
            }
        }
        if !node.children.is_empty() {
            return Err("is a prefix of a longer existing binding".to_string());
        }
        self.insert(keys, command);
        Ok(())
    }

    /// Remove the binding for `keys`, pruning now-empty nodes so a removed prefix
    /// no longer reports as partial. Returns whether a binding was removed.
    pub fn remove(&mut self, keys: &[Key]) -> bool {
        Self::remove_node(&mut self.root, keys)
    }

    fn remove_node(node: &mut TrieNode, keys: &[Key]) -> bool {
        match keys.split_first() {
            None => node.command.take().is_some(),
            Some((first, rest)) => {
                let Some(child) = node.children.get_mut(first) else {
                    return false;
                };
                let removed = Self::remove_node(child, rest);
                if child.command.is_none() && child.children.is_empty() {
                    node.children.remove(first);
                }
                removed
            }
        }
    }

    /// Enumerate all bindings as (key sequence, command), in no particular order.
    pub fn bindings(&self) -> Vec<(Vec<Key>, String)> {
        let mut out = Vec::new();
        Self::collect(&self.root, &mut Vec::new(), &mut out);
        out
    }

    fn collect(node: &TrieNode, prefix: &mut Vec<Key>, out: &mut Vec<(Vec<Key>, String)>) {
        if let Some(cmd) = &node.command {
            out.push((prefix.clone(), cmd.clone()));
        }
        for (key, child) in &node.children {
            prefix.push(key.clone());
            Self::collect(child, prefix, out);
            prefix.pop();
        }
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
    fn insert_checked_replaces_exact_binding() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("j")], "scroll down".into());
        assert!(
            t.insert_checked(&[Key::plain("j")], "scroll up".into())
                .is_ok()
        );
        assert_eq!(
            t.lookup(&[Key::plain("j")]),
            TrieMatch::Exact("scroll up".into())
        );
    }

    #[test]
    fn insert_checked_rejects_prefix_and_extension() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("g"), Key::plain("g")], "top".into());
        // `g` is a prefix of the existing `gg`.
        assert!(t.insert_checked(&[Key::plain("g")], "x".into()).is_err());
        // `ggg` extends the existing `gg`.
        assert!(
            t.insert_checked(
                &[Key::plain("g"), Key::plain("g"), Key::plain("g")],
                "x".into()
            )
            .is_err()
        );
        // The original binding is untouched.
        assert_eq!(
            t.lookup(&[Key::plain("g"), Key::plain("g")]),
            TrieMatch::Exact("top".into())
        );
    }

    #[test]
    fn remove_prunes_so_prefix_is_unbound() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("g"), Key::plain("g")], "top".into());
        assert!(t.remove(&[Key::plain("g"), Key::plain("g")]));
        // After removal the `g` prefix no longer matches anything.
        assert_eq!(t.lookup(&[Key::plain("g")]), TrieMatch::None);
        assert_eq!(
            t.lookup(&[Key::plain("g"), Key::plain("g")]),
            TrieMatch::None
        );
        // Removing a missing binding reports false.
        assert!(!t.remove(&[Key::plain("z")]));
    }

    #[test]
    fn bindings_enumerates_all() {
        let mut t = BindingTrie::new();
        t.insert(&[Key::plain("j")], "down".into());
        t.insert(&[Key::plain("g"), Key::plain("g")], "top".into());
        let mut got = t.bindings();
        got.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1, "down");
        assert_eq!(got[1].1, "top");
        assert_eq!(got[1].0, vec![Key::plain("g"), Key::plain("g")]);
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
