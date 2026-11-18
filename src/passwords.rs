//! Saved passwords for sites, each for one exact domain.
//!
//! They're saved one per line in a text file that only the user can read:
//! `domain<tab>username<tab>password<tab>created<tab>edited<tab>last used`,
//! with the times in seconds since 1970, and the last used time empty if it
//! never was. Backslashes, tabs and line breaks in the fields are escaped.
//!
//! They aren't site data, so they aren't deleted when BrowseGoo quits.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, os::unix::fs::OpenOptionsExt, os::unix::fs::PermissionsExt};

/// Identifies a record while BrowseGoo runs. It isn't saved.
pub type RecordId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: RecordId,
    pub domain: String,
    pub username: String,
    pub password: String,
    /// Seconds since 1970.
    pub created: i64,
    pub edited: i64,
    pub last_used: Option<i64>,
}

impl Record {
    /// When it was last changed, or created if it never was, which is what
    /// the menu of passwords is sorted by.
    pub fn changed(&self) -> i64 {
        self.edited.max(self.created)
    }
}

pub struct Passwords {
    path: PathBuf,
    records: Vec<Record>,
    next_id: RecordId,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64)
}

/// The domain that passwords for the page at `url` are saved for, which is
/// its host, if it has one.
pub fn domain_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_lowercase)
}

/// Checks a domain typed in the Password Manager, and makes it lowercase.
pub fn check_domain(domain: &str) -> Result<String, String> {
    let domain = domain.trim().to_lowercase();
    if domain.is_empty() || domain.contains(|c: char| c.is_whitespace() || "/:?#@".contains(c)) {
        return Err("That isn't a valid domain name.".to_owned());
    }
    Ok(domain)
}

impl Passwords {
    /// Loads the passwords. A file that doesn't exist yet means none.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let mut passwords = Self {
            path,
            records: Vec::new(),
            next_id: 1,
        };
        let text = match fs::read_to_string(&passwords.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(passwords),
            Err(error) => return Err(error),
        };
        for (number, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let record = passwords.parse_line(line).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "line {} of {} isn't valid",
                        number + 1,
                        passwords.path.display()
                    ),
                )
            })?;
            passwords.records.push(record);
        }
        Ok(passwords)
    }

    fn parse_line(&mut self, line: &str) -> Option<Record> {
        let fields: Vec<String> = line.split('\t').map(unescape).collect::<Option<_>>()?;
        let [domain, username, password, created, edited, last_used] = fields.as_slice() else {
            return None;
        };
        let last_used = match last_used.as_str() {
            "" => None,
            time => Some(time.parse().ok()?),
        };
        Some(Record {
            id: self.take_id(),
            domain: domain.clone(),
            username: username.clone(),
            password: password.clone(),
            created: created.parse().ok()?,
            edited: edited.parse().ok()?,
            last_used,
        })
    }

    fn take_id(&mut self) -> RecordId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn get(&self, id: RecordId) -> Option<&Record> {
        self.records.iter().find(|record| record.id == id)
    }

    /// The passwords for exactly `domain`, not for its subdomains or the
    /// domains it's a subdomain of, most recently created or edited first.
    pub fn for_domain(&self, domain: &str) -> Vec<Record> {
        let mut records: Vec<Record> = self
            .records
            .iter()
            .filter(|record| record.domain == domain)
            .cloned()
            .collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.changed()));
        records
    }

    /// Whether exactly this username and password are saved for `domain`.
    pub fn contains(&self, domain: &str, username: &str, password: &str) -> bool {
        self.records.iter().any(|record| {
            record.domain == domain && record.username == username && record.password == password
        })
    }

    /// Adds a record, even if `domain` already has one.
    pub fn add(&mut self, domain: &str, username: &str, password: &str) -> io::Result<RecordId> {
        let time = now();
        let record = Record {
            id: self.take_id(),
            domain: domain.to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
            created: time,
            edited: time,
            last_used: None,
        };
        let id = record.id;
        let mut records = self.records.clone();
        records.push(record);
        self.save(records)?;
        Ok(id)
    }

    /// Changes a record, and when it was last edited, if anything changed.
    pub fn edit(
        &mut self,
        id: RecordId,
        domain: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        let domain = check_domain(domain)?;
        let mut records = self.records.clone();
        let record = records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or("That password is no longer saved.")?;
        if record.domain == domain && record.username == username && record.password == password {
            return Ok(());
        }
        record.domain = domain;
        record.username = username.to_owned();
        record.password = password.to_owned();
        record.edited = now();
        self.save(records)
            .map_err(|error| format!("Could not save passwords: {error}"))
    }

    pub fn remove(&mut self, id: RecordId) -> io::Result<()> {
        let mut records = self.records.clone();
        records.retain(|record| record.id != id);
        self.save(records)
    }

    /// Records that the password was just filled in on a page.
    pub fn mark_used(&mut self, id: RecordId) -> io::Result<()> {
        let mut records = self.records.clone();
        if let Some(record) = records.iter_mut().find(|record| record.id == id) {
            record.last_used = Some(now());
        }
        self.save(records)
    }

    /// Saves `records`, and uses them if that worked, so that nothing changes
    /// if it didn't.
    fn save(&mut self, records: Vec<Record>) -> io::Result<()> {
        let text: String = records.iter().map(serialize).collect();
        write_privately(&self.path, &text)?;
        self.records = records;
        Ok(())
    }
}

