//! The page image store: `pages/images.jsonl`, one line per image attempt,
//! and `pages/images/<sha256>.jpg`, one image per page.
//!
//! slice: content
//! why: A page's image is kept apart from its text because many pages
//!      with an image have no text file (a page that is itself an image, a
//!      post read through its API), and a text file is only rewritten when
//!      its text changes. So the image log is the record, keyed by the
//!      same hash of the page's address as its text, and a reader joins
//!      the two by it. An image is replaced whole under the archive lock,
//!      before its line, and only when its bytes changed, so a backup's
//!      history stays flat. A line that failed keeps the candidates it
//!      tried, so the next run retries them without fetching the page.
//!      `serve` reads kept images here too, by the hex name alone, so no
//!      other input ever reaches a path.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::content_fetch::Attempted;
use crate::content_store::sha256_hex;
use crate::error::Error;
use crate::image_pick::{Candidate, Source};
use crate::jsonl::{self, Keyed};
use crate::metadata;
use crate::targets::{Outcome, Recorded};
use crate::{archive, capture};

pub const LOG_FILE: &str = "images.jsonl";
pub const DIR: &str = "images";
/// The one version of the image line this build writes and reads.
pub const SCHEMA_VERSION: u32 = 1;

pub fn log_path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(LOG_FILE)
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(DIR)
}

/// A page's image file: the lowercase hex SHA-256 of its exact URL, as
/// its text file is named.
pub fn file_name(url: &str) -> String {
    named(&sha256_hex(url.as_bytes()))
}

fn named(hex: &str) -> String {
    format!("{hex}.jpg")
}

/// Where `serve` answers with a kept image, by its hex name.
pub const ROUTE: &str = "/api/image/";

/// What became of a page's image. Only `error` is tried again on a plain
/// run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    /// The page has no image worth keeping, and says why.
    None,
    /// A failure that may pass on the best candidate.
    Error,
    Unavailable,
    /// A status a newer build wrote: read as final, never rewritten.
    #[serde(other)]
    Unknown,
}

impl Outcome for Status {
    fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::None => "none",
            Self::Error => "error",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }
}

/// One attempt at a page's image, written and read in this one shape.
/// Absent fields are not written; fields a newer build adds are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    #[serde(deserialize_with = "jsonl::schema::<_, SCHEMA_VERSION>")]
    pub schema_version: u32,
    pub url: String,
    pub attempted_at: Timestamp,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// The candidate's address, as the page named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Where its redirects ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The size of the kept JPEG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// The SHA-256 of the kept JPEG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_sha256: Option<String>,
    /// A card a service drew from the page's name rather than an image of
    /// the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What a line that kept no image tried, for a retry without the page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// Which run this is for the page's image: 1, then one more for each
    /// run that tries again after an `error`.
    #[serde(default = "jsonl::first_attempt")]
    pub attempt: u32,
}

impl Line {
    pub fn new(url: &str, status: Status) -> Self {
        let now = Timestamp::now();
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            attempted_at: Timestamp::from_second(now.as_second()).unwrap_or(now),
            status,
            source: None,
            image_url: None,
            final_url: None,
            http_status: None,
            mime: None,
            source_width: None,
            source_height: None,
            width: None,
            height: None,
            bytes: None,
            image_sha256: None,
            generated: None,
            reason: None,
            candidates: Vec::new(),
            attempt: 1,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl Keyed for Line {
    fn key(&self) -> &str {
        &self.url
    }
}

impl Attempted for Line {
    fn is_error(&self) -> bool {
        self.status == Status::Error
    }

    fn attempt(&self) -> u32 {
        self.attempt
    }

    fn set_attempt(&mut self, attempt: u32) {
        self.attempt = attempt;
    }

    fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    fn give_up(&mut self, reason: String) {
        self.status = Status::Unavailable;
        self.reason = Some(reason);
    }
}

pub type Log = jsonl::Latest<Line>;

/// The whole log; a missing file is an empty log.
pub fn read(root: &Path) -> Result<Log, Error> {
    jsonl::read(&log_path(root), "read the image log")
}

/// How the planner sees a page's latest image line.
pub fn recorded(log: &Log, url: &str) -> Recorded {
    match log.pages.get(url).map(|line| line.status) {
        None => Recorded::Nothing,
        Some(Status::Error) => Recorded::Failed,
        Some(_) => Recorded::Final,
    }
}

/// The pages whose latest line is `ok` and whose image is on disk, each
/// with the hex name `serve` answers it by: one read of the log and one of
/// the directory, however large the library. A log or directory that
/// cannot be read costs the images, not the library.
pub fn kept(root: &Path, log: capture::Log) -> HashMap<String, String> {
    read_kept(root).unwrap_or_else(|err| {
        log.warn(&format!("{err}; the library shows no preview images"));
        HashMap::new()
    })
}

fn read_kept(root: &Path) -> Result<HashMap<String, String>, Error> {
    let dir = dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(Error::io("read", &dir)(err)),
    };
    let mut files: HashSet<OsString> = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(Error::io("read", &dir))?;
        if entry
            .file_type()
            .map_err(Error::io("read", &dir))?
            .is_file()
        {
            files.insert(entry.file_name());
        }
    }
    Ok(read(root)?
        .pages
        .into_iter()
        .filter(|(_, line)| line.status == Status::Ok)
        .filter_map(|(url, _)| {
            let hex = sha256_hex(url.as_bytes());
            files
                .contains(OsStr::new(&named(&hex)))
                .then_some((url, hex))
        })
        .collect())
}

