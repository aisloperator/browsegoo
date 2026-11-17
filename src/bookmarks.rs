//! Simple bookmarks, stored one per line in a text file as `URL<tab>title`.

use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

pub struct Bookmarks {
    path: PathBuf,
    items: Vec<Bookmark>,
}

impl Bookmarks {
    /// Loads the bookmarks file. A file that doesn't exist yet means no bookmarks.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let items = match fs::read_to_string(&path) {
            Ok(text) => parse(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, items })
    }

    pub fn items(&self) -> &[Bookmark] {
        &self.items
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|item| item.url == url)
    }

    /// Bookmarks `url` if it isn't bookmarked, and otherwise removes it.
    pub fn toggle(&mut self, url: &str, title: &str) -> io::Result<()> {
        if self.contains(url) {
            self.items.retain(|item| item.url != url);
        } else {
            self.items.push(Bookmark {
                url: url.to_owned(),
                title: single_line(title),
            });
        }
        self.save()
    }

    /// Changes the bookmark at `index`. Its URL can't be another bookmark's, as
    /// a page is bookmarked at most once.
    pub fn edit(&mut self, index: usize, url: &str, title: &str) -> Result<(), String> {
        if index >= self.items.len() {
            return Err("That bookmark no longer exists.".to_owned());
        }
        self.check_url(url, Some(index))?;
        self.items[index] = Bookmark {
            url: url.to_owned(),
            title: single_line(title),
        };
        self.save_for_edit()
    }

    /// Adds a bookmark at `index`, or at the end if that's past it.
    pub fn insert(&mut self, index: usize, url: &str, title: &str) -> Result<(), String> {
        self.check_url(url, None)?;
        let index = index.min(self.items.len());
        self.items.insert(
            index,
            Bookmark {
                url: url.to_owned(),
                title: single_line(title),
            },
        );
        self.save_for_edit()
    }

    /// Checks that `url` can be the bookmark at `index`, or a new one.
    fn check_url(&self, url: &str, index: Option<usize>) -> Result<(), String> {
        if url.is_empty() || url.contains(char::is_whitespace) {
            return Err("That isn't a valid address.".to_owned());
        }
        let is_duplicate = self
            .items
            .iter()
            .enumerate()
            .any(|(other, item)| Some(other) != index && item.url == url);
        if is_duplicate {
            return Err("That address is already bookmarked.".to_owned());
        }
        Ok(())
    }

    fn save_for_edit(&self) -> Result<(), String> {
        self.save()
            .map_err(|error| format!("Could not save bookmarks: {error}"))
    }

    pub fn remove(&mut self, index: usize) -> io::Result<()> {
        if index < self.items.len() {
            self.items.remove(index);
        }
        self.save()
    }

    /// Moves the bookmark at `from` to be at `to`.
    pub fn move_item(&mut self, from: usize, to: usize) -> io::Result<()> {
        if from < self.items.len() && to < self.items.len() {
            let item = self.items.remove(from);
            self.items.insert(to, item);
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        write_atomically(&self.path, &serialize(&self.items))
    }
}

fn parse(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(url, title)| Bookmark {
            url: url.to_owned(),
            title: title.to_owned(),
        })
        .collect()
}

fn serialize(items: &[Bookmark]) -> String {
    items
        .iter()
        .map(|item| format!("{}\t{}\n", item.url, item.title))
        .collect()
}

/// Page titles are arbitrary text, and must not be able to break the file format.
fn single_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("browsegoo-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory.join("bookmarks.tsv")
    }

    #[test]
    fn missing_file_means_no_bookmarks() {
        let bookmarks = Bookmarks::load(temporary_path("missing")).unwrap();
        assert!(bookmarks.items().is_empty());
    }

    #[test]
    fn toggling_adds_and_removes_and_persists() {
        let path = temporary_path("toggle");
        let mut bookmarks = Bookmarks::load(path.clone()).unwrap();

        bookmarks.toggle("https://a.example/", "A").unwrap();
        bookmarks.toggle("https://b.example/", "B").unwrap();
        bookmarks.toggle("https://a.example/", "A").unwrap();

        let reloaded = Bookmarks::load(path).unwrap();
        assert_eq!(
            reloaded.items(),
            [Bookmark {
                url: "https://b.example/".into(),
                title: "B".into()
            }]
        );
    }

    fn urls(bookmarks: &Bookmarks) -> Vec<&str> {
        bookmarks
            .items()
            .iter()
            .map(|item| item.url.as_str())
            .collect()
    }

    #[test]
    fn editing_removing_and_moving_persist() {
        let path = temporary_path("edit");
        let mut bookmarks = Bookmarks::load(path.clone()).unwrap();
        for url in [
            "https://a.example/",
            "https://b.example/",
            "https://c.example/",
        ] {
            bookmarks.toggle(url, "x").unwrap();
        }

        bookmarks
            .edit(1, "https://bee.example/", "Bee\nline")
            .unwrap();
        bookmarks.move_item(2, 0).unwrap();
        bookmarks.remove(1).unwrap();

        let reloaded = Bookmarks::load(path).unwrap();
        assert_eq!(
            urls(&reloaded),
            ["https://c.example/", "https://bee.example/"]
        );
        assert_eq!(reloaded.items()[1].title, "Bee line");
    }

    #[test]
    fn edits_cannot_duplicate_or_break_urls() {
        let mut bookmarks = Bookmarks::load(temporary_path("bad-edit")).unwrap();
        bookmarks.toggle("https://a.example/", "A").unwrap();
        bookmarks.toggle("https://b.example/", "B").unwrap();

        assert!(bookmarks.edit(1, "https://a.example/", "B").is_err());
        assert!(bookmarks.edit(1, "https://b.example/\tx", "B").is_err());
        assert!(bookmarks.edit(1, "", "B").is_err());
        assert!(bookmarks.edit(5, "https://c.example/", "C").is_err());
        // A bookmark keeping its own URL isn't a duplicate.
        assert!(bookmarks.edit(1, "https://b.example/", "Renamed").is_ok());
        assert_eq!(
            urls(&bookmarks),
            ["https://a.example/", "https://b.example/"]
        );
    }

    #[test]
    fn inserting_checks_and_persists() {
        let path = temporary_path("insert");
        let mut bookmarks = Bookmarks::load(path.clone()).unwrap();
        bookmarks.toggle("https://a.example/", "A").unwrap();
        bookmarks.toggle("https://c.example/", "C").unwrap();

        bookmarks.insert(1, "https://b.example/", "B\tee").unwrap();
        bookmarks.insert(99, "https://d.example/", "D").unwrap();
        assert!(bookmarks.insert(0, "https://a.example/", "A").is_err());
        assert!(bookmarks.insert(0, "", "Nothing").is_err());

        let reloaded = Bookmarks::load(path).unwrap();
        assert_eq!(
            urls(&reloaded),
            [
                "https://a.example/",
                "https://b.example/",
                "https://c.example/",
                "https://d.example/"
            ]
        );
        assert_eq!(reloaded.items()[1].title, "B ee");
    }

    #[test]
    fn hostile_titles_cannot_add_lines_or_fields() {
        let path = temporary_path("hostile");
        let mut bookmarks = Bookmarks::load(path.clone()).unwrap();

        bookmarks
            .toggle(
                "https://a.example/",
                "one\ttwo\nhttps://evil.example/\tthree",
            )
            .unwrap();

        let reloaded = Bookmarks::load(path).unwrap();
        assert_eq!(reloaded.items().len(), 1);
        assert_eq!(
            reloaded.items()[0].title,
            "one two https://evil.example/ three"
        );
    }
}
