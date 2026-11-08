//! Completing what's typed in the location field, from the bookmarks.

use crate::bookmarks::Bookmark;

/// Text to fill in after what was typed, and the address it stands for.
#[derive(Debug, PartialEq)]
pub struct Autofill {
    /// What was typed, followed by the rest of the completion.
    pub text: String,
    /// The bookmark's URL, up to the end of the completion, with its scheme.
    pub url: String,
}

/// Completes typed text as the start of a bookmarked address. Like Firefox, it
/// completes the host name first, and then one part of the path at a time.
pub fn autofill(typed: &str, bookmarks: &[Bookmark]) -> Option<Autofill> {
    if typed.is_empty() || !typed.is_ascii() || typed.contains(char::is_whitespace) {
        return None;
    }
    let with_scheme = typed.contains("://");
    let with_www = typed.to_ascii_lowercase().starts_with("www.");

    bookmarks
        .iter()
        .filter_map(|bookmark| {
            let offset = if with_scheme {
                0
            } else {
                address_start(&bookmark.url, with_www)
            };
            let rest = &bookmark.url[offset..];
            let candidate = rest.get(..typed.len())?;
            if !candidate.eq_ignore_ascii_case(typed) {
                return None;
            }
            let end = rest[typed.len()..]
                .find('/')
                .map_or(rest.len(), |slash| typed.len() + slash + 1);
            if end == typed.len() {
                return None;
            }
            Some(Autofill {
                text: format!("{typed}{}", &rest[typed.len()..end]),
                url: bookmark.url[..offset + end].to_owned(),
            })
        })
        .min_by_key(|autofill| autofill.text.len())
}

/// Where the part of a URL that people type starts, after the scheme and, unless
/// `keep_www`, a `www.`.
fn address_start(url: &str, keep_www: bool) -> usize {
    let mut start = url.find("://").map_or(0, |index| index + 3);
    if !keep_www && url[start..].to_ascii_lowercase().starts_with("www.") {
        start += 4;
    }
    start
}

/// Bookmarks matching every word that was typed, in their title or URL. Those
/// whose address starts with the typed text come first, then those whose title
/// does, then the rest.
pub fn suggestions<'a>(typed: &str, bookmarks: &'a [Bookmark], limit: usize) -> Vec<&'a Bookmark> {
    let typed = typed.trim().to_lowercase();
    let words: Vec<&str> = typed.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut matches: Vec<(u8, &Bookmark)> = bookmarks
        .iter()
        .filter_map(|bookmark| {
            let url = bookmark.url.to_lowercase();
            let title = bookmark.title.to_lowercase();
            if !words
                .iter()
                .all(|word| url.contains(word) || title.contains(word))
            {
                return None;
            }
            let rank = if url[address_start(&url, false)..].starts_with(&typed)
                || url[address_start(&url, true)..].starts_with(&typed)
                || url.starts_with(&typed)
            {
                0
            } else if title.starts_with(&typed) {
                1
            } else {
                2
            };
            Some((rank, bookmark))
        })
        .collect();
    matches.sort_by_key(|(rank, _)| *rank);
    matches
        .into_iter()
        .take(limit)
        .map(|(_, bookmark)| bookmark)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmarks() -> Vec<Bookmark> {
        [
            (
                "https://github.com/aisloperator/browsegoo",
                "BrowseGoo on GitHub",
            ),
            ("https://www.example.org/docs/guide.html", "Example Guide"),
            ("http://old.example.net/", "Old Site"),
            ("https://servo.org/", "Servo, the embeddable browser engine"),
        ]
        .into_iter()
        .map(|(url, title)| Bookmark {
            url: url.into(),
            title: title.into(),
        })
        .collect()
    }

    fn filled(typed: &str) -> Option<(String, String)> {
        autofill(typed, &bookmarks()).map(|autofill| (autofill.text, autofill.url))
    }

    #[test]
    fn completes_the_host_first_then_one_path_part_at_a_time() {
        assert_eq!(
            filled("git"),
            Some(("github.com/".into(), "https://github.com/".into()))
        );
        assert_eq!(
            filled("github.com/ais"),
            Some((
                "github.com/aisloperator/".into(),
                "https://github.com/aisloperator/".into()
            ))
        );
        assert_eq!(
            filled("github.com/aisloperator/b"),
            Some((
                "github.com/aisloperator/browsegoo".into(),
                "https://github.com/aisloperator/browsegoo".into()
            ))
        );
    }

    #[test]
    fn keeps_what_was_typed_and_ignores_case() {
        assert_eq!(
            filled("GitH"),
            Some(("GitHub.com/".into(), "https://github.com/".into()))
        );
    }

    #[test]
    fn skips_www_unless_typed() {
        assert_eq!(
            filled("exa"),
            Some(("example.org/".into(), "https://www.example.org/".into()))
        );
        assert_eq!(
            filled("www.exa"),
            Some(("www.example.org/".into(), "https://www.example.org/".into()))
        );
    }

    #[test]
    fn keeps_the_bookmarks_scheme() {
        assert_eq!(
            filled("old"),
            Some(("old.example.net/".into(), "http://old.example.net/".into()))
        );
        assert_eq!(
            filled("https://ser"),
            Some(("https://servo.org/".into(), "https://servo.org/".into()))
        );
    }

    #[test]
    fn nothing_to_fill() {
        assert_eq!(filled(""), None);
        assert_eq!(filled("servo.org/"), None);
        assert_eq!(filled("nothing"), None);
        assert_eq!(filled("two words"), None);
        assert_eq!(filled("ex\u{e4}mple"), None);
    }

    fn suggested(typed: &str) -> Vec<String> {
        suggestions(typed, &bookmarks(), 8)
            .into_iter()
            .map(|bookmark| bookmark.title.clone())
            .collect()
    }

    #[test]
    fn suggests_by_url_or_title_words() {
        assert_eq!(suggested("example"), ["Example Guide", "Old Site"]);
        assert_eq!(
            suggested("browser engine"),
            ["Servo, the embeddable browser engine"]
        );
        assert_eq!(suggested("GUIDE"), ["Example Guide"]);
        assert!(suggested("   ").is_empty());
        assert!(suggested("nothing").is_empty());
    }

    #[test]
    fn address_matches_come_first() {
        // "ser" is in both titles' words, but only Servo's address starts with it.
        let bookmarks = vec![
            Bookmark {
                url: "https://example.com/".into(),
                title: "Browser user guide".into(),
            },
            Bookmark {
                url: "https://servo.org/".into(),
                title: "Servo".into(),
            },
        ];
        let titles: Vec<_> = suggestions("ser", &bookmarks, 8)
            .into_iter()
            .map(|bookmark| bookmark.title.as_str())
            .collect();
        assert_eq!(titles, ["Servo", "Browser user guide"]);
    }

    #[test]
    fn limits_suggestions() {
        assert_eq!(suggestions("o", &bookmarks(), 2).len(), 2);
    }
}