/// A kept image's bytes by its name: exactly 64 lowercase hex characters,
/// else `None`, as for a name with no regular file. Symlinks are refused.
/// No lock: an image is replaced by a rename, so a read sees the old file
/// or the new one whole.
pub fn served(root: &Path, name: &str) -> Result<Option<Vec<u8>>, Error> {
    if name.len() != 64 || !name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Ok(None);
    }
    let path = dir(root).join(named(name));
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::io("read", &path)(err)),
    }
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io("read", &path)(err)),
    }
}

/// Where a run writes: the log, and the directory of images.
#[derive(Debug)]
pub struct Store(jsonl::Store<Line>);

impl Store {
    /// Opens the log and the image directory, both private, and removes
    /// what an interrupted run left staged there, all under one hold of
    /// the archive lock.
    pub fn open(root: &Path) -> Result<Self, Error> {
        jsonl::Store::open(root, log_path(root), dir(root)).map(Self)
    }

    /// Records one attempt: the page's image first, when one was kept,
    /// then its line, both under one hold of the archive lock. An image
    /// whose bytes are unchanged is left as it is. Returns the line
    /// written.
    pub fn record(&mut self, mut line: Line, jpeg: Option<&[u8]>) -> Result<Line, Error> {
        self.0.record(|dir| {
            let Some(jpeg) = jpeg else {
                return Ok(line);
            };
            let hash = sha256_hex(jpeg);
            let path = dir.join(file_name(&line.url));
            let unchanged = match fs::read(&path) {
                Ok(bytes) => sha256_hex(&bytes) == hash,
                Err(err) if err.kind() == ErrorKind::NotFound => false,
                Err(err) => return Err(Error::io("read", &path)(err)),
            };
            line.bytes = Some(jpeg.len());
            line.image_sha256 = Some(hash);
            if !unchanged {
                archive::replace_file(&path, jpeg)?;
            }
            Ok(line)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_fetch::{next_attempt, settle};

    fn ok_line(url: &str) -> Line {
        let mut line = Line::new(url, Status::Ok);
        line.source = Some(Source::OgImage);
        line.image_url = Some("https://cdn.test/a.png".to_owned());
        line.final_url = Some("https://cdn.test/a.png".to_owned());
        line.http_status = Some(200);
        line.mime = Some("image/png".to_owned());
        line.source_width = Some(1200);
        line.source_height = Some(630);
        line.width = Some(768);
        line.height = Some(403);
        line.generated = Some(false);
        line
    }

    fn keys(line: &Line) -> Vec<String> {
        serde_json::to_value(line)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn an_image_line_writes_what_it_has_and_reads_back() {
        let mut line = ok_line("https://a.test/");
        line.bytes = Some(41210);
        line.image_sha256 = Some("ab".repeat(32));
        let text = serde_json::to_string(&line).unwrap();
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
        assert_eq!(
            keys(&line),
            [
                "attempt",
                "attempted_at",
                "bytes",
                "final_url",
                "generated",
                "height",
                "http_status",
                "image_sha256",
                "image_url",
                "mime",
                "schema_version",
                "source",
                "source_height",
                "source_width",
                "status",
                "url",
                "width"
            ]
        );
        let value = serde_json::to_value(&line).unwrap();
        assert_eq!(value["source"], "og_image");
        assert_eq!(value["status"], "ok");

        let mut none =
            Line::new("https://b.test/", Status::None).with_reason("all_rejected: too small");
        none.candidates = vec![Candidate {
            url: "https://b.test/i.png".to_owned(),
            source: Source::BodyImg,
        }];
        let text = serde_json::to_string(&none).unwrap();
        assert!(
            text.contains(r#""candidates":[{"url":"https://b.test/i.png","source":"body_img"}]"#)
        );
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), none);
        assert_eq!(
            keys(&Line::new("https://c.test/", Status::None)),
            ["attempt", "attempted_at", "schema_version", "status", "url"]
        );
    }

    #[test]
    fn the_latest_line_wins_and_newer_values_are_read_as_unknown() {
        let text = concat!(
            r#"{"schema_version":1,"url":"https://a.test/","attempted_at":"2026-10-07T09:00:00Z","status":"error","reason":"timeout","candidates":[{"url":"https://c.test/x.jpg","source":"og_image"}]}"#,
            "\n",
            r#"{"schema_version":1,"url":"https://b.test/","attempted_at":"2026-10-07T09:00:00Z","status":"blurred","source":"screenshot","attempt":2}"#,
            "\n",
            r#"{"schema_version":2,"url":"https://c.test/","attempted_at":"2026-10-07T09:00:00Z","status":"ok"}"#,
            "\n",
        );
        let log: Log = jsonl::parse(text);
        assert_eq!(log.unreadable, 1, "schema 2");
        let a = &log.pages["https://a.test/"];
        assert_eq!((a.status, a.attempt), (Status::Error, 1));
        assert_eq!(a.candidates[0].source, Source::OgImage);
        let b = &log.pages["https://b.test/"];
        assert_eq!(
            (b.status, b.source),
            (Status::Unknown, Some(Source::Unknown))
        );
        assert_eq!(recorded(&log, "https://a.test/"), Recorded::Failed);
        assert_eq!(recorded(&log, "https://b.test/"), Recorded::Final);
        assert_eq!(recorded(&log, "https://z.test/"), Recorded::Nothing);
    }

    #[test]
    fn an_image_failing_on_three_runs_becomes_unavailable() {
        let error = Line::new("https://a.test/", Status::Error).with_reason("HTTP 503");
        let first = settle(error.clone(), next_attempt::<Line>(None));
        let second = settle(error.clone(), next_attempt(Some(&first)));
        let third = settle(error.clone(), next_attempt(Some(&second)));
        assert_eq!(
            [first.status, second.status, third.status],
            [Status::Error, Status::Error, Status::Unavailable]
        );
        assert_eq!(third.attempt, 3);
        assert_eq!(third.reason.as_deref(), Some("HTTP 503; failed on 3 runs"));
        assert_eq!(next_attempt(Some(&third)), 1, "a refetch starts again");
        let ok = settle(ok_line("https://a.test/"), next_attempt(Some(&second)));
        assert_eq!((ok.status, ok.attempt), (Status::Ok, 3));
    }

    #[test]
    fn a_store_writes_the_image_then_the_line_and_leaves_an_unchanged_image_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::open(root).unwrap();
        let written = store
            .record(ok_line("https://a.test/"), Some(b"jpeg one"))
            .unwrap();
        assert_eq!(written.bytes, Some(8));
        assert_eq!(
            written.image_sha256.as_deref(),
            Some(sha256_hex(b"jpeg one").as_str())
        );
        let path = super::dir(root).join(file_name("https://a.test/"));
        assert_eq!(fs::read(&path).unwrap(), b"jpeg one");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .record(ok_line("https://a.test/"), Some(b"jpeg one"))
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "the same bytes leave the file alone"
        );
        store
            .record(ok_line("https://a.test/"), Some(b"jpeg two"))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"jpeg two");
        store
            .record(
                Line::new("https://b.test/", Status::None).with_reason("no_candidate"),
                None,
            )
            .unwrap();
        assert!(!super::dir(root).join(file_name("https://b.test/")).exists());
        let log = read(root).unwrap();
        assert_eq!((log.pages.len(), log.unreadable), (2, 0));
        assert_eq!(
            fs::read_to_string(log_path(root)).unwrap().lines().count(),
            4
        );
    }

