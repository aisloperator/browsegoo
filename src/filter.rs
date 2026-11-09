//! Converts the URL filter rules of Adblock Plus style lists, such as EasyList,
//! the format uBlock Origin also uses, into WebKit content blocker rules.
//!
//! WebKit applies its rules itself, in a compiled form, so this only has to
//! translate them. It does the network part of the lists, and only the common
//! parts of their syntax. Rules that can't be translated correctly are skipped
//! rather than approximated: regular expressions, element hiding, scriptlets,
//! redirects, and options it doesn't know.
//!
//! WebKit applies rules in order, and an `ignore-previous-rules` rule undoes
//! the ones before it. So blocking rules come first, then exceptions, then
//! `$important` blocking rules, which exceptions don't undo, and last the
//! exceptions that turn filtering off on whole pages.

/// The kinds of resource that filter rules can be limited to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceType {
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
    const ALL: [ResourceType; 12] = [
        ResourceType::Document,
        ResourceType::Subdocument,
        ResourceType::Script,
        ResourceType::Stylesheet,
        ResourceType::Image,
        ResourceType::Font,
        ResourceType::Media,
        ResourceType::Object,
        ResourceType::XmlHttpRequest,
        ResourceType::Ping,
        ResourceType::WebSocket,
        ResourceType::Other,
    ];

    fn bit(self) -> u16 {
        1 << (self as u16)
    }

    /// WebKit's name for this type of resource, for those other than documents.
    fn webkit_name(self) -> Option<&'static str> {
        match self {
            ResourceType::Document | ResourceType::Subdocument => None,
            ResourceType::Script => Some("script"),
            ResourceType::Stylesheet => Some("style-sheet"),
            ResourceType::Image => Some("image"),
            ResourceType::Font => Some("font"),
            ResourceType::Media => Some("media"),
            // WebKit has no plugins, so what's left of these is "other".
            ResourceType::Object | ResourceType::Other => Some("other"),
            ResourceType::XmlHttpRequest => Some("fetch"),
            ResourceType::Ping => Some("ping"),
            ResourceType::WebSocket => Some("websocket"),
        }
    }
}

const ALL_TYPES: u16 = (1 << 12) - 1;
/// Rules with no type options apply to everything but whole pages.
const DEFAULT_TYPES: u16 = ALL_TYPES & !(1 << ResourceType::Document as u16);

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

/// How many lines of a list were used.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ListStats {
    pub rules: usize,
    pub unsupported: usize,
}

/// Converts a filter list into the JSON of a WebKit content blocker.
pub fn convert_list(text: &str) -> (String, ListStats) {
    let mut stats = ListStats::default();
    // In the order WebKit needs them, as described at the top.
    let mut groups: [Vec<String>; 4] = Default::default();
    for line in text.lines() {
        let (kind, rule) = parse_line(line);
        let Some(rule) = rule else {
            if kind == Parsed::Unsupported {
                stats.unsupported += 1;
            }
            continue;
        };
        let (group, triggers, action) = match kind {
            Parsed::Block if rule.important => (2, url_triggers(&rule), "block"),
            Parsed::Block => (0, url_triggers(&rule), "block"),
            Parsed::Exception => (1, url_triggers(&rule), "ignore-previous-rules"),
            Parsed::PageException => (3, page_triggers(&rule), "ignore-previous-rules"),
            Parsed::NotNetworkRule | Parsed::Unsupported => unreachable!("these have no rule"),
        };
        let Some(triggers) = triggers else {
            stats.unsupported += 1;
            continue;
        };
        stats.rules += 1;
        for trigger in triggers {
            groups[group].push(format!(
                "{{\"trigger\":{trigger},\"action\":{{\"type\":\"{action}\"}}}}"
            ));
        }
    }
    let rules: Vec<String> = groups.into_iter().flatten().collect();
    (format!("[{}]", rules.join(",\n")), stats)
}

