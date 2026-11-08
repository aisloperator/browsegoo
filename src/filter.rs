//! Blocks network requests with the URL filter rules of Adblock Plus style
//! lists, such as EasyList.
//!
//! This only does the network part of those lists, and only the common parts of
//! their syntax. Rules that it can't apply correctly are skipped rather than
//! approximated: regular expressions, element hiding, scriptlets, redirects, and
//! options it doesn't know.

use std::collections::HashMap;

/// The kinds of resource that filter rules can be limited to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceType {
    Document,
    Subdocument,
    Script,
    Stylesheet,
    Image,
    Font,
    Media,
    Object,
    XmlHttpRequest,
    Ping,
    WebSocket,
    Other,
}

impl ResourceType {
    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

const ALL_TYPES: u16 = (1 << 12) - 1;
/// Rules with no type options apply to everything but whole pages.
const DEFAULT_TYPES: u16 = ALL_TYPES & !(1 << ResourceType::Document as u16);

/// A request that might be blocked.
pub struct Request<'a> {
    pub url: &'a str,
    /// The host of the page that made the request, if known.
    pub page_host: Option<&'a str>,
    /// The URL of the page that made the request, if known.
    pub page_url: Option<&'a str>,
    /// Whether the request is to a different site than the page.
    pub third_party: bool,
    pub resource_type: ResourceType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Anchor {
    None,
    /// `|`: the start of the URL.
    Start,
    /// `||`: the start of the host name, or of any of its subdomains.
    Host,
}

#[derive(Debug)]
struct Rule {
    anchor: Anchor,
    /// The pattern, with `*` and `^` wildcards, in lower case unless `match_case`.
    body: Vec<u8>,
    end_anchor: bool,
    match_case: bool,
    types: u16,
    third_party: Option<bool>,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
    important: bool,
}

/// What a line of a filter list turned out to be.
#[derive(Debug, PartialEq)]
enum Parsed {
    Block,
    Exception,
    /// An exception that turns off blocking on whole pages.
    PageException,
    /// Comments, element hiding rules, and so on.
    NotNetworkRule,
    Unsupported,
}

#[derive(Default)]
struct RuleSet {
    rules: Vec<Rule>,
    by_token: HashMap<u64, Vec<u32>>,
    /// Rules with no usable token, which have to be tried for every request.
    untokened: Vec<u32>,
}

impl RuleSet {
    fn add(&mut self, rule: Rule) {
        let index = self.rules.len() as u32;
        match best_token(&rule) {
            Some(token) => self.by_token.entry(token).or_default().push(index),
            None => self.untokened.push(index),
        }
        self.rules.push(rule);
    }

    /// Returns whether any rule matches, and whether any of those is important.
    fn find(&self, request: &Request, lower_url: &[u8], tokens: &[u64]) -> Option<bool> {
        let mut found = None;
        let candidates = tokens
            .iter()
            .filter_map(|token| self.by_token.get(token))
            .flatten()
            .chain(self.untokened.iter());
        for &index in candidates {
            let rule = &self.rules[index as usize];
            if rule.applies_to(request) && rule.matches_url(request.url.as_bytes(), lower_url) {
                if rule.important {
                    return Some(true);
                }
                found = Some(false);
            }
        }
        found
    }
}

#[derive(Default)]
pub struct Engine {
    blocking: RuleSet,
    exceptions: RuleSet,
    page_exceptions: RuleSet,
}

/// How many lines of a list were used.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ListStats {
    pub rules: usize,
    pub unsupported: usize,
}

impl Engine {
    pub fn rule_count(&self) -> usize {
        self.blocking.rules.len() + self.exceptions.rules.len() + self.page_exceptions.rules.len()
    }

    /// Adds the network rules of a filter list.
    pub fn add_list(&mut self, text: &str) -> ListStats {
        let mut stats = ListStats::default();
        for line in text.lines() {
            let (kind, rule) = parse_line(line);
            let set = match kind {
                Parsed::Block => &mut self.blocking,
                Parsed::Exception => &mut self.exceptions,
                Parsed::PageException => &mut self.page_exceptions,
                Parsed::NotNetworkRule => continue,
                Parsed::Unsupported => {
                    stats.unsupported += 1;
                    continue;
                }
            };
            set.add(rule.expect("every kind of network rule has a rule"));
            stats.rules += 1;
        }
        stats
    }

