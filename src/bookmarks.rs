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

fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
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
