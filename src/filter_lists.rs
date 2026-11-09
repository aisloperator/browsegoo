//! The filter lists BrowseGoo knows about, which of them are enabled, and their
//! copies in the cache directory, which are kept in the lists' own format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use url::Url;

use crate::fetch;
use crate::filter::{self, ListStats};

pub struct FilterList {
    /// Also the name of its file in the cache.
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub enabled_by_default: bool,
}

pub const LISTS: [FilterList; 4] = [
    FilterList {
        id: "easylist",
        name: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
        enabled_by_default: true,
    },
    FilterList {
        id: "easyprivacy",
        name: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        enabled_by_default: true,
    },
    FilterList {
        id: "peter-lowe",
        name: "Peter Lowe's Ad and tracking server list",
        url: "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext",
        enabled_by_default: false,
    },
    FilterList {
        id: "fanboy-annoyance",
        name: "Fanboy's Annoyance List",
        url: "https://secure.fanboy.co.nz/fanboy-annoyance.txt",
        enabled_by_default: false,
    },
];

/// Which lists are enabled, saved one per line as `URL<tab>on` or `URL<tab>off`.
pub struct Settings {
    path: PathBuf,
    /// Only lists that have been changed from their default.
    by_url: BTreeMap<String, bool>,
}

impl Settings {
    /// Loads the settings. A file that doesn't exist yet means the defaults.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let by_url = match fs::read_to_string(&path) {
            Ok(text) => text
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .filter_map(|(url, state)| match state {
                    "on" => Some((url.to_owned(), true)),
                    "off" => Some((url.to_owned(), false)),
                    _ => None,
                })
                .collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, by_url })
    }

    pub fn is_enabled(&self, list: &FilterList) -> bool {
        self.by_url
            .get(list.url)
            .copied()
            .unwrap_or(list.enabled_by_default)
    }

    pub fn set_enabled(&mut self, list: &FilterList, enabled: bool) -> io::Result<()> {
        self.by_url.insert(list.url.to_owned(), enabled);
        let text: String = self
            .by_url
            .iter()
            .map(|(url, &on)| format!("{url}\t{}\n", if on { "on" } else { "off" }))
            .collect();
        write_atomically(&self.path, text.as_bytes())
    }

    pub fn enabled_lists(&self) -> Vec<&'static FilterList> {
        LISTS.iter().filter(|list| self.is_enabled(list)).collect()
    }
}

/// What's known about a list's cached copy.
#[derive(Clone, Debug, Default)]
pub struct ListStatus {
    pub downloaded: Option<SystemTime>,
    /// For enabled lists that are downloaded.
    pub stats: Option<ListStats>,
    /// Why WebKit couldn't use the list, if it couldn't.
    pub problem: Option<String>,
}

pub fn cache_path(cache_dir: &Path, list: &FilterList) -> PathBuf {
    cache_dir.join(format!("{}.txt", list.id))
}

/// A downloaded list, converted for WebKit.
pub struct Converted {
    pub list: &'static FilterList,
    /// Changes with the list and with how it's converted, so that it's only
    /// compiled again when needed.
    pub fingerprint: String,
    pub json: String,
}

/// Where WebKit keeps the compiled lists, each named by its list's ID.
pub fn compiled_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("compiled")
}

fn fingerprint_path(cache_dir: &Path, list: &FilterList) -> PathBuf {
    compiled_dir(cache_dir).join(format!("{}.fingerprint", list.id))
}

/// Whether the compiled list in WebKit's store is of this conversion.
pub fn is_compiled(cache_dir: &Path, converted: &Converted) -> bool {
    fs::read_to_string(fingerprint_path(cache_dir, converted.list))
        .is_ok_and(|fingerprint| fingerprint == converted.fingerprint)
}

/// Records that this conversion is the one compiled in WebKit's store.
pub fn mark_compiled(cache_dir: &Path, converted: &Converted) -> io::Result<()> {
    fs::create_dir_all(compiled_dir(cache_dir))?;
    write_atomically(
        &fingerprint_path(cache_dir, converted.list),
        converted.fingerprint.as_bytes(),
    )
}

/// Bump this when `filter` converts lists differently, so that they're compiled again.
const CONVERSION_VERSION: u32 = 1;

