//! Turns what the user typed into the location field into a URL.

use std::net::IpAddr;

use url::Url;

/// The URL schemes that can be opened from the location field.
const ALLOWED_SCHEMES: [&str; 3] = ["http", "https", "about"];

/// Interprets location field text as a URL, or returns `None` if it isn't one.
///
/// Text with no scheme is treated as a host name, using HTTPS unless the host
/// is local, since local development servers rarely have certificates.
pub fn parse_location(input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() || input.contains(char::is_whitespace) {
        return None;
    }

    let has_scheme = input.contains("://") || input.starts_with("about:");
    let url = if has_scheme {
        Url::parse(input).ok()?
    } else {
        let https = Url::parse(&format!("https://{input}")).ok()?;
        if is_local_host(&https) {
            Url::parse(&format!("http://{input}")).ok()?
        } else {
            https
        }
    };

    ALLOWED_SCHEMES.contains(&url.scheme()).then_some(url)
}

fn is_local_host(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
            .is_ok(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_location;

    fn parsed(input: &str) -> Option<String> {
        parse_location(input).map(String::from)
    }

    #[test]
    fn keeps_explicit_web_urls() {
        assert_eq!(
            parsed("http://example.com/a?b=c").unwrap(),
            "http://example.com/a?b=c"
        );
        assert_eq!(
            parsed(" https://example.com/ ").unwrap(),
            "https://example.com/"
        );
    }

    #[test]
    fn defaults_to_https_for_bare_hosts() {
        assert_eq!(parsed("example.com").unwrap(), "https://example.com/");
        assert_eq!(
            parsed("example.com/path").unwrap(),
            "https://example.com/path"
        );
        assert_eq!(
            parsed("example.com:8443").unwrap(),
            "https://example.com:8443/"
        );
    }

    #[test]
    fn defaults_to_http_for_local_hosts() {
        assert_eq!(
            parsed("localhost:8000/x").unwrap(),
            "http://localhost:8000/x"
        );
        assert_eq!(parsed("127.0.0.1:8000").unwrap(), "http://127.0.0.1:8000/");
        assert_eq!(parsed("[::1]:8000").unwrap(), "http://[::1]:8000/");
    }

    #[test]
    fn allows_about_pages() {
        assert_eq!(parsed("about:blank").unwrap(), "about:blank");
    }

    #[test]
    fn rejects_everything_else() {
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("   "), None);
        assert_eq!(parsed("two words"), None);
        assert_eq!(parsed("javascript:alert(1)"), None);
        assert_eq!(parsed("file:///etc/passwd"), None);
        assert_eq!(parsed("ftp://example.com/"), None);
    }
}
