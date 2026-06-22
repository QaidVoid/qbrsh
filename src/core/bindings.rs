//! Default Normal-mode keybindings.
//!
//! Only bindings whose commands are implemented are included here; the table
//! grows as commands (hints, search, bookmarks, zoom, …) are added in their
//! respective subsystems.

use std::collections::BTreeMap;

use crate::core::key::parse_key_string;
use crate::core::trie::BindingTrie;

/// Build the effective binding trie from the built-in defaults overlaid with the
/// user `[bindings]` overrides. Returns the trie and a list of error messages
/// for entries that could not be applied (unparseable keys or conflicts); those
/// entries are skipped so the rest of the configuration still loads.
pub fn build_bindings(overrides: &BTreeMap<String, String>) -> (BindingTrie, Vec<String>) {
    let mut trie = default_bindings();
    let mut errors = Vec::new();
    for (keys, command) in overrides {
        let parsed = parse_key_string(keys);
        if parsed.is_empty() {
            errors.push(format!("invalid binding key: {keys}"));
            continue;
        }
        if let Err(e) = trie.insert_checked(&parsed, command.clone()) {
            errors.push(format!("binding '{keys}' {e}"));
        }
    }
    (trie, errors)
}

/// Build the default binding trie.
pub fn default_bindings() -> BindingTrie {
    let mut trie = BindingTrie::new();
    let mut bind = |keys: &str, cmd: &str| trie.insert(&parse_key_string(keys), cmd.to_string());

    // Scrolling
    bind("h", "scroll left");
    bind("j", "scroll down");
    bind("k", "scroll up");
    bind("l", "scroll right");
    bind("gg", "scroll-to-perc 0");
    bind("G", "scroll-to-perc 100");
    bind("<C-f>", "scroll-page down");
    bind("<C-b>", "scroll-page up");
    bind("<C-d>", "scroll-page down half");
    bind("<C-u>", "scroll-page up half");

    // Navigation
    bind("H", "back");
    bind("L", "forward");
    bind("r", "reload");
    bind("R", "reload --force");

    // Tabs
    bind("J", "tab-next");
    bind("K", "tab-prev");
    bind("d", "tab-close");
    bind("u", "undo");
    bind("gC", "tab-clone");
    bind("gJ", "tab-move +1");
    bind("gK", "tab-move -1");
    bind("co", "tab-only");
    bind("t", "cmd-set-text :tabopen ");
    for i in 1..=9 {
        bind(&format!("<A-{i}>"), &format!("tab-focus {i}"));
    }

    // Hints
    bind("f", "hint");
    bind("F", "hint-tab");

    // Open URL / command line
    bind("o", "cmd-set-text :open ");
    bind("O", "cmd-set-text :open {url}");
    bind(":", "cmd-set-text :");

    // Yank
    bind("yy", "yank");
    bind("yt", "yank title");

    // Bookmarks and quickmarks
    bind("M", "bookmark-add");
    bind("m", "cmd-set-text :quickmark-save ");
    bind("b", "cmd-set-text :quickmark-load ");
    bind("gb", "cmd-set-text :bookmark-load ");

    // Find. `n` steps forward (wrapping); `N` is a best-effort backward step
    // (WebKit's backward search is unreliable, see EngineView::find_previous).
    bind("/", "cmd-set-text /");
    bind("n", "find-next");
    bind("N", "find-prev");

    // Zoom
    bind("zi", "zoom-in");
    bind("zo", "zoom-out");
    bind("zz", "zoom-reset");

    // Content toggles live under the `,` leader: `t` is itself bound (tabopen),
    // so it cannot also be a binding prefix. The leader has no command of its
    // own, so the sequence resolves without ambiguity.
    bind(",d", "darkmode");
    bind(",j", "js-toggle");
    bind(",t", "tabs-toggle");
    bind(",p", "cmd-set-text :private ");

    // Panes (vim-style <C-w> prefix)
    bind("<C-w>s", "split");
    bind("<C-w>v", "vsplit");
    bind("<C-w>c", "close-pane");
    bind("<C-w>o", "only-pane");
    bind("<C-w>w", "focus-pane");
    bind("<C-w>W", "focus-pane-prev");

    // Mode switching
    bind("i", "mode-enter insert");
    bind("Escape", "mode-leave");

    trie
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::trie::TrieMatch;

    #[test]
    fn pane_prefix_bindings_resolve() {
        let trie = default_bindings();
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>s")),
            TrieMatch::Exact("split".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>v")),
            TrieMatch::Exact("vsplit".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>c")),
            TrieMatch::Exact("close-pane".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>o")),
            TrieMatch::Exact("only-pane".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>w")),
            TrieMatch::Exact("focus-pane".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("<C-w>W")),
            TrieMatch::Exact("focus-pane-prev".to_string())
        );
        // The prefix alone is partial (no command yet).
        assert_eq!(trie.lookup(&parse_key_string("<C-w>")), TrieMatch::Partial);
    }

    #[test]
    fn override_replaces_default_and_keeps_others() {
        let mut overrides = BTreeMap::new();
        overrides.insert("j".to_string(), "scroll up".to_string());
        let (trie, errors) = build_bindings(&overrides);
        assert!(errors.is_empty());
        // The overridden key takes the new command.
        assert_eq!(
            trie.lookup(&parse_key_string("j")),
            TrieMatch::Exact("scroll up".to_string())
        );
        // Other defaults are untouched.
        assert_eq!(
            trie.lookup(&parse_key_string("k")),
            TrieMatch::Exact("scroll up".to_string())
        );
        assert_eq!(
            trie.lookup(&parse_key_string("l")),
            TrieMatch::Exact("scroll right".to_string())
        );
    }

    #[test]
    fn invalid_and_conflicting_overrides_are_reported_and_skipped() {
        let mut overrides = BTreeMap::new();
        // `g` conflicts with the default multi-key `gg`.
        overrides.insert("g".to_string(), "scroll down".to_string());
        // `<bogus>` does not parse to any key.
        overrides.insert("<bogus>".to_string(), "nop".to_string());
        let (trie, errors) = build_bindings(&overrides);
        assert_eq!(errors.len(), 2);
        // The default `gg` survives the rejected conflict.
        assert_eq!(
            trie.lookup(&parse_key_string("gg")),
            TrieMatch::Exact("scroll-to-perc 0".to_string())
        );
    }
}
