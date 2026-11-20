//! Which cookies and site data to preserve. Everything else is deleted when
//! BrowseGoo quits, and when it starts, in case it didn't quit cleanly.
//!
//! Each preserved item is saved as one line, which is the key that the Site
//! Data Manager identifies it by: `site<tab>type<tab>name<tab>path`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::{fs, io};

use crate::bookmarks::write_atomically;

pub struct Preserved {
    path: PathBuf,
    keys: BTreeSet<String>,
}

impl Preserved {
    /// Loads the preserved items. A file that doesn't exist yet means none.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let keys = match fs::read_to_string(&path) {
            Ok(text) => text
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, keys })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Preserves the item with `key`, or stops preserving it, and saves the
    /// change. Nothing changes if it can't be saved.
    pub fn set(&mut self, key: &str, preserve: bool) -> io::Result<()> {
        if self.contains(key) == preserve {
            return Ok(());
        }
        if key.is_empty() || key.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the item's name can't be saved",
            ));
        }
        let mut keys = self.keys.clone();
        if preserve {
            keys.insert(key.to_owned());
        } else {
            keys.remove(key);
        }
        let text: String = keys.iter().map(|key| format!("{key}\n")).collect();
        write_atomically(&self.path, &text)?;
        self.keys = keys;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "browsegoo-test-{}-preserved-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory.join("preserved_site_data.tsv")
    }

    #[test]
    fn missing_file_means_nothing_is_preserved() {
        let preserved = Preserved::load(temporary_path("missing")).unwrap();
        assert!(!preserved.contains("example.com\tCookie\tid\t/"));
    }

    #[test]
    fn preserving_persists() {
        let path = temporary_path("set");
        let mut preserved = Preserved::load(path.clone()).unwrap();
        preserved.set("a.example\tCookie\tid\t/", true).unwrap();
        preserved.set("b.example\tLocal storage\t\t", true).unwrap();
        preserved.set("a.example\tCookie\tid\t/", false).unwrap();

        let reloaded = Preserved::load(path).unwrap();
        assert!(!reloaded.contains("a.example\tCookie\tid\t/"));
        assert!(reloaded.contains("b.example\tLocal storage\t\t"));
    }

    #[test]
    fn keys_that_would_break_the_file_are_refused() {
        let mut preserved = Preserved::load(temporary_path("refused")).unwrap();
        assert!(preserved.set("a.example\tCookie\tid\n\t/", true).is_err());
        assert!(!preserved.contains("a.example\tCookie\tid\n\t/"));
    }
}