fn serialize(record: &Record) -> String {
    let last_used = record
        .last_used
        .map(|time| time.to_string())
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        escape(&record.domain),
        escape(&record.username),
        escape(&record.password),
        record.created,
        record.edited,
        last_used
    )
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            c => escaped.push(c),
        }
    }
    escaped
}

fn unescape(text: &str) -> Option<String> {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            unescaped.push(c);
            continue;
        }
        unescaped.push(match chars.next()? {
            '\\' => '\\',
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(unescaped)
}

/// Replaces the file at `path` with `contents`, in a file that only the user
/// can read or write.
fn write_privately(path: &std::path::Path, contents: &str) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    // In case the file was already there, with other permissions.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "browsegoo-test-{}-passwords-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory.join("passwords.tsv")
    }

    #[test]
    fn missing_file_means_no_passwords() {
        let passwords = Passwords::load(temporary_path("missing")).unwrap();
        assert!(passwords.records().is_empty());
    }

    #[test]
    fn adding_editing_and_removing_persist() {
        let path = temporary_path("persist");
        let mut passwords = Passwords::load(path.clone()).unwrap();
        let a = passwords.add("a.example", "alice", "secret").unwrap();
        let b = passwords.add("a.example", "bob", "hunter2").unwrap();
        let c = passwords.add("c.example", "carol", "pw").unwrap();
        passwords
            .edit(b, "A.Example", "bob", "tab\there\nand \\ newline")
            .unwrap();
        passwords.mark_used(a).unwrap();
        passwords.remove(c).unwrap();

        let reloaded = Passwords::load(path).unwrap();
        let records = reloaded.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].username, "alice");
        assert!(records[0].last_used.is_some());
        assert_eq!(records[1].domain, "a.example");
        assert_eq!(records[1].password, "tab\there\nand \\ newline");
        assert_eq!(records[1].last_used, None);
    }

    #[test]
    fn adding_never_replaces() {
        let mut passwords = Passwords::load(temporary_path("no-replace")).unwrap();
        passwords.add("a.example", "alice", "one").unwrap();
        passwords.add("a.example", "alice", "two").unwrap();
        assert_eq!(passwords.for_domain("a.example").len(), 2);
        assert!(passwords.contains("a.example", "alice", "one"));
        assert!(!passwords.contains("a.example", "alice", "three"));
    }

    #[test]
    fn domains_match_exactly() {
        let mut passwords = Passwords::load(temporary_path("exact")).unwrap();
        passwords.add("example.com", "a", "1").unwrap();
        passwords.add("www.example.com", "b", "2").unwrap();
        let names = |domain| {
            passwords
                .for_domain(domain)
                .into_iter()
                .map(|record| record.username)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("example.com"), ["a"]);
        assert_eq!(names("www.example.com"), ["b"]);
        assert!(names("evil-example.com").is_empty());
        assert!(names("com").is_empty());
    }

    #[test]
    fn most_recently_changed_first() {
        let mut passwords = Passwords::load(temporary_path("order")).unwrap();
        let old = passwords.add("a.example", "old", "1").unwrap();
        passwords.add("a.example", "new", "2").unwrap();
        // Times are in seconds, so these are set rather than waited for.
        passwords.records[0].created = 100;
        passwords.records[0].edited = 100;
        passwords.records[1].created = 200;
        passwords.records[1].edited = 200;
        assert_eq!(passwords.for_domain("a.example")[0].username, "new");
        passwords.records[0].edited = 300;
        assert_eq!(passwords.for_domain("a.example")[0].id, old);
    }

    #[test]
    fn the_file_is_private() {
        let path = temporary_path("private");
        let mut passwords = Passwords::load(path.clone()).unwrap();
        passwords.add("a.example", "alice", "secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn domains_of_pages() {
        assert_eq!(
            domain_of("https://Login.Example.com:8443/x?y").as_deref(),
            Some("login.example.com")
        );
        assert_eq!(
            domain_of("http://192.168.1.1/").as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(domain_of("about:blank"), None);
        assert_eq!(domain_of("file:///etc/passwd"), None);
    }

    #[test]
    fn bad_domains_and_lines_are_refused() {
        assert!(check_domain("").is_err());
        assert!(check_domain("a b.example").is_err());
        assert!(check_domain("https://a.example").is_err());
        assert_eq!(check_domain(" A.Example ").unwrap(), "a.example");
        assert_eq!(unescape("bad\\x"), None);
        assert_eq!(unescape("trailing\\"), None);
        assert_eq!(
            unescape(&escape("a\\b\tc\nd\re")).as_deref(),
            Some("a\\b\tc\nd\re")
        );

        let path = temporary_path("bad-line");
        fs::write(&path, "only\tthree\tfields\n").unwrap();
        assert!(Passwords::load(path).is_err());
    }
}
