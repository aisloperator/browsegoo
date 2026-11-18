//! Keeps Web pages from reaching this computer and its local network, unless
//! they're on the same local address themselves.
//!
//! A page can always reach public addresses, and a page from a local address,
//! such as a development server on `localhost:3000`, can also reach that same
//! host and port, but no other local one.
//!
//! This is done in three parts, as WebKit has no hook for each request:
//!
//! - A content blocker rule list, which WebKit applies to every request,
//!   blocks URLs whose host is a local address or name, except from a frame on
//!   that same host and port. The hosts and ports that are allowed are added
//!   as the user goes to them.
//! - Pages going to a local address in the whole tab is decided in the
//!   browser, which only allows it when the user asks for it, or from the same
//!   host and port.
//! - Pages could still reach local addresses through public names that
//!   resolve to them, as with DNS rebinding, so WebKit connects through
//!   BrowseGoo's own proxy (`socks_proxy`), which refuses those.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

/// The identifier of the content blocker rule list.
pub const FILTER_ID: &str = "browsegoo-local-network";

/// Top-level domains, and names, that are only used for local networks or
/// this computer.
const LOCAL_SUFFIXES: [&str; 7] = [
    "localhost",
    "local",
    "lan",
    "home.arpa",
    "internal",
    "localdomain",
    "test",
];

/// Whether `address` is on the public Internet, rather than this computer, a
/// local or private network, or otherwise special.
pub fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    !(address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_multicast()
        // 0.0.0.0/8, which reaches this computer on Linux.
        || a == 0
        // Shared address space, for carrier-grade NAT and some VPNs.
        || (a == 100 && (64..=127).contains(&b))
        // Benchmarking, and reserved.
        || (a == 198 && (18..=19).contains(&b))
        || a >= 240)
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(v4) = address.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segments = address.segments();
    // IPv4-compatible addresses, such as ::127.0.0.1, and :: and ::1.
    if segments[..6] == [0; 6] {
        return false;
    }
    !(address.is_multicast()
        // Unique local, fc00::/7.
        || (segments[0] & 0xfe00) == 0xfc00
        // Link-local fe80::/10, and the old site-local fec0::/10.
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0)
}

/// Whether `name` is only used for this computer or a local network, such as
/// `localhost`, `printer.local` or a name with no dots, such as `router`.
pub fn is_local_name(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    !name.contains('.')
        || LOCAL_SUFFIXES.iter().any(|suffix| {
            name.strip_suffix(suffix)
                .is_some_and(|rest| rest.ends_with('.'))
        })
}

/// Whether the URL's host is this computer or on a local network.
pub fn is_local_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => !is_public_v4(address),
        Some(Host::Ipv6(address)) => !is_public_v6(address),
        Some(Host::Domain(name)) => {
            matches!(url.scheme(), "http" | "https" | "ws" | "wss") && is_local_name(name)
        }
        None => false,
    }
}

/// A local host and port that pages from it can reach.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalOrigin {
    /// As in URLs, such as `localhost`, `192.168.1.1` or `[::1]`.
    host: String,
    port: u16,
}

impl LocalOrigin {
    /// The local origin of `url`, if it's local.
    pub fn of(url: &Url) -> Option<Self> {
        if !is_local_url(url) {
            return None;
        }
        Some(Self {
            host: url.host_str()?.to_ascii_lowercase(),
            port: url.port_or_known_default()?,
        })
    }

    /// Patterns for the start of URLs on this host and port, as WebKit
    /// writes them, which leaves out the scheme's default port.
    fn url_patterns(&self) -> Vec<String> {
        let host = escape_regex(&self.host);
        let with_default = |schemes: [&str; 2]| {
            schemes
                .iter()
                .map(|scheme| format!("^{scheme}://{host}/"))
                .collect::<Vec<_>>()
        };
        let mut patterns = match self.port {
            80 => with_default(["http", "ws"]),
            443 => with_default(["https", "wss"]),
            _ => Vec::new(),
        };
        patterns.push(format!("^{SCHEME}{host}:{}/", self.port));
        patterns
    }
}

/// Any scheme, then `://`.
const SCHEME: &str = "[a-z][a-z0-9+.-]*://";

