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

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};

use crate::archive::{self, Archive, Lock};
use crate::error::Error;
use crate::triage::plural;

/// A line's `schema_version`, when it is `V`: a line from another schema
/// is unreadable, not misread.
pub fn schema<'de, D: Deserializer<'de>, const V: u32>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version == V {
        Ok(version)
    } else {
        Err(D::Error::custom(format!(
            "schema_version {version} is not {V}"
        )))
    }
}

/// The attempt number of a line that does not say: a page's first run.
pub fn first_attempt() -> u32 {
    1
}

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
    /// absent, under the archive lock, so the root must exist. The log is
    /// `0600` on Unix, an existing one included.
    pub fn open(root: &Path, path: PathBuf) -> Result<Self, Error> {
        let lock = Archive::at(root).lock(|| {})?;
        Self::open_locked(root, path, &lock)
    }

    /// [`Self::open`] for a caller already holding the archive lock, so
    /// that the log's setup and the caller's own are one step.
    pub fn open_locked(root: &Path, path: PathBuf, _held: &Lock) -> Result<Self, Error> {
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
        archive::make_private(&path).map_err(Error::io("make private", &path))?;
        Ok(Self {
            archive: Archive::at(root),
            path,
            file,
            line: PhantomData,
        })
    }

    /// One whole line in one write, under the archive lock, synced.
    pub fn append(&mut self, line: &T) -> Result<(), Error> {
        let lock = self.archive.lock(|| {})?;
        self.append_locked(line, &lock)
    }

    /// [`Self::append`] for a caller already holding the archive lock, so
    /// that a write before the line and the line itself are one step.
    pub fn append_locked(&mut self, line: &T, _held: &Lock) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec(line).map_err(|source| Error::Json {
            path: self.path.clone(),
            source,
        })?;
        bytes.push(b'\n');
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

/// A log and the directory of files its lines stand for, written as one:
/// every write to either holds the archive lock, a file before its line.
#[derive(Debug)]
pub struct Store<T> {
    archive: Archive,
    dir: PathBuf,
    log: Appender<T>,
}

impl<T: Serialize> Store<T> {
    /// Opens the log at `log` and the directory `dir`, both private, and
    /// removes what an interrupted run left staged in `dir`, all under one
    /// hold of the archive lock, so every write the store makes holds it.
    pub fn open(root: &Path, log: PathBuf, dir: PathBuf) -> Result<Self, Error> {
        let archive = Archive::at(root);
        let lock = archive.lock(|| {})?;
        let log = Appender::open_locked(root, log, &lock)?;
        archive::create_private_dir(&dir).map_err(Error::io("create", &dir))?;
        archive::make_private(&dir).map_err(Error::io("make private", &dir))?;
        archive::clean_stale_staging(&dir)?;
        for entry in fs::read_dir(&dir).map_err(Error::io("list", &dir))? {
            let entry = entry.map_err(Error::io("list", &dir))?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(Error::io("read", &path))?
                .is_file()
            {
                archive::make_private(&path).map_err(Error::io("make private", &path))?;
            }
        }
        drop(lock);
        Ok(Self { archive, dir, log })
    }

    /// Records one line under one hold of the archive lock: `files` first
    /// writes what the line stands for in the directory it is given and
    /// returns the line, which is then appended. Returns the line written.
    pub fn record(&mut self, files: impl FnOnce(&Path) -> Result<T, Error>) -> Result<T, Error> {
        let lock = self.archive.lock(|| {})?;
        let line = files(&self.dir)?;
        self.log.append_locked(&line, &lock)?;
        Ok(line)
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

impl<R> Latest<R> {
    /// The warning a command gives when lines of the log at `path` could
    /// not be read; `None` when every line could.
    pub fn unreadable_note(&self, path: &Path) -> Option<String> {
        (self.unreadable > 0).then(|| {
            format!(
                "{} of {} could not be read and {} ignored",
                plural(self.unreadable, "line"),
                path.display(),
                if self.unreadable == 1 { "was" } else { "were" }
            )
        })
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
        archive::create_private_dir(&root).unwrap();
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

    #[test]
    fn opening_a_store_writes_nothing_until_it_holds_the_archive_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let files = root.join("pages").join("files");
        let held = Archive::at(&root).lock(|| {}).unwrap();
        let (ready, started) = std::sync::mpsc::channel();
        let opening = std::thread::spawn({
            let root = root.clone();
            let files = files.clone();
            move || {
                // Check each actual directory creation, including after an
                // Appender::open that would release its own lock too early.
                let probe = fs::File::options()
                    .read(true)
                    .write(true)
                    .open(root.join(archive::LOCK_FILE))
                    .unwrap();
                archive::DIRECTORY_CREATION_LOCK_PROBE.with(|slot| {
                    *slot.borrow_mut() = Some(probe);
                });
                ready.send(()).unwrap();
                Store::<Entry>::open(&root, log(&root), files).map(|_| ())
            }
        });
        started.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let wrote_early = root.join("pages").exists();
        drop(held);
        opening.join().unwrap().unwrap();
        assert!(!wrote_early, "nothing before the lock");
        assert!(log(&root).is_file());
        assert!(files.is_dir());
    }

    #[test]
    fn opening_a_store_clears_staged_leftovers_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let files = root.join("pages").join("files");
        fs::create_dir_all(&files).unwrap();
        fs::write(
            files.join(format!("{}abcdef", archive::STAGING_PREFIX)),
            "half",
        )
        .unwrap();
        fs::write(files.join("kept.md"), "kept").unwrap();
        Store::<Entry>::open(root, log(root), files.clone()).unwrap();
        let names: Vec<String> = fs::read_dir(&files)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["kept.md"]);
    }
}
