//! Domain-level ad and tracker blocking.
//!
//! Blocking happens synchronously in the engine's navigation-policy handler (a
//! hot path, kept native per design D5 and never routed through the message loop).
//! This blocks navigations and subframe (iframe) loads to blocked domains, which
//! covers ad frames, popups, and tracker redirects. A built-in domain set is
//! merged with an optional user list at `~/.local/share/qbrsh/adblock`.
//!
//! Comprehensive per-subresource blocking (full filter lists via WebKit content
//! filters) is a future enhancement; this is the native MVP.

use std::collections::HashSet;
use std::path::Path;

/// Well-known ad and tracker domains blocked by default.
const DEFAULT_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "googletagservices.com",
    "adservice.google.com",
    "amazon-adsystem.com",
    "adnxs.com",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "scorecardresearch.com",
    "quantserve.com",
    "pubmatic.com",
    "rubiconproject.com",
    "openx.net",
    "casalemedia.com",
    "moatads.com",
    "zedo.com",
    "adcolony.com",
    "applovin.com",
    "chartbeat.com",
    "hotjar.com",
    "mixpanel.com",
    "segment.com",
    "branch.io",
    "connect.facebook.net",
    "ads.yahoo.com",
    "ads.linkedin.com",
    "bat.bing.com",
];

/// Load the blocklist: built-in domains plus any from the user file (one domain
/// per line, `#` comments allowed).
pub fn load(user_file: &Path) -> HashSet<String> {
    let mut set: HashSet<String> = DEFAULT_DOMAINS.iter().map(|s| s.to_string()).collect();
    if let Ok(text) = std::fs::read_to_string(user_file) {
        for line in text.lines() {
            let domain = line.trim();
            if !domain.is_empty() && !domain.starts_with('#') {
                set.insert(domain.to_string());
            }
        }
    }
    set
}

/// Extract the host from a URI without pulling in a URL parser.
pub fn host_of(uri: &str) -> Option<&str> {
    let after_scheme = uri.split("://").nth(1)?;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?; // drop any userinfo
    let host = host.split(':').next()?; // drop any port
    if host.is_empty() { None } else { Some(host) }
}

/// Whether `uri`'s host matches a blocked domain (exact or subdomain).
pub fn is_blocked(uri: &str, blocklist: &HashSet<String>) -> bool {
    let Some(host) = host_of(uri) else {
        return false;
    };
    blocklist
        .iter()
        .any(|d| host == d || host.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> HashSet<String> {
        ["doubleclick.net".to_string()].into_iter().collect()
    }

    #[test]
    fn extracts_host() {
        assert_eq!(host_of("https://ad.doubleclick.net/foo?x=1"), Some("ad.doubleclick.net"));
        assert_eq!(host_of("http://user@example.com:8080/p"), Some("example.com"));
        assert_eq!(host_of("about:blank"), None);
    }

    #[test]
    fn blocks_domain_and_subdomains() {
        assert!(is_blocked("https://doubleclick.net/x", &list()));
        assert!(is_blocked("https://ad.doubleclick.net/x", &list()));
        assert!(!is_blocked("https://example.com/x", &list()));
        // Must not match a domain that merely ends with the same letters.
        assert!(!is_blocked("https://notdoubleclick.net/x", &list()));
    }
}