/// The start of a URL, up to its host, allowing for a user name.
fn before_host() -> String {
    format!("^{SCHEME}([^/]*@)?")
}

/// Patterns for URLs whose host is local, without alternatives (`|`), which
/// WebKit's content blockers don't support. They must agree with
/// `is_local_url`.
fn local_url_patterns() -> Vec<String> {
    let octet = "[0-9]+";
    let rest = |count: usize| format!("{}[:/]", vec![octet; count].join("\\."));
    let ipv4 = [
        format!("127\\.{}", rest(3)),
        format!("10\\.{}", rest(3)),
        format!("192\\.168\\.{}", rest(2)),
        format!("169\\.254\\.{}", rest(2)),
        format!("172\\.1[6-9]\\.{}", rest(2)),
        format!("172\\.2[0-9]\\.{}", rest(2)),
        format!("172\\.3[01]\\.{}", rest(2)),
        format!("0\\.{}", rest(3)),
        format!("100\\.6[4-9]\\.{}", rest(2)),
        format!("100\\.[7-9][0-9]\\.{}", rest(2)),
        format!("100\\.1[01][0-9]\\.{}", rest(2)),
        format!("100\\.12[0-7]\\.{}", rest(2)),
        format!("198\\.1[89]\\.{}", rest(2)),
        // Multicast, and reserved: 224 and above.
        format!("22[4-9]\\.{}", rest(3)),
        format!("23[0-9]\\.{}", rest(3)),
        format!("24[0-9]\\.{}", rest(3)),
        format!("25[0-5]\\.{}", rest(3)),
    ];
    let ipv6 = [
        // ::, ::1, IPv4-mapped and IPv4-compatible addresses.
        "\\[::".to_owned(),
        // Unique local.
        "\\[f[cd][0-9a-f]*:".to_owned(),
        // Link-local, site-local and multicast.
        "\\[fe[89a-f][0-9a-f]:".to_owned(),
        "\\[ff[0-9a-f]*:".to_owned(),
    ];
    let names = std::iter::once("[^./:@\\[]+\\.?[:/]".to_owned()).chain(
        LOCAL_SUFFIXES.iter().flat_map(|suffix| {
            let suffix = escape_regex(suffix);
            [
                format!("{suffix}\\.?[:/]"),
                format!("[^/]*\\.{suffix}\\.?[:/]"),
            ]
        }),
    );
    ipv4.into_iter()
        .chain(ipv6)
        .chain(names)
        .map(|pattern| format!("{}{pattern}", before_host()))
        .collect()
}

fn escape_regex(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn json_string(text: &str) -> String {
    let mut json = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            c if (c as u32) < 0x20 => json.push_str(&format!("\\u{:04x}", c as u32)),
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

fn json_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| json_string(item)).collect();
    format!("[{}]", items.join(","))
}

/// The content blocker rules, in WebKit's JSON format: requests to local
/// addresses are blocked, except to `allowed` hosts and ports, from frames on
/// them, or for frames in a page on them.
pub fn rules(allowed: &[LocalOrigin]) -> String {
    let mut rules: Vec<String> = local_url_patterns()
        .iter()
        .map(|pattern| {
            format!(
                "{{\"trigger\":{{\"url-filter\":{}}},\"action\":{{\"type\":\"block\"}}}}",
                json_string(pattern)
            )
        })
        .collect();
    for origin in allowed {
        let patterns = origin.url_patterns();
        let same = json_list(&patterns);
        for pattern in &patterns {
            let url = json_string(pattern);
            rules.push(format!(
                "{{\"trigger\":{{\"url-filter\":{url},\"if-frame-url\":{same}}},\
                 \"action\":{{\"type\":\"ignore-previous-rules\"}}}}"
            ));
            // A frame's own load is in its parent, which isn't this frame.
            rules.push(format!(
                "{{\"trigger\":{{\"url-filter\":{url},\"resource-type\":[\"document\"],\
                 \"if-top-url\":{same}}},\"action\":{{\"type\":\"ignore-previous-rules\"}}}}"
            ));
        }
    }
    format!("[{}]", rules.join(",\n"))
}

