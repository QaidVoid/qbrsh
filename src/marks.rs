//! Flat-file persistence for quickmarks and bookmarks.
//!
//! Quickmarks are `name<space>url` per line; bookmarks are `url<space>title`
//! per line. The files are tiny, so reads happen once at startup and writes are
//! full rewrites on change.

use std::fs;
use std::path::Path;

/// Load quickmarks as (name, url) pairs. Missing or unreadable files yield none.
pub fn load_quickmarks(path: &Path) -> Vec<(String, String)> {
    read_pairs(path)
}

/// Load bookmarks as (url, title) pairs.
pub fn load_bookmarks(path: &Path) -> Vec<(String, String)> {
    read_pairs(path)
}

/// Write quickmarks (name, url) to disk.
pub fn save_quickmarks(path: &Path, entries: &[(String, String)]) {
    write_pairs(path, entries);
}

/// Write bookmarks (url, title) to disk.
pub fn save_bookmarks(path: &Path, entries: &[(String, String)]) {
    write_pairs(path, entries);
}

fn read_pairs(path: &Path) -> Vec<(String, String)> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (a, b) = line.split_once(char::is_whitespace)?;
            Some((a.to_string(), b.trim().to_string()))
        })
        .collect()
}

fn write_pairs(path: &Path, entries: &[(String, String)]) {
    let body: String = entries.iter().map(|(a, b)| format!("{a} {b}\n")).collect();
    if let Err(e) = fs::write(path, body) {
        eprintln!("[qbrsh] could not write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_pairs() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("qbrsh-marks-test-{}", std::process::id()));
        let entries = vec![
            ("gh".to_string(), "https://github.com".to_string()),
            ("rs".to_string(), "https://rust-lang.org".to_string()),
        ];
        save_quickmarks(&path, &entries);
        assert_eq!(load_quickmarks(&path), entries);
        let _ = fs::remove_file(&path);
    }
}