    #[cfg(unix)]
    #[test]
    fn reopening_a_store_repairs_existing_modes_without_rewriting_images() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let image = super::dir(root).join(file_name("https://a.test/"));
        Store::open(root)
            .unwrap()
            .record(ok_line("https://a.test/"), Some(b"jpeg"))
            .unwrap();
        let before = fs::metadata(&image).unwrap();
        for (path, mode) in [
            (root.join(metadata::DIR), 0o755),
            (super::dir(root), 0o755),
            (image.clone(), 0o644),
            (log_path(root), 0o644),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        }
        let held = archive::Archive::at(root).lock(|| {}).unwrap();
        let (ready, started) = std::sync::mpsc::channel();
        let opening = std::thread::spawn({
            let root = root.to_path_buf();
            move || {
                ready.send(()).unwrap();
                Store::open(&root)
            }
        });
        started.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(
            fs::metadata(&image).unwrap().permissions().mode() & 0o777,
            0o644
        );
        drop(held);
        opening.join().unwrap().unwrap();
        assert_eq!(
            fs::metadata(root.join(metadata::DIR))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "a directory the store does not own is left alone"
        );
        assert_eq!(
            fs::metadata(super::dir(root)).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [&image, &log_path(root)] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let after = fs::metadata(&image).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert_eq!(fs::read(image).unwrap(), b"jpeg");
    }

    #[cfg(unix)]
    #[test]
    fn images_and_their_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        archive::create_private_dir(&root).unwrap();
        let mut store = Store::open(&root).unwrap();
        store
            .record(ok_line("https://a.test/"), Some(b"jpeg"))
            .unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&super::dir(&root)), 0o700);
        assert_eq!(mode(&log_path(&root)), 0o600);
        assert_eq!(
            mode(&super::dir(&root).join(file_name("https://a.test/"))),
            0o600
        );
    }
}