/// Whether a tab can go to `target`. A local address is only allowed when the
/// user asked for it, or from a page on the same host and port, such as the
/// tab's own page, or the one that opened it.
pub fn may_navigate(target: &Url, user_asked: bool, from: &[Option<Url>]) -> bool {
    let Some(origin) = LocalOrigin::of(target) else {
        return true;
    };
    user_asked
        || from
            .iter()
            .flatten()
            .any(|url| LocalOrigin::of(url).as_ref() == Some(&origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn public_and_local_addresses() {
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.0.1",
            "100.128.0.1",
            "2001:4860:4860::8888",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip(public)), "{public} is public");
        }
        for local in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "169.254.1.1",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.1",
            "::127.0.0.1",
            "fd00::1",
            "fc12::1",
            "fe80::1",
            "febf::1",
            "ff02::1",
        ] {
            assert!(!is_public_ip(ip(local)), "{local} is local");
        }
    }

    #[test]
    fn local_names() {
        for local in [
            "localhost",
            "LOCALHOST.",
            "app.localhost",
            "router",
            "printer.local",
            "nas.lan",
            "box.home.arpa",
            "db.internal",
            "host.localdomain",
            "site.test",
        ] {
            assert!(is_local_name(local), "{local} is local");
        }
        for public in [
            "example.com",
            "localhost.example.com",
            "local.example.com",
            "notlocal",
            "x.testing",
        ] {
            assert_eq!(is_local_name(public), public == "notlocal", "{public}");
        }
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn local_urls() {
        assert!(is_local_url(&url("http://127.0.0.1:8000/")));
        assert!(is_local_url(&url("http://2130706433/")));
        assert!(is_local_url(&url("http://[::1]/")));
        assert!(is_local_url(&url("ws://localhost:3000/hmr")));
        assert!(is_local_url(&url("https://intranet/")));
        assert!(!is_local_url(&url("https://example.com/")));
        assert!(!is_local_url(&url("http://8.8.8.8/")));
        assert!(!is_local_url(&url("about:blank")));
        assert!(!is_local_url(&url("file:///etc/passwd")));
    }

    #[test]
    fn origins_include_the_default_port() {
        let origin = |text| LocalOrigin::of(&url(text));
        assert_eq!(origin("http://localhost/"), origin("http://LocalHost:80/x"));
        assert_ne!(origin("http://localhost/"), origin("https://localhost/"));
        assert_eq!(origin("https://localhost/"), origin("wss://localhost:443/"));
        assert_eq!(origin("https://example.com/"), None);
        assert_eq!(
            origin("http://[::1]:8080/").unwrap().url_patterns(),
            ["^[a-z][a-z0-9+.-]*://\\[::1\\]:8080/"]
        );
        assert_eq!(
            origin("http://192.168.1.1/").unwrap().url_patterns(),
            [
                "^http://192\\.168\\.1\\.1/",
                "^ws://192\\.168\\.1\\.1/",
                "^[a-z][a-z0-9+.-]*://192\\.168\\.1\\.1:80/"
            ]
        );
    }

    #[test]
    fn navigating_to_local_addresses() {
        let local = url("http://localhost:3000/page");
        let same = Some(url("http://localhost:3000/other"));
        let other_port = Some(url("http://localhost:3001/"));
        let public = Some(url("https://example.com/"));
        assert!(may_navigate(
            &url("https://example.com/"),
            false,
            std::slice::from_ref(&public)
        ));
        assert!(may_navigate(&local, true, std::slice::from_ref(&public)));
        assert!(may_navigate(&local, false, &[public.clone(), same.clone()]));
        assert!(!may_navigate(&local, false, &[public, None]));
        assert!(!may_navigate(&local, false, &[other_port]));
        assert!(!may_navigate(&local, false, &[]));
    }

    #[test]
    fn rules_are_json_without_alternatives() {
        let allowed = [LocalOrigin::of(&url("http://localhost:3000/")).unwrap()];
        let rules = rules(&allowed);
        assert!(rules.starts_with('[') && rules.ends_with(']'));
        assert!(!rules.contains('|'));
        assert!(rules.contains("\"if-frame-url\":[\"^[a-z][a-z0-9+.-]*://localhost:3000/\"]"));
        assert_eq!(rules.matches("ignore-previous-rules").count(), 2);
    }
}
