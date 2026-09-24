//! `pages/history.json`, the library's own record of History signals, and
//! `knowmoretabs history`, which shows it or refreshes it.
//!
//! slice: capture, library
//! why: A snapshot's signals cover only the tabs open during that save, and
//!      snapshots never change, so most pages in a library would never carry
//!      any. This file is a refreshed view beside them: every written save,
//!      and `history --refresh`, looks up every page the library knows in the
//!      same private copy of History and merges what it found. Chrome forgets
//!      visits after about 90 days, so an entry History no longer knows is
//!      kept, with the time History last knew it, rather than dropped. The
//!      copy and the queries are `history.rs`; this is which URLs, the merge,
//!      and the atomic write under the archive lock.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Archive};
use crate::capture::{self, Log};
use crate::error::Error;
use crate::history::{self, Reading};
use crate::library::{self, State};
use crate::metadata;
use crate::model::{HistorySource, Tab, TabHistory};
use crate::out;
use crate::triage::plural;

pub const FILE: &str = "history.json";
const SCHEMA_VERSION: u32 = 1;

pub fn path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(FILE)
}

/// The whole file. Not append-only: each refresh rewrites it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Recorded {
    schema_version: u32,
    pub updated_at: Timestamp,
    /// The copy of History the latest refresh read, in a snapshot's shape;
    /// `tabs_found` counts the library pages it found.
    pub source: HistorySource,
    pub pages: BTreeMap<String, Entry>,
}

/// One page's signals, as a tab in a snapshot carries them, and when History
/// last knew the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub history: TabHistory,
    /// The latest refresh whose copy of History knew this URL. Older than
    /// the file's `updated_at` means History has forgotten it since.
    pub refreshed_at: Timestamp,
}

/// `None` when no refresh has written the file yet. A file this build cannot
/// read is an error: a refresh over it would drop every page History has
/// forgotten.
pub fn read(root: &Path) -> Result<Option<Recorded>, Error> {
    let path = path(root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::io("read", &path)(err)),
    };
    let recorded: Recorded =
        serde_json::from_slice(&bytes).map_err(|err| Error::HistoryRecord {
            path: path.clone(),
            reason: err.to_string(),
        })?;
    if recorded.schema_version != SCHEMA_VERSION {
        return Err(Error::HistoryRecord {
            path,
            reason: "unsupported schema_version".to_owned(),
        });
    }
    Ok(Some(recorded))
}

/// The entries the library shows, empty when there are none. A file that
/// cannot be read costs its entries, not the library: each page falls back
/// to the signals its snapshots recorded.
pub fn for_library(root: &Path, log: Log) -> BTreeMap<String, Entry> {
    match read(root) {
        Ok(recorded) => recorded.map(|recorded| recorded.pages).unwrap_or_default(),
        Err(err) => {
            log.warn(&format!(
                "{err}; pages show the History signals their snapshots recorded"
            ));
            BTreeMap::new()
        }
    }
}

/// A refresh that knows what to look up and is waiting for History.
#[derive(Debug)]
pub struct Pending {
    previous: Option<Recorded>,
    pub urls: BTreeSet<String>,
}

/// What a refresh did, for the report.
#[derive(Debug, Serialize)]
pub struct Refreshed {
    pub path: PathBuf,
    /// Library pages looked up.
    pub pages: usize,
    /// Of those, the ones History knew.
    pub found: usize,
    /// Looked up, not found, and kept from an earlier refresh.
    pub kept: usize,
    /// Entries in the file now.
    pub recorded: usize,
}

/// Reads what a refresh needs before History is copied: the library's
/// pages, the forgotten set, and the record as it stands. `also` are tabs
/// not yet in the archive: the snapshot `save` is about to publish.
pub fn prepare(root: &Path, archive: &Archive, also: &[Tab]) -> Result<Pending, Error> {
    let previous = read(root)?;
    let state = State::read(root)?;
    let loaded = library::load(archive)?;
    let tabs = loaded
        .snapshots
        .iter()
        .flat_map(|snapshot| &snapshot.tabs)
        .chain(also);
    Ok(Pending {
        previous,
        urls: library::history_urls(tabs, &state.forgotten),
    })
}

