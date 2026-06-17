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

/// Best-effort site key (registrable domain) for grouping tabs into web
/// processes: the host reduced to its last two labels, or the host itself for IP
/// literals and single-label hosts. Without a public-suffix list this over-splits
/// some multi-label TLDs, which is the safe direction (more isolation, not less).
pub fn site_of(uri: &str) -> Option<String> {
    let host = host_of(uri)?;
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Some(host.to_string());
    }
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    if n >= 2 {
        Some(format!("{}.{}", labels[n - 2], labels[n - 1]))
    } else {
        Some(host.to_string())
    }
}

/// Build a WebKit content-blocker rule list (JSON) from the blocklist, so the
/// same domains are blocked at the subresource level (images, scripts, XHR),
/// not only at navigation. Each rule blocks URLs whose host is, or is a
/// subdomain of, a blocked domain.
pub fn content_filter_json(blocklist: &HashSet<String>) -> String {
    let rules: Vec<serde_json::Value> = blocklist
        .iter()
        .map(|domain| {
            let escaped = domain.replace('.', r"\.");
            serde_json::json!({
                "trigger": { "url-filter": format!(r"https?://([^/]+\.)?{escaped}[:/]") },
                "action": { "type": "block" }
            })
        })
        .collect();
    serde_json::to_string(&rules).unwrap_or_else(|_| "[]".to_string())
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
    fn content_filter_is_valid_json_rules() {
        let set: HashSet<String> = ["doubleclick.net".to_string()].into_iter().collect();
        let json = content_filter_json(&set);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["action"]["type"], "block");
        assert!(
            arr[0]["trigger"]["url-filter"]
                .as_str()
                .unwrap()
                .contains(r"doubleclick\.net")
        );
    }

    #[test]
    fn site_key_is_registrable_domain() {
        assert_eq!(site_of("https://www.example.com/x").as_deref(), Some("example.com"));
        assert_eq!(site_of("https://example.com").as_deref(), Some("example.com"));
        assert_eq!(site_of("https://a.b.example.com").as_deref(), Some("example.com"));
        assert_eq!(site_of("http://127.0.0.1:8080/p").as_deref(), Some("127.0.0.1"));
        assert_eq!(site_of("about:blank"), None);
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
