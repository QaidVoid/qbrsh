//! GTK-independent key representation used by bindings and the trie.
//!
//! The windowing layer translates raw GDK key events into [`Key`] values, so the
//! binding trie and `update` stay free of any toolkit dependency and remain
//! unit-testable. For printable keys the symbol carries the shifted form (`G`,
//! `:`), so Shift is not tracked separately; for named keys (Tab, F1) it is.

use std::fmt;

/// A single key press: a normalized symbol plus modifier state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// Normalized symbol: a single printable character (`"j"`, `"G"`, `":"`) or a
    /// canonical name (`"Escape"`, `"Tab"`, `"F1"`).
    pub sym: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Key {
    /// A key with no modifiers.
    pub fn plain(sym: impl Into<String>) -> Self {
        Self {
            sym: sym.into(),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Whether this is a bare ASCII digit usable as a count prefix.
    pub fn is_count_digit(&self) -> bool {
        !self.ctrl
            && !self.alt
            && self.sym.len() == 1
            && self.sym.chars().next().is_some_and(|c| c.is_ascii_digit())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.ctrl && !self.alt && !self.shift {
            return write!(f, "{}", self.sym);
        }
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("C");
        }
        if self.alt {
            parts.push("A");
        }
        if self.shift {
            parts.push("S");
        }
        write!(f, "<{}-{}>", parts.join("-"), self.sym)
    }
}

/// Render a key sequence for display in the status bar.
pub fn display_sequence(keys: &[Key]) -> String {
    keys.iter().map(|k| k.to_string()).collect()
}

/// Parse a binding string like `"gg"`, `"<C-f>"`, `"<C-S-t>"`, `"<A-1>"` into keys.
pub fn parse_key_string(s: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c == '<' {
            chars.next();
            let mut inner = String::new();
            for ch in chars.by_ref() {
                if ch == '>' {
                    break;
                }
                inner.push(ch);
            }
            let parts: Vec<&str> = inner.split('-').collect();
            let (mods, name) = parts.split_at(parts.len().saturating_sub(1));
            let mut key = match name.first().and_then(|n| canonical_name(n)) {
                Some(sym) => Key::plain(sym),
                None => continue,
            };
            for &m in mods {
                match m {
                    "C" => key.ctrl = true,
                    "A" => key.alt = true,
                    "S" => key.shift = true,
                    _ => {}
                }
            }
            keys.push(key);
        } else {
            chars.next();
            keys.push(Key::plain(c.to_string()));
        }
    }
    keys
}

/// Normalize a key name (from a binding string) to its canonical symbol.
/// Single printable characters are returned as-is, preserving case.
pub fn canonical_name(name: &str) -> Option<String> {
    let sym = match name.to_lowercase().as_str() {
        "escape" | "esc" => "Escape",
        "return" | "enter" | "cr" => "Return",
        "tab" => "Tab",
        "space" => "space",
        "backspace" | "bs" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" => "Insert",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "pgup" | "pageup" => "PgUp",
        "pgdown" | "pagedown" => "PgDown",
        "home" => "Home",
        "end" => "End",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        _ => {
            // A single printable character keeps its original case.
            if name.chars().count() == 1 {
                return Some(name.to_string());
            }
            return None;
        }
    };
    Some(sym.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_sequence() {
        let keys = parse_key_string("gg");
        assert_eq!(keys, vec![Key::plain("g"), Key::plain("g")]);
    }

    #[test]
    fn parse_ctrl() {
        let keys = parse_key_string("<C-f>");
        assert_eq!(keys.len(), 1);
        assert!(keys[0].ctrl);
        assert_eq!(keys[0].sym, "f");
    }

    #[test]
    fn parse_alt_digit() {
        let keys = parse_key_string("<A-1>");
        assert_eq!(keys.len(), 1);
        assert!(keys[0].alt);
        assert_eq!(keys[0].sym, "1");
    }

    #[test]
    fn parse_named_in_mods() {
        let keys = parse_key_string("<C-Tab>");
        assert_eq!(keys.len(), 1);
        assert!(keys[0].ctrl);
        assert_eq!(keys[0].sym, "Tab");
    }

    #[test]
    fn uppercase_preserved() {
        let keys = parse_key_string("G");
        assert_eq!(keys, vec![Key::plain("G")]);
    }

    #[test]
    fn count_digit_detection() {
        assert!(Key::plain("5").is_count_digit());
        assert!(!Key::plain("j").is_count_digit());
        let alt5 = Key {
            sym: "5".into(),
            alt: true,
            ctrl: false,
            shift: false,
        };
        assert!(!alt5.is_count_digit());
    }

    #[test]
    fn display_roundtrip() {
        assert_eq!(Key::plain("j").to_string(), "j");
        assert_eq!(
            Key {
                sym: "f".into(),
                ctrl: true,
                alt: false,
                shift: false
            }
            .to_string(),
            "<C-f>"
        );
    }
}
