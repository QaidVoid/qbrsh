//! Default Normal-mode keybindings.
//!
//! Only bindings whose commands are implemented are included here; the table
//! grows as commands (hints, search, bookmarks, zoom, …) are added in their
//! respective subsystems.

use crate::core::key::parse_key_string;
use crate::core::trie::BindingTrie;

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
    bind("t", "cmd-set-text :tabopen ");
    for i in 1..=9 {
        bind(&format!("<A-{i}>"), &format!("tab-focus {i}"));
    }

    // Open URL / command line
    bind("o", "cmd-set-text :open ");
    bind("O", "cmd-set-text :open {url}");
    bind(":", "cmd-set-text :");

    // Yank
    bind("yy", "yank");
    bind("yt", "yank title");

    // Mode switching
    bind("i", "mode-enter insert");
    bind("Escape", "mode-leave");

    trie
}