    pub fn should_block(&self, request: &Request) -> bool {
        if self.is_page_excepted(request) {
            return false;
        }
        let lower_url = request.url.to_ascii_lowercase().into_bytes();
        let tokens = url_tokens(&lower_url);
        match self.blocking.find(request, &lower_url, &tokens) {
            None => false,
            Some(true) => true,
            Some(false) => self.exceptions.find(request, &lower_url, &tokens).is_none(),
        }
    }

    fn is_page_excepted(&self, request: &Request) -> bool {
        let (Some(page_url), Some(page_host)) = (request.page_url, request.page_host) else {
            return false;
        };
        if self.page_exceptions.rules.is_empty() {
            return false;
        }
        let page = Request {
            url: page_url,
            page_host: Some(page_host),
            page_url: Some(page_url),
            third_party: false,
            resource_type: ResourceType::Document,
        };
        let lower_url = page_url.to_ascii_lowercase().into_bytes();
        let tokens = url_tokens(&lower_url);
        self.page_exceptions
            .find(&page, &lower_url, &tokens)
            .is_some()
    }
}

impl Rule {
    fn applies_to(&self, request: &Request) -> bool {
        if self.types & request.resource_type.bit() == 0 {
            return false;
        }
        if self
            .third_party
            .is_some_and(|third_party| third_party != request.third_party)
        {
            return false;
        }
        if !self.include_domains.is_empty() || !self.exclude_domains.is_empty() {
            let Some(page_host) = request.page_host else {
                return self.include_domains.is_empty();
            };
            let page_host = page_host.to_ascii_lowercase();
            let on = |domain: &String| is_same_or_subdomain(&page_host, domain);
            if self.exclude_domains.iter().any(on) {
                return false;
            }
            if !self.include_domains.is_empty() && !self.include_domains.iter().any(on) {
                return false;
            }
        }
        true
    }

    fn matches_url(&self, url: &[u8], lower_url: &[u8]) -> bool {
        let url = if self.match_case { url } else { lower_url };
        match self.anchor {
            Anchor::None => glob(&self.body, url, true, self.end_anchor),
            Anchor::Start => glob(&self.body, url, false, self.end_anchor),
            Anchor::Host => {
                let Some((start, end)) = host_range(url) else {
                    return false;
                };
                let label_starts = std::iter::once(start)
                    .chain((start..end).filter(|&i| url[i] == b'.').map(|i| i + 1));
                label_starts
                    .into_iter()
                    .any(|i| glob(&self.body, &url[i..], false, self.end_anchor))
            }
        }
    }
}

fn is_same_or_subdomain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Finds the host in a URL, as byte offsets.
fn host_range(url: &[u8]) -> Option<(usize, usize)> {
    let scheme_end = url.windows(3).position(|window| window == b"://")?;
    let mut start = scheme_end + 3;
    let authority_end = url[start..]
        .iter()
        .position(|&c| matches!(c, b'/' | b'?' | b'#'))
        .map_or(url.len(), |i| start + i);
    if let Some(at) = url[start..authority_end].iter().rposition(|&c| c == b'@') {
        start += at + 1;
    }
    let end = if url.get(start) == Some(&b'[') {
        url[start..authority_end]
            .iter()
            .position(|&c| c == b']')
            .map_or(authority_end, |i| start + i + 1)
    } else {
        url[start..authority_end]
            .iter()
            .position(|&c| c == b':')
            .map_or(authority_end, |i| start + i)
    };
    Some((start, end))
}

/// The characters that `^` matches, besides the end of the URL.
fn is_separator(c: u8) -> bool {
    !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'%'))
}