/// The triggers for a rule about requests, or `None` if WebKit can't express it.
fn url_triggers(rule: &Rule) -> Option<Vec<String>> {
    // WebKit allows only one of these in a trigger.
    if !rule.include_domains.is_empty() && !rule.exclude_domains.is_empty() {
        return None;
    }
    let mut conditions = Vec::new();
    if rule.match_case {
        conditions.push("\"url-filter-is-case-sensitive\":true".to_owned());
    }
    match rule.third_party {
        Some(true) => conditions.push("\"load-type\":[\"third-party\"]".to_owned()),
        Some(false) => conditions.push("\"load-type\":[\"first-party\"]".to_owned()),
        None => {}
    }
    for (name, domains) in [
        ("if-domain", &rule.include_domains),
        ("unless-domain", &rule.exclude_domains),
    ] {
        if !domains.is_empty() {
            let domains: Vec<String> = domains
                .iter()
                .map(|domain| json_string(&format!("*{domain}")))
                .collect();
            conditions.push(format!("\"{name}\":[{}]", domains.join(",")));
        }
    }

    // Documents loading in frames are a type of their own in these lists, but
    // in WebKit they're documents loading in a child frame.
    let mut type_sets = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for kind in ResourceType::ALL {
        if let Some(name) = kind.webkit_name().filter(|_| rule.types & kind.bit() != 0) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if !names.is_empty() {
        let names: Vec<String> = names.iter().map(|name| json_string(name)).collect();
        type_sets.push(format!("\"resource-type\":[{}]", names.join(",")));
    }
    if rule.types & ResourceType::Subdocument.bit() != 0 {
        type_sets.push(
            "\"resource-type\":[\"document\"],\"load-context\":[\"child-frame\"]".to_owned(),
        );
    }

    let mut triggers = Vec::new();
    for filter in url_filters(rule)? {
        for types in &type_sets {
            let mut parts = vec![format!("\"url-filter\":{}", json_string(&filter))];
            parts.push(types.clone());
            parts.extend(conditions.iter().cloned());
            triggers.push(format!("{{{}}}", parts.join(",")));
        }
    }
    Some(triggers)
}

/// The triggers for an exception that turns off filtering on the pages whose
/// URL matches the rule.
fn page_triggers(rule: &Rule) -> Option<Vec<String>> {
    // WebKit can't combine a page URL condition with these.
    if !rule.include_domains.is_empty()
        || !rule.exclude_domains.is_empty()
        || rule.third_party.is_some()
    {
        return None;
    }
    let pages: Vec<String> = url_filters(rule)?
        .iter()
        .map(|filter| json_string(filter))
        .collect();
    let case = if rule.match_case {
        ",\"top-url-filter-is-case-sensitive\":true"
    } else {
        ""
    };
    Some(vec![format!(
        "{{\"url-filter\":\".*\",\"if-top-url\":[{}]{case}}}",
        pages.join(",")
    )])
}

/// The characters that `^` matches, besides the end of the URL.
const SEPARATOR: &str = "[^a-zA-Z0-9_.%-]";

/// Translates a rule's pattern into the regular expressions WebKit matches URLs
/// with, which have no alternatives. So a `^` at the end, which also matches
/// the end of the URL, takes two expressions, except after a host name, which
/// in a URL is always followed by something else.
fn url_filters(rule: &Rule) -> Option<Vec<String>> {
    let mut body = rule.body.as_slice();
    let mut start = match rule.anchor {
        Anchor::None => {
            while let [b'*', rest @ ..] = body {
                body = rest;
            }
            String::new()
        }
        Anchor::Start => "^".to_owned(),
        Anchor::Host => "^[a-z][a-z0-9.+-]*://([^/:@?#]*\\.)?".to_owned(),
    };
    if !rule.end_anchor {
        while let [rest @ .., b'*'] = body {
            body = rest;
        }
    }
    let ends_with_separator = !rule.end_anchor && body.last() == Some(&b'^');
    if ends_with_separator {
        body = &body[..body.len() - 1];
    }

    for &c in body {
        match c {
            b'*' => start.push_str(".*"),
            b'^' => start.push_str(SEPARATOR),
            b'\\' | b'.' | b'+' | b'?' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'|' | b'$' => {
                start.push('\\');
                start.push(c as char);
            }
            // WebKit only takes ASCII, and nothing else is in a URL anyway.
            32..=126 => start.push(c as char),
            _ => return None,
        }
    }
    let filter = start;

    if rule.end_anchor {
        return Some(vec![format!("{filter}$")]);
    }
    if !ends_with_separator {
        return Some(vec![if filter.is_empty() {
            ".*".to_owned()
        } else {
            filter
        }]);
    }
    let after_host = rule.anchor == Anchor::Host && !body.contains(&b'/');
    if after_host {
        Some(vec![format!("{filter}{SEPARATOR}")])
    } else {
        Some(vec![format!("{filter}{SEPARATOR}"), format!("{filter}$")])
    }
}

/// A JSON string. The text is always ASCII here, from `url_filters` or domains.
fn json_string(text: &str) -> String {
    let mut json = String::with_capacity(text.len() + 2);
    json.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            json.push('\\');
        }
        json.push(c);
    }
    json.push('"');
    json
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

#[cfg(test)]
mod tests {
    use super::*;

    fn converted(list: &str) -> Vec<String> {
        let (json, _) = convert_list(list);
        let json = json.trim_start_matches('[').trim_end_matches(']');
        if json.is_empty() {
            return Vec::new();
        }
        json.split(",\n").map(str::to_owned).collect()
    }