/// Converts the cached copies of the enabled lists, and reports on every list.
pub fn convert_lists(
    cache_dir: &Path,
    enabled: &[&FilterList],
) -> (Vec<Converted>, BTreeMap<&'static str, ListStatus>) {
    let mut converted = Vec::new();
    let mut statuses = BTreeMap::new();
    for list in &LISTS {
        let path = cache_path(cache_dir, list);
        let mut status = ListStatus {
            downloaded: fs::metadata(&path).and_then(|data| data.modified()).ok(),
            stats: None,
            problem: None,
        };
        if enabled.iter().any(|enabled| enabled.id == list.id) {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let (json, stats) = filter::convert_list(&text);
                    status.stats = Some(stats);
                    converted.push(Converted {
                        list,
                        fingerprint: format!("{:016x}", fingerprint(&text)),
                        json,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("Could not read filter list {}: {error}", list.name),
            }
        }
        statuses.insert(list.id, status);
    }
    (converted, statuses)
}

/// A hash of a list and of how it's converted.
fn fingerprint(text: &str) -> u64 {
    // FNV-1a, which unlike Rust's own hashing stays the same between versions.
    let bytes = CONVERSION_VERSION.to_le_bytes();
    bytes
        .iter()
        .chain(text.as_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, &c| {
            (hash ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// Downloads a list into the cache.
pub fn update(cache_dir: &Path, list: &FilterList, user_agent: &str) -> Result<(), String> {
    let url = Url::parse(list.url).map_err(|error| error.to_string())?;
    let data = fetch::get(&url, user_agent)?;
    let text = String::from_utf8(data).map_err(|_| "the list isn't text".to_owned())?;
    check_is_filter_list(&text)?;
    fs::create_dir_all(cache_dir).map_err(|error| error.to_string())?;
    write_atomically(&cache_path(cache_dir, list), text.as_bytes())
        .map_err(|error| error.to_string())
}

/// Guards against caching something else, such as an error page, as a list.
fn check_is_filter_list(text: &str) -> Result<(), String> {
    let first_line = text.lines().map(str::trim).find(|line| !line.is_empty());
    match first_line {
        Some(line) if line.starts_with("[Adblock") || line.starts_with('!') => Ok(()),
        _ => Err("the download isn't a filter list".to_owned()),
    }
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("browsegoo-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn settings_default_then_persist_changes() {
        let path = temporary_dir("settings").join("filter_lists.tsv");
        let mut settings = Settings::load(path.clone()).unwrap();
        let ids = |settings: &Settings| {
            settings
                .enabled_lists()
                .iter()
                .map(|list| list.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&settings), ["easylist", "easyprivacy"]);

        settings.set_enabled(&LISTS[0], false).unwrap();
        settings.set_enabled(&LISTS[3], true).unwrap();
        let reloaded = Settings::load(path).unwrap();
        assert_eq!(ids(&reloaded), ["easyprivacy", "fanboy-annoyance"]);
    }

    #[test]
    fn only_enabled_downloaded_lists_are_converted() {
        let cache = temporary_dir("cache");
        fs::write(
            cache_path(&cache, &LISTS[0]),
            "[Adblock Plus 2.0]\n||a.example^\n||b.example^\n",
        )
        .unwrap();
        fs::write(
            cache_path(&cache, &LISTS[2]),
            "[Adblock Plus 2.0]\n||c.example^\n",
        )
        .unwrap();

        let (converted, statuses) = convert_lists(&cache, &[&LISTS[0], &LISTS[1]]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].list.id, "easylist");
        assert!(!is_compiled(&cache, &converted[0]));
        mark_compiled(&cache, &converted[0]).unwrap();
        assert!(is_compiled(&cache, &converted[0]));
        assert!(converted[0].json.contains("a\\\\.example"));
        assert_eq!(statuses["easylist"].stats.map(|stats| stats.rules), Some(2));
        assert!(statuses["easyprivacy"].downloaded.is_none());
        assert!(statuses["peter-lowe"].downloaded.is_some());
        assert!(statuses["peter-lowe"].stats.is_none());
    }

    #[test]
    fn identifiers_change_with_the_list() {
        assert_eq!(fingerprint("||a^"), fingerprint("||a^"));
        assert_ne!(fingerprint("||a^"), fingerprint("||b^"));
    }

    #[test]
    fn only_filter_lists_are_cached() {
        assert!(check_is_filter_list("[Adblock Plus 2.0]\n||a^").is_ok());
        assert!(check_is_filter_list("\n! Title: x\n||a^").is_ok());
        assert!(check_is_filter_list("<!doctype html><title>Error</title>").is_err());
        assert!(check_is_filter_list("").is_err());
    }
}
