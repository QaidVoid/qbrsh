//! Configuration file loading.
//!
//! Reads `~/.config/qbrsh/config.toml` (or `$XDG_CONFIG_HOME`), falling back to
//! defaults when the file is missing or invalid. Reload is command-driven
//! (`:config-source`); there is no file watcher.

use std::path::{Path, PathBuf};

use crate::core::state::{Config, Permissions};

/// Path to the user config file.
pub fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "qbrsh").map(|p| p.config_dir().join("config.toml"))
}

/// Load the runtime permission store (per-site grants saved by the prompt and the
/// management view), if present. This is data-dir state, separate from the
/// user-authored `config.toml` `[permissions]`.
pub fn load_permissions(path: &Path) -> Option<Permissions> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Persist the runtime permission store to `path`.
pub fn save_permissions(path: &Path, permissions: &Permissions) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = toml::to_string_pretty(permissions) {
        let _ = std::fs::write(path, text);
    }
}

/// Load configuration, falling back to defaults on any error.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("[qbrsh] config: {}: {e}", path.display());
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::state::Config;

    #[test]
    fn partial_toml_fills_defaults() {
        let toml = "homepage = \"https://x.test\"\n[colors]\naccent = \"#abcdef\"\n";
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.homepage, "https://x.test");
        assert_eq!(c.colors.accent, "#abcdef");
        // Unspecified fields keep their defaults.
        assert_eq!(c.colors.background, Config::default().colors.background);
        assert_eq!(c.font.size, Config::default().font.size);
    }

    #[test]
    fn set_updates_known_keys_and_rejects_unknown() {
        let mut c = Config::default();
        assert!(c.set("colors.accent", "#123456").is_ok());
        assert_eq!(c.colors.accent, "#123456");
        assert!(c.set("font.size", "14").is_ok());
        assert_eq!(c.font.size, 14);
        assert!(c.set("font.size", "huge").is_err());
        assert!(c.set("nope.key", "x").is_err());
    }

    #[test]
    fn per_site_permission_lookup() {
        use crate::core::state::{Capability, PermissionPolicy};
        let cam = Capability::Camera;
        let geo = Capability::Geolocation;
        let mut c = Config::default();
        // Default policy is ask (the prompt decides).
        assert_eq!(
            c.permissions.policy_for("example.com", cam),
            PermissionPolicy::Ask
        );
        // A bare host rule applies to every capability.
        assert!(c.set("permissions.github.com", "allow").is_ok());
        assert_eq!(
            c.permissions.policy_for("github.com", cam),
            PermissionPolicy::Allow
        );
        // Subdomains inherit the site rule by suffix.
        assert_eq!(
            c.permissions.policy_for("gist.github.com", geo),
            PermissionPolicy::Allow
        );
        assert_eq!(
            c.permissions.policy_for("other.com", cam),
            PermissionPolicy::Ask
        );
        // A per-capability rule is independent of other capabilities.
        assert!(c.set("permissions.example.org.geolocation", "allow").is_ok());
        assert_eq!(
            c.permissions.policy_for("example.org", geo),
            PermissionPolicy::Allow
        );
        // Camera has no rule, so it falls back to the default (ask).
        assert_eq!(
            c.permissions.policy_for("example.org", cam),
            PermissionPolicy::Ask
        );
        assert!(c.set("permissions.default", "ask").is_ok());
        assert_eq!(c.permissions.default, PermissionPolicy::Ask);
        assert!(c.set("permissions.x", "bogus").is_err());
    }
}
