//! The User-Agent that pages get, in request headers and as
//! `navigator.userAgent`: BrowseGoo's own, or, for Web development, WebKit's
//! default or that of a popular browser.

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::VERSION;
use crate::bookmarks::write_atomically;

/// The version tokens that WebKit's default User-Agent has, which WebKit
/// keeps the same from one release to another. They're read from WebKit's
/// default when they can be, so that they stay WebKit's.
const APPLE_WEBKIT: &str = "605.1.15";
const SAFARI_VERSION: &str = "60.5";

/// The latest stable Chrome and Firefox, on 64-bit Windows 11 on x86. Both
/// say Windows 10 there, which Windows 11 is to them, and Chrome gives only
/// its major version.
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/159.0.0.0 Safari/537.36";
const FIREFOX: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:160.0) Gecko/20100101 Firefox/160.0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// BrowseGoo's own, which says nothing of the window system, operating
    /// system or hardware.
    Actual,
    /// WebKit's default, which says it's Safari.
    WebKit,
    Chrome,
    Firefox,
}

impl Choice {
    pub const ALL: [Choice; 4] = [
        Choice::Actual,
        Choice::WebKit,
        Choice::Chrome,
        Choice::Firefox,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Choice::Actual => "Actual",
            Choice::WebKit => "Development: WebKit (default)",
            Choice::Chrome => "Development: Chrome on MS on Intel",
            Choice::Firefox => "Development: Firefox on MS on Intel",
        }
    }

    /// How it's saved.
    fn id(self) -> &'static str {
        match self {
            Choice::Actual => "actual",
            Choice::WebKit => "webkit",
            Choice::Chrome => "chrome",
            Choice::Firefox => "firefox",
        }
    }

    /// The User-Agent, given WebKit's default one, or `None` for WebKit's
    /// default.
    pub fn value(self, webkit_default: &str) -> Option<String> {
        match self {
            Choice::Actual => Some(actual(webkit_default)),
            Choice::WebKit => None,
            Choice::Chrome => Some(CHROME.to_owned()),
            Choice::Firefox => Some(FIREFOX.to_owned()),
        }
    }

    /// Whether WebKit may give some sites another User-Agent, among its fixes
    /// for particular sites. Otherwise pages would not always get the one
    /// chosen, so those fixes are turned off.
    pub fn allows_site_fixes(self) -> bool {
        self == Choice::WebKit
    }
}

/// BrowseGoo's own User-Agent, with WebKit's version tokens, but no platform.
pub fn actual(webkit_default: &str) -> String {
    let token = |name: &str, fallback: &'static str| {
        webkit_default
            .split(' ')
            .find_map(|part| part.strip_prefix(name))
            .filter(|version| !version.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    };
    format!(
        "Mozilla/5.0 () AppleWebKit/{} (KHTML, like Gecko) Version/{} BrowseGoo/{VERSION}",
        token("AppleWebKit/", APPLE_WEBKIT),
        token("Version/", SAFARI_VERSION),
    )
}

/// Where the choice is saved.
fn choice_path(config_dir: &Path) -> PathBuf {
    config_dir.join("user_agent.txt")
}

/// The saved choice, which is Actual unless another was chosen.
pub fn chosen(config_dir: &Path) -> Choice {
    let text = fs::read_to_string(choice_path(config_dir)).unwrap_or_default();
    Choice::ALL
        .into_iter()
        .find(|choice| choice.id() == text.trim())
        .unwrap_or(Choice::Actual)
}

pub fn choose(config_dir: &Path, choice: Choice) -> io::Result<()> {
    write_atomically(&choice_path(config_dir), &format!("{}\n", choice.id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEBKIT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 \
                          (KHTML, like Gecko) Version/60.5 Safari/605.1.15";

    #[test]
    fn actual_has_webkit_and_browsegoo_versions_but_no_platform() {
        assert_eq!(
            actual(WEBKIT),
            format!(
                "Mozilla/5.0 () AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 \
                 BrowseGoo/{VERSION}"
            )
        );
        let later = WEBKIT
            .replace("605.1.15", "606.1.1")
            .replace("60.5", "61.0");
        assert!(actual(&later).contains("AppleWebKit/606.1.1 (KHTML, like Gecko) Version/61.0 "));
        assert_eq!(actual(""), actual(WEBKIT));
    }

    #[test]
    fn choice_persists_and_defaults_to_actual() {
        let directory =
            std::env::temp_dir().join(format!("browsegoo-test-{}-user-agent", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        assert_eq!(chosen(&directory), Choice::Actual);
        for choice in Choice::ALL {
            choose(&directory, choice).unwrap();
            assert_eq!(chosen(&directory), choice);
        }
        fs::write(choice_path(&directory), "nonsense\n").unwrap();
        assert_eq!(chosen(&directory), Choice::Actual);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn values() {
        assert_eq!(Choice::WebKit.value(WEBKIT), None);
        assert!(
            Choice::Chrome
                .value(WEBKIT)
                .unwrap()
                .contains("Chrome/159.0.0.0 Safari/537.36")
        );
        assert!(
            Choice::Firefox
                .value(WEBKIT)
                .unwrap()
                .ends_with("rv:160.0) Gecko/20100101 Firefox/160.0")
        );
    }
}