/// Matches a pattern with `*` and `^` wildcards against `text`, which it must
/// match from the start, unless `any_start`.
fn glob(pattern: &[u8], text: &[u8], any_start: bool, to_end: bool) -> bool {
    let (mut p, mut t) = (0, 0);
    // Where to resume after the last `*`, if a later part of the pattern fails.
    let mut star: Option<(usize, usize)> = any_start.then_some((0, 0));
    loop {
        if p == pattern.len() {
            if !to_end || t == text.len() {
                return true;
            }
        } else {
            let c = pattern[p];
            if c == b'*' {
                p += 1;
                star = Some((p, t));
                continue;
            }
            if t < text.len() && (c == text[t] || (c == b'^' && is_separator(text[t]))) {
                p += 1;
                t += 1;
                continue;
            }
            if c == b'^' && t == text.len() {
                p += 1;
                continue;
            }
        }
        match star {
            Some((star_p, star_t)) if star_t < text.len() => {
                star = Some((star_p, star_t + 1));
                p = star_p;
                t = star_t + 1;
            }
            _ => return false,
        }
    }
}

fn hash_token(token: &[u8]) -> u64 {
    // FNV-1a.
    token.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &c| {
        (hash ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn url_tokens(lower_url: &[u8]) -> Vec<u64> {
    lower_url
        .split(|c| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(hash_token)
        .collect()
}

/// Tokens so common in URLs that they're poor at narrowing down rules.
const COMMON_TOKENS: [&[u8]; 9] = [
    b"http", b"https", b"www", b"com", b"net", b"org", b"js", b"html", b"php",
];

/// Picks a word from the rule that any URL it matches must contain as a whole
/// token, for looking the rule up by.
fn best_token(rule: &Rule) -> Option<u64> {
    let body = &rule.body;
    let mut best: Option<(bool, usize, &[u8])> = None;
    let mut start = 0;
    while start < body.len() {
        if !body[start].is_ascii_alphanumeric() {
            start += 1;
            continue;
        }
        let end = body[start..]
            .iter()
            .position(|c| !c.is_ascii_alphanumeric())
            .map_or(body.len(), |i| start + i);
        let bounded_before = if start == 0 {
            rule.anchor != Anchor::None
        } else {
            body[start - 1] != b'*'
        };
        let bounded_after = if end == body.len() {
            rule.end_anchor
        } else {
            body[end] != b'*'
        };
        if bounded_before && bounded_after {
            let token = &body[start..end];
            let rank = (!COMMON_TOKENS.contains(&token), token.len(), token);
            if best.is_none_or(|best| (rank.0, rank.1) > (best.0, best.1)) {
                best = Some(rank);
            }
        }
        start = end;
    }
    best.map(|(_, _, token)| hash_token(&token.to_ascii_lowercase()))
}

fn parse_line(line: &str) -> (Parsed, Option<Rule>) {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
        return (Parsed::NotNetworkRule, None);
    }
    // Element hiding, scriptlets and the like.
    if ["##", "#@#", "#?#", "#$#", "#%#", "#@?#", "#@$#"]
        .iter()
        .any(|marker| line.contains(marker))
    {
        return (Parsed::NotNetworkRule, None);
    }
    if line.contains(char::is_whitespace) {
        return (Parsed::Unsupported, None);
    }

    let (exception, line) = match line.strip_prefix("@@") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let (pattern, options) = match line.rfind('$') {
        Some(index) => (&line[..index], Some(&line[index + 1..])),
        None => (line, None),
    };
    if pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/') {
        // A regular expression.
        return (Parsed::Unsupported, None);
    }

    let mut rule = Rule {
        anchor: Anchor::None,
        body: Vec::new(),
        end_anchor: false,
        match_case: false,
        types: 0,
        third_party: None,
        include_domains: Vec::new(),
        exclude_domains: Vec::new(),
        important: false,
    };
    let mut excluded_types = 0;
    let mut page_exception = false;

    for option in options.into_iter().flat_map(|options| options.split(',')) {
        let (negated, name) = match option.strip_prefix('~') {
            Some(name) => (true, name),
            None => (false, option),
        };
        let (name, value) = match name.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (name, None),
        };
        let resource_type = match name {
            "script" => Some(ResourceType::Script),
            "image" => Some(ResourceType::Image),
            "stylesheet" | "css" => Some(ResourceType::Stylesheet),
            "font" => Some(ResourceType::Font),
            "media" => Some(ResourceType::Media),
            "object" | "object-subrequest" => Some(ResourceType::Object),
            "xmlhttprequest" | "xhr" => Some(ResourceType::XmlHttpRequest),
            "subdocument" | "frame" => Some(ResourceType::Subdocument),
            "ping" | "beacon" => Some(ResourceType::Ping),
            "websocket" => Some(ResourceType::WebSocket),
            "other" => Some(ResourceType::Other),
            _ => None,
        };
        if let Some(resource_type) = resource_type {
            if negated {
                excluded_types |= resource_type.bit();
            } else {
                rule.types |= resource_type.bit();
            }
            continue;
        }
        match (name, value, negated) {
            ("third-party" | "3p", None, _) => rule.third_party = Some(!negated),
            ("first-party" | "1p", None, _) => rule.third_party = Some(negated),
            ("match-case", None, false) => rule.match_case = true,
            ("important", None, false) => rule.important = true,
            ("all", None, false) => rule.types |= DEFAULT_TYPES,
            ("document" | "doc", None, false) if exception => page_exception = true,
            ("domain" | "from", Some(domains), false) => {
                for domain in domains.split('|') {
                    let (excluded, domain) = match domain.strip_prefix('~') {
                        Some(domain) => (true, domain),
                        None => (false, domain),
                    };
                    // Entities like `example.*`, and regular expressions.
                    if domain.is_empty() || domain.ends_with(".*") || domain.starts_with('/') {
                        return (Parsed::Unsupported, None);
                    }
                    let domain = domain.to_ascii_lowercase();
                    if excluded {
                        rule.exclude_domains.push(domain);
                    } else {
                        rule.include_domains.push(domain);
                    }
                }
            }
            _ => return (Parsed::Unsupported, None),
        }
    }
    if rule.types == 0 {
        rule.types = DEFAULT_TYPES;
    }
    rule.types &= !excluded_types;
    if page_exception {
        rule.types = ResourceType::Document.bit();
    }
    if rule.types == 0 {
        return (Parsed::Unsupported, None);
    }

    let mut pattern = pattern;
    if let Some(rest) = pattern.strip_prefix("||") {
        rule.anchor = Anchor::Host;
        pattern = rest;
    } else if let Some(rest) = pattern.strip_prefix('|') {
        rule.anchor = Anchor::Start;
        pattern = rest;
    }
    if let Some(rest) = pattern.strip_suffix('|') {
        rule.end_anchor = true;
        pattern = rest;
    }
    if pattern.contains('|') {
        return (Parsed::Unsupported, None);
    }
    rule.body = if rule.match_case {
        pattern.as_bytes().to_vec()
    } else {
        pattern.to_ascii_lowercase().into_bytes()
    };

    let kind = match (exception, page_exception) {
        (false, _) => Parsed::Block,
        (true, false) => Parsed::Exception,
        (true, true) => Parsed::PageException,
    };
    (kind, Some(rule))
}

/// The part of a host name that a site registered, such as `example.co.uk` for
/// `www.example.co.uk`, given a test for whether a name is one.
pub fn registrable_domain(host: &str, is_registrable: impl Fn(&str) -> bool) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let dots: Vec<usize> = host.match_indices('.').map(|(i, _)| i).collect();
    for &dot in dots.iter().rev() {
        let suffix = &host[dot + 1..];
        if suffix.contains('.') && is_registrable(suffix) {
            return suffix.to_owned();
        }
    }
    host
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(rules: &str) -> Engine {
        let mut engine = Engine::default();
        engine.add_list(rules);
        engine
    }

    fn request<'a>(url: &'a str, page_url: &'a str, resource_type: ResourceType) -> Request<'a> {
        let host = |url: &'a str| {
            let (start, end) = host_range(url.as_bytes()).unwrap();
            &url[start..end]
        };
        let base = |host: &str| host.rsplitn(3, '.').take(2).collect::<Vec<_>>().join(".");
        Request {
            url,
            page_host: Some(host(page_url)),
            page_url: Some(page_url),
            third_party: base(host(url)) != base(host(page_url)),
            resource_type,
        }
    }

    fn blocks(engine: &Engine, url: &str, page_url: &str, resource_type: ResourceType) -> bool {
        engine.should_block(&request(url, page_url, resource_type))
    }

    const PAGE: &str = "https://news.example/story";

    #[test]
    fn host_anchor_matches_the_host_and_its_subdomains_only() {
        let engine = engine("||ads.example.com^");
        let script = ResourceType::Script;
        assert!(blocks(
            &engine,
            "https://ads.example.com/x.js",
            PAGE,
            script
        ));
        assert!(blocks(
            &engine,
            "https://cdn.ads.example.com/x.js",
            PAGE,
            script
        ));
        assert!(blocks(
            &engine,
            "http://ads.example.com:8080/",
            PAGE,
            script
        ));
        assert!(!blocks(
            &engine,
            "https://badads.example.com/x.js",
            PAGE,
            script
        ));
        assert!(!blocks(
            &engine,
            "https://ads.example.com.evil/x.js",
            PAGE,
            script
        ));
        assert!(!blocks(
            &engine,
            "https://example.com/ads.example.com/",
            PAGE,
            script
        ));
    }

    #[test]
    fn plain_patterns_match_anywhere_with_wildcards_and_separators() {
        let engine = engine("/banner/*/ad_\n-advert-\n&adtype=^");
        let image = ResourceType::Image;
        assert!(blocks(
            &engine,
            "https://a.example/banner/123/ad_1.png",
            PAGE,
            image
        ));
        assert!(blocks(
            &engine,
            "https://a.example/x-advert-y.gif",
            PAGE,
            image
        ));
        assert!(blocks(
            &engine,
            "https://a.example/q?a=1&adtype=",
            PAGE,
            image
        ));
        assert!(blocks(
            &engine,
            "https://a.example/q?a=1&adtype=&b",
            PAGE,
            image
        ));
        assert!(!blocks(
            &engine,
            "https://a.example/q?a=1&adtype=x",
            PAGE,
            image
        ));
        assert!(!blocks(
            &engine,
            "https://a.example/banner.png",
            PAGE,
            image
        ));
    }

    #[test]
    fn start_and_end_anchors() {
        let engine = engine("|https://track.\n.gif|");
        let other = ResourceType::Other;
        assert!(blocks(&engine, "https://track.example/p", PAGE, other));
        assert!(!blocks(
            &engine,
            "https://x.example/https://track.",
            PAGE,
            other
        ));
        assert!(blocks(&engine, "https://x.example/pixel.gif", PAGE, other));
        assert!(!blocks(
            &engine,
            "https://x.example/pixel.gif?x=1",
            PAGE,
            other
        ));
    }

    #[test]
    fn exceptions_override_unless_important() {
        let engine = engine(
            "||ads.example.com^\n@@||ads.example.com/allowed/\n||track.example^$important\n@@||track.example^",
        );
        let script = ResourceType::Script;
        assert!(!blocks(
            &engine,
            "https://ads.example.com/allowed/a.js",
            PAGE,
            script
        ));
        assert!(blocks(
            &engine,
            "https://ads.example.com/other/a.js",
            PAGE,
            script
        ));
        assert!(blocks(&engine, "https://track.example/t.js", PAGE, script));
    }

    #[test]
    fn third_party_and_resource_type_options() {
        let engine = engine("||cdn.example^$third-party,script\n||img.example^$~image");
        assert!(blocks(
            &engine,
            "https://cdn.example/a.js",
            PAGE,
            ResourceType::Script
        ));
        assert!(!blocks(
            &engine,
            "https://cdn.example/a.css",
            PAGE,
            ResourceType::Stylesheet
        ));
        assert!(!blocks(
            &engine,
            "https://cdn.example/a.js",
            "https://www.cdn.example/",
            ResourceType::Script
        ));
        assert!(!blocks(
            &engine,
            "https://img.example/a.png",
            PAGE,
            ResourceType::Image
        ));
        assert!(blocks(
            &engine,
            "https://img.example/a.js",
            PAGE,
            ResourceType::Script
        ));
    }

    #[test]
    fn domain_option_limits_rules_to_pages() {
        let engine = engine("/promo.$domain=shop.example|~sub.shop.example");
        let image = ResourceType::Image;
        assert!(blocks(
            &engine,
            "https://x.example/promo.png",
            "https://shop.example/",
            image
        ));
        assert!(blocks(
            &engine,
            "https://x.example/promo.png",
            "https://a.shop.example/",
            image
        ));
        assert!(!blocks(
            &engine,
            "https://x.example/promo.png",
            "https://sub.shop.example/",
            image
        ));
        assert!(!blocks(&engine, "https://x.example/promo.png", PAGE, image));
    }

    #[test]
    fn page_exceptions_turn_off_blocking_on_whole_pages() {
        let engine = engine("||ads.example^\n@@||trusted.example^$document");
        let script = ResourceType::Script;
        assert!(!blocks(
            &engine,
            "https://ads.example/a.js",
            "https://trusted.example/",
            script
        ));
        assert!(blocks(&engine, "https://ads.example/a.js", PAGE, script));
    }

    #[test]
    fn pages_themselves_are_not_blocked_by_default() {
        let engine = engine("||ads.example^");
        assert!(!blocks(
            &engine,
            "https://ads.example/",
            PAGE,
            ResourceType::Document
        ));
        assert!(blocks(
            &engine,
            "https://ads.example/",
            PAGE,
            ResourceType::Subdocument
        ));
    }

    #[test]
    fn match_case() {
        let engine = engine("/BannerAd.$match-case");
        let image = ResourceType::Image;
        assert!(blocks(
            &engine,
            "https://x.example/BannerAd.png",
            PAGE,
            image
        ));
        assert!(!blocks(
            &engine,
            "https://x.example/bannerad.png",
            PAGE,
            image
        ));
    }

    #[test]
    fn skips_what_it_cannot_apply() {
        let mut engine = Engine::default();
        let stats = engine.add_list(
            "[Adblock Plus 2.0]\n! comment\n\nexample.com##.ad\n##.banner\nexample.com#@#.ad\n\
             /ads[0-9]+/\n||x.example^$redirect=noop.js\n||y.example^$csp=script-src 'none'\n\
             ||z.example^$domain=example.*\n||popup.example^$popup\n||w.example^",
        );
        assert_eq!(
            stats,
            ListStats {
                rules: 1,
                unsupported: 5
            }
        );
        let script = ResourceType::Script;
        assert!(blocks(&engine, "https://w.example/a.js", PAGE, script));
        assert!(!blocks(&engine, "https://x.example/a.js", PAGE, script));
        assert!(!blocks(&engine, "https://popup.example/", PAGE, script));
    }

    #[test]
    fn rules_are_found_whatever_token_they_are_indexed_by() {
        let engine = engine("||ab.com^\n*/ad/*\nad\n||a.b.c^");
        let other = ResourceType::Other;
        assert!(blocks(&engine, "https://ab.com/", PAGE, other));
        assert!(blocks(&engine, "https://x.example/p/ad/q", PAGE, other));
        assert!(blocks(&engine, "https://x.example/readme", PAGE, other));
        assert!(blocks(&engine, "https://a.b.c/", PAGE, other));
    }

    #[test]
    fn registrable_domains() {
        let is_registrable = |name: &str| matches!(name, "example.com" | "example.co.uk");
        assert_eq!(
            registrable_domain("www.example.com", is_registrable),
            "example.com"
        );
        assert_eq!(
            registrable_domain("a.b.example.co.uk", is_registrable),
            "example.co.uk"
        );
        assert_eq!(
            registrable_domain("example.com", is_registrable),
            "example.com"
        );
        assert_eq!(registrable_domain("localhost", is_registrable), "localhost");
    }
}