impl Pending {
    /// Merges `reading` into the record and replaces the file. A page the
    /// reading knows gets its new signals and `refreshed_at`; one it does not
    /// know keeps whatever entry it had. Caller holds the archive lock across
    /// [`prepare`] and this, and has checked that the reading has no error.
    pub fn finish(self, root: &Path, reading: &Reading, at: Timestamp) -> Result<Refreshed, Error> {
        let mut pages = self
            .previous
            .map(|previous| previous.pages)
            .unwrap_or_default();
        let (mut found, mut kept) = (0, 0);
        for url in &self.urls {
            if let Some(history) = reading.signals.get(url) {
                found += 1;
                pages.insert(
                    url.clone(),
                    Entry {
                        history: history.clone(),
                        refreshed_at: at,
                    },
                );
            } else if pages.contains_key(url) {
                kept += 1;
            }
        }
        let mut source = reading.source.clone();
        source.tabs_found = found as u64;
        let recorded = Recorded {
            schema_version: SCHEMA_VERSION,
            updated_at: at,
            source,
            pages,
        };
        let path = path(root);
        write(&path, &recorded)?;
        Ok(Refreshed {
            path,
            pages: self.urls.len(),
            found,
            kept,
            recorded: recorded.pages.len(),
        })
    }
}

/// Atomically, into a private `pages/`, as `library.json` is written.
fn write(path: &Path, recorded: &Recorded) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        archive::create_private_dir(dir).map_err(Error::io("create", dir))?;
    }
    let mut bytes = serde_json::to_vec_pretty(recorded).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    archive::replace_file(path, &bytes)
}

/// `knowmoretabs history --refresh`: the refresh a written save does, without
/// a save. History comes from the profile `save` would read, found the same
/// way and with the same flags. Unlike in `save`, a History that cannot be
/// read is an error here, because reading it is the whole command.
pub fn refresh_command(opts: &capture::Options, json: bool, log: Log) -> Result<(), Error> {
    let profile_dir = capture::profile_dir(opts, log)?;
    let archive = Archive::open(&opts.root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    archive.clean_stale_staging()?;
    let pending = prepare(&opts.root, &archive, &[])?;
    let urls: BTreeSet<&str> = pending.urls.iter().map(String::as_str).collect();
    let reading = history::read_urls(profile_dir.as_deref(), &archive, &urls);
    if let Some(reason) = &reading.source.error {
        return Err(Error::HistoryUnreadable(reason.clone()));
    }
    let refreshed = pending.finish(&opts.root, &reading, Timestamp::now())?;
    if json {
        out::json(&serde_json::json!({
            "refreshed": refreshed,
            "source": reading.source,
        }));
    } else if !log.quiet {
        out::line(&describe(&refreshed));
    }
    Ok(())
}

/// One line for a person: what was found and what was kept.
pub fn describe(refreshed: &Refreshed) -> String {
    let mut line = format!(
        "History signals for {} of {} written to {}",
        refreshed.found,
        plural(refreshed.pages, "library page"),
        refreshed.path.display()
    );
    if refreshed.kept > 0 {
        let _ = write!(
            line,
            "; kept the earlier signals of {} History no longer has",
            plural(refreshed.kept, "page")
        );
    }
    line
}

/// `knowmoretabs history`: what the record holds and how current it is.
pub fn status_command(root: &Path, json: bool, log: Log) -> Result<(), Error> {
    let path = path(root);
    let recorded = read(root)?;
    let forgotten_since = |recorded: &Recorded| {
        recorded
            .pages
            .values()
            .filter(|entry| entry.refreshed_at < recorded.updated_at)
            .count()
    };
    if json {
        out::json(&serde_json::json!({
            "history": recorded.as_ref().map(|recorded| serde_json::json!({
                "path": path,
                "updated_at": recorded.updated_at,
                "pages": recorded.pages.len(),
                "not_in_history": forgotten_since(recorded),
                "source": recorded.source,
            })),
        }));
        return Ok(());
    }
    if log.quiet {
        return Ok(());
    }
    let Some(recorded) = recorded else {
        out::line(&format!(
            "No History signals recorded for the library yet: {} does not exist. Run knowmoretabs history --refresh.",
            path.display()
        ));
        return Ok(());
    };
    let mut line = format!(
        "{} with History signals in {}, refreshed {}",
        plural(recorded.pages.len(), "page"),
        path.display(),
        recorded.updated_at
    );
    if let Some(source) = &recorded.source.path {
        let _ = write!(line, " from {}", source.display());
    }
    let stale = forgotten_since(&recorded);
    if stale > 0 {
        let _ = write!(
            line,
            "; {stale} of them no longer in History, kept from earlier refreshes"
        );
    }
    out::line(&line);
    Ok(())
}