    const DEFAULT_TYPES_JSON: &str = r#""resource-type":["script","style-sheet","image","font","media","other","fetch","ping","websocket"]"#;

    #[test]
    fn host_anchors_match_the_host_and_its_subdomains() {
        let rules = converted("||ads.example.com^$script");
        assert_eq!(
            rules,
            [
                r#"{"trigger":{"url-filter":"^[a-z][a-z0-9.+-]*://([^/:@?#]*\\.)?ads\\.example\\.com[^a-zA-Z0-9_.%-]","resource-type":["script"]},"action":{"type":"block"}}"#
            ]
        );
    }

    #[test]
    fn a_final_separator_also_matches_the_end_of_the_url() {
        let rules = converted("&adtype=^$image");
        assert_eq!(rules.len(), 2);
        assert!(rules[0].contains(r#""url-filter":"&adtype=[^a-zA-Z0-9_.%-]""#));
        assert!(rules[1].contains(r#""url-filter":"&adtype=$""#));
    }

    #[test]
    fn wildcards_anchors_and_escaping() {
        let rules = converted("/banner/*/ad_$image\n|https://track.$image\n.gif|$image\n*/a+b?(c)$image");
        assert!(rules[0].contains(r#""url-filter":"/banner/.*/ad_""#));
        assert!(rules[1].contains(r#""url-filter":"^https://track\\.""#));
        assert!(rules[2].contains(r#""url-filter":"\\.gif$""#));
        assert!(rules[3].contains(r#""url-filter":"/a\\+b\\?\\(c\\)""#));
    }

    #[test]
    fn untyped_rules_cover_everything_but_top_level_pages() {
        let rules = converted("-advert-");
        assert_eq!(rules.len(), 2);
        assert!(rules[0].contains(DEFAULT_TYPES_JSON));
        assert!(rules[1].contains(r#""resource-type":["document"],"load-context":["child-frame"]"#));
    }

    #[test]
    fn options_become_conditions() {
        let rules = converted(
            "||cdn.example^$third-party,script\n/promo.$image,domain=shop.example|a.example\n\
             /x.$image,domain=~b.example,first-party\n/BannerAd.$image,match-case",
        );
        assert!(rules[0].contains(r#""load-type":["third-party"]"#));
        assert!(rules[1].contains(r#""if-domain":["*shop.example","*a.example"]"#));
        assert!(rules[2].contains(r#""load-type":["first-party"]"#));
        assert!(rules[2].contains(r#""unless-domain":["*b.example"]"#));
        assert!(rules[3].contains(r#""url-filter":"/BannerAd\\.""#));
        assert!(rules[3].contains(r#""url-filter-is-case-sensitive":true"#));
    }

    #[test]
    fn exceptions_follow_blocks_and_important_rules_follow_exceptions() {
        let rules = converted(
            "@@||trusted.example^$document\n||track.example^$important,script\n\
             @@||ads.example/allowed/$script\n||ads.example^$script",
        );
        let actions: Vec<&str> = rules
            .iter()
            .map(|rule| {
                if rule.contains("if-top-url") {
                    "page exception"
                } else if rule.contains("ignore-previous-rules") {
                    "exception"
                } else if rule.contains("track") {
                    "important"
                } else {
                    "block"
                }
            })
            .collect();
        assert_eq!(
            actions,
            ["block", "exception", "important", "page exception"]
        );
        assert!(rules[3].contains(
            r#"{"url-filter":".*","if-top-url":["^[a-z][a-z0-9.+-]*://([^/:@?#]*\\.)?trusted\\.example[^a-zA-Z0-9_.%-]"]}"#
        ));
    }

    #[test]
    fn skips_what_it_cannot_express() {
        let (_, stats) = convert_list(
            "[Adblock Plus 2.0]\n! comment\n\nexample.com##.ad\n##.banner\nexample.com#@#.ad\n\
             /ads[0-9]+/\n||x.example^$redirect=noop.js\n||y.example^$csp=script-src 'none'\n\
             ||z.example^$domain=example.*\n||popup.example^$popup\n||w.example^\n\
             ||v.example^$domain=a.example|~b.a.example\n||caf\u{e9}.example^",
        );
        assert_eq!(
            stats,
            ListStats {
                rules: 1,
                unsupported: 7
            }
        );
    }

    #[test]
    fn output_is_a_json_array() {
        let (json, _) = convert_list("||a.example^\n@@||a.example/ok^");
        assert!(json.starts_with("[{") && json.ends_with("}]"));
        assert_eq!(convert_list("! nothing").0, "[]");
    }
}
