//! Append-only JSON Lines logs: one whole line per record, appended under
//! the archive lock, and read back as the latest line for each URL.
//!
//! slice: tags, enrich, content
//! why: Every attempt log in the archive makes the same promises, so they
//!      are kept once rather than per log. A line is written whole, under the
//!      archive lock and synced before the next, so two runs never interleave
//!      and a run killed halfway leaves every earlier line readable. A torn
//!      tail from a crash is left for the reader to skip, and the next line
//!      starts on a fresh line rather than being glued to it. Logs record
//!      what you browse, so each file is private to you, as the root is.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::archive::{self, Archive};
use crate::error::Error;

/// A record that belongs to one URL; the latest line for a URL wins.
pub trait Keyed {
    fn key(&self) -> &str;
}

/// Appends lines of one type to one log for the length of a run.
#[derive(Debug)]
pub struct Appender<T> {
    archive: Archive,
    path: PathBuf,
    file: File,
    line: PhantomData<fn(&T)>,
}

impl<T: Serialize> Appender<T> {
    /// Creates the log's directory (private, as the root is) and the log if
    /// absent. The log is `0600` on Unix, an existing one included.
    pub fn open(root: &Path, path: PathBuf) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            archive::create_private_dir(dir).map_err(Error::io("create", dir))?;
        }
        let mut options = File::options();
        options.read(true).append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(Error::io("open", &path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = file
                .metadata()
                .map_err(Error::io("read", &path))?
                .permissions()
                .mode();
            if mode & 0o777 != 0o600 {
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .map_err(Error::io("make private", &path))?;
            }
        }
        Ok(Self {
            archive: Archive::at(root),
            path,
            file,
            line: PhantomData,
        })
    }

    /// One whole line in one write, under the archive lock, synced.
    pub fn append(&mut self, line: &T) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec(line).map_err(|source| Error::Json {
            path: self.path.clone(),
            source,
        })?;
        bytes.push(b'\n');
        let _lock = self.archive.lock(|| {})?;
        if !self.ends_with_newline()? {
            bytes.insert(0, b'\n');
        }
        self.file
            .write_all(&bytes)
            .map_err(Error::io("append to", &self.path))?;
        self.file.sync_data().map_err(Error::io("sync", &self.path))
    }

    /// Whether the file is empty or its last byte ends a line. Appends go to
    /// the end whatever the read position, so seeking here is harmless.
    fn ends_with_newline(&mut self) -> Result<bool, Error> {
        let len = self
            .file
            .metadata()
            .map_err(Error::io("read", &self.path))?
            .len();
        if len == 0 {
            return Ok(true);
        }
        let mut last = [0u8];
        self.file
            .seek(SeekFrom::Start(len - 1))
            .and_then(|_| self.file.read_exact(&mut last))
            .map_err(Error::io("read", &self.path))?;
        Ok(last[0] == b'\n')
    }
}

/// A log as its readers see it.
#[derive(Debug)]
pub struct Latest<R> {
    /// The latest line for each URL.
    pub pages: HashMap<String, R>,
    /// Lines that were not a record this build can read.
    pub unreadable: usize,
}

impl<R> Default for Latest<R> {
    fn default() -> Self {
        Self {
            pages: HashMap::new(),
            unreadable: 0,
        }
    }
}

/// The whole log; a missing file is an empty log. `what` names the log in
/// an error, as in "cannot read page metadata".
pub fn read<R: DeserializeOwned + Keyed>(
    path: &Path,
    what: &'static str,
) -> Result<Latest<R>, Error> {
    let text = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Latest::default()),
        Err(err) => return Err(Error::io(what, path)(err)),
    };
    Ok(parse(&text))
}

/// Blank lines are not counted; a line that is not a record is.
pub fn parse<R: DeserializeOwned + Keyed>(text: &str) -> Latest<R> {
    let mut latest = Latest::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<R>(line) {
            Ok(record) => {
                latest.pages.insert(record.key().to_owned(), record);
            }
            Err(_) => latest.unreadable += 1,
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Entry {
        url: String,
        n: u32,
    }

    impl Keyed for Entry {
        fn key(&self) -> &str {
            &self.url
        }
    }

    fn entry(url: &str) -> Entry {
        Entry {
            url: url.to_owned(),
            n: 1,
        }
    }

    fn log(root: &Path) -> PathBuf {
        root.join("pages").join("log.jsonl")
    }

    fn open(root: &Path) -> Appender<Entry> {
        Appender::open(root, log(root)).unwrap()
    }

    fn read_log(root: &Path) -> Latest<Entry> {
        read(&log(root), "read the log").unwrap()
    }

    fn raw_lines(root: &Path) -> Vec<String> {
        fs::read_to_string(log(root))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_torn_tail_costs_one_line_and_the_next_append_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        open(dir.path()).append(&entry("https://a.test/")).unwrap();
        let mut file = File::options().append(true).open(log(dir.path())).unwrap();
        file.write_all(br#"{"url":"https://torn.test/","n":"#)
            .unwrap();
        drop(file);
        let torn = read_log(dir.path());
        assert_eq!((torn.pages.len(), torn.unreadable), (1, 1));

        open(dir.path()).append(&entry("https://b.test/")).unwrap();
        let read = read_log(dir.path());
        assert_eq!(read.pages.len(), 2, "b.test was not glued to the tear");
        assert_eq!(read.unreadable, 1);
        assert!(raw_lines(dir.path())[2].starts_with(r#"{"url":"https://b.test/""#));
    }

    #[test]
    fn the_latest_line_for_a_url_wins_and_a_missing_log_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_log(dir.path()).pages.len(), 0);
        let mut appender = open(dir.path());
        appender.append(&entry("https://a.test/")).unwrap();
        appender
            .append(&Entry {
                url: "https://a.test/".to_owned(),
                n: 2,
            })
            .unwrap();
        let read = read_log(dir.path());
        assert_eq!((read.pages.len(), read.unreadable), (1, 0));
        assert_eq!(read.pages["https://a.test/"].n, 2);
    }

    #[test]
    fn concurrent_appenders_preserve_complete_records() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let root = dir.path();
                scope.spawn(move || {
                    let mut writer = open(root);
                    for n in 0..16 {
                        writer
                            .append(&entry(&format!("https://a.test/{worker}/{n}")))
                            .unwrap();
                    }
                });
            }
        });
        let read = read_log(dir.path());
        assert_eq!((read.pages.len(), read.unreadable), (128, 0));
        assert_eq!(raw_lines(dir.path()).len(), 128);
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_and_the_log_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        open(&root);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.join("pages")), 0o700);
        assert_eq!(mode(&log(&root)), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_log_is_made_private_and_keeps_its_lines() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = log(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"url\":\"https://a.test/\",\"n\":1}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        open(dir.path());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(read_log(dir.path()).pages.len(), 1);
    }
}
