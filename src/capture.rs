//! `knowmoretabs save`, end to end: find the session, refuse if stale, read
//! it stably, parse, skip if unchanged, stage, publish.
//!
//! slice: capture
//! why: The order of these steps is the product's safety story. Discovery
//!      and the staleness check run before the archive is touched, so a
//!      machine without Chrome never gets an empty archive; the lock is taken
//!      before anything is read, so two runs never interleave; the copy is
//!      verified stable before it is parsed, so what we archive is what we
//!      describe; and the unchanged-session check runs before staging, so a
//!      no-op run leaves no trace.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::archive::{Archive, SNAPSHOT_JSON};
use crate::error::Error;
use crate::model::{SCHEMA_VERSION, SESSION_FILE_NAME, Snapshot, Source};
use crate::platform::{self, Profile, SESSIONS_DIR};
use crate::session::{self, CommandTable, Parsed};
use crate::snss::HeaderError;
use crate::staleness;

/// Everything the CLI decides; nothing here reads flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub session: Option<PathBuf>,
    pub profile: Option<String>,
    pub user_data_dir: Option<PathBuf>,
    pub force: bool,
}

/// Where progress and warnings go. Warnings always reach stderr unless
/// `quiet`; notes only under `verbose`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Log {
    pub quiet: bool,
    pub verbose: bool,
}

impl Log {
    pub fn warn(self, message: &str) {
        if !self.quiet {
            eprintln!("knowmoretabs: {message}");
        }
    }

    pub fn note(self, message: &str) {
        if self.verbose {
            eprintln!("knowmoretabs: {message}");
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Saved {
        path: PathBuf,
        snapshot: Snapshot,
    },
    /// The layout matched `previous_id`; `snapshot` was built but not written.
    Skipped {
        previous_id: String,
        snapshot: Snapshot,
    },
}

/// A session file to try, with what we know about where it came from.
#[derive(Debug)]
struct Located {
    candidates: Vec<PathBuf>,
    browser: Option<String>,
    profile: Option<Profile>,
    /// The directory whose `Sessions_Encrypted/` sibling the staleness
    /// check inspects, when there is one.
    profile_dir: Option<PathBuf>,
    /// Where discovery looked, for the error when it found nothing.
    sessions_dir: Option<PathBuf>,
}

/// Set in debug builds by the atomicity test to simulate a kill after
/// staging and before the rename. Release builds ignore it.
pub const CRASH_BEFORE_PUBLISH_ENV: &str = "KNOWMORETABS_CRASH_BEFORE_PUBLISH";

pub fn save(opts: &Options, log: Log) -> Result<Outcome, Error> {
    let located = locate(opts, log)?;
    // Staleness comes before "no session file": an emptied `Sessions/`
    // beside a populated `Sessions_Encrypted/` is the migration, not a
    // machine without Chrome.
    if let Some(dir) = &located.profile_dir
        && let Some(reason) = staleness::check(dir).map_err(Error::io("inspect", dir))?
    {
        return Err(Error::Stale {
            profile: dir.clone(),
            reason,
        });
    }
    if let Some(sessions_dir) = &located.sessions_dir
        && located.candidates.is_empty()
    {
        return Err(Error::NoSession(sessions_dir.clone()));
    }

    let archive = Archive::open(&opts.root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let removed = archive.clean_stale_staging()?;
    if removed > 0 {
        log.note(&format!(
            "removed {removed} stale staging director{}",
            if removed == 1 { "y" } else { "ies" }
        ));
    }

    let read = read_first_parseable(&located.candidates, log)?;
    let captured_at = Timestamp::now();
    let profile = located.profile.as_ref();
    let mut snapshot = Snapshot {
        schema_version: SCHEMA_VERSION,
        id: String::new(),
        captured_at,
        source: Source {
            browser: located.browser.clone(),
            profile: profile.map(|p| p.dir_name.clone()),
            profile_display: profile.and_then(|p| p.display.clone()),
            path: read.path.clone(),
            file: SESSION_FILE_NAME.to_owned(),
            sha256: format!("{:x}", Sha256::digest(&read.bytes)),
            bytes: read.bytes.len() as u64,
            saved_at: read.modified.and_then(|t| Timestamp::try_from(t).ok()),
            session_started_at: read
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| platform::session_suffix(n, "Session_"))
                .and_then(platform::suffix_timestamp),
        },
        stats: read.parsed.stats,
        windows: read.parsed.windows,
        groups: read.parsed.groups,
        tabs: read.parsed.tabs,
    };

    if !opts.force {
        let previous = archive.latest_matching(
            snapshot.source.browser.as_deref(),
            snapshot.source.profile.as_deref(),
        )?;
        for path in &previous.unreadable {
            log.warn(&format!("ignoring unreadable snapshot {}", path.display()));
        }
        if let Some(previous) = previous.snapshot
            && previous.layout() == snapshot.layout()
        {
            return Ok(Outcome::Skipped {
                previous_id: previous.id,
                snapshot,
            });
        }
    }

    snapshot.id = archive.allocate_id(captured_at)?;
    let staging = archive.stage()?;
    staging.write(SESSION_FILE_NAME, &read.bytes)?;
    let json = serde_json::to_vec_pretty(&snapshot).map_err(|source| Error::Json {
        path: staging.path().join(SNAPSHOT_JSON),
        source,
    })?;
    staging.write(SNAPSHOT_JSON, &json)?;
    if cfg!(debug_assertions) && std::env::var_os(CRASH_BEFORE_PUBLISH_ENV).is_some() {
        std::process::exit(70);
    }
    let path = archive.publish(staging, &snapshot.id)?;
    Ok(Outcome::Saved { path, snapshot })
}

fn locate(opts: &Options, log: Log) -> Result<Located, Error> {
    if let Some(session) = &opts.session {
        let name = session.file_name().and_then(|n| n.to_str()).unwrap_or("");
        match CommandTable::for_file_name(name) {
            Some(CommandTable::Tabs) => return Err(Error::TabsFile(session.clone())),
            Some(CommandTable::Session) => {}
            None => log.note(&format!(
                "{name} has no Session_/Tabs_ prefix; reading it with the Session_ command table"
            )),
        }
        // A file still inside Chrome's own Sessions/ directory has an
        // encrypted sibling worth checking; a copy elsewhere does not.
        let profile_dir = session
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == SESSIONS_DIR))
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        return Ok(Located {
            candidates: vec![session.clone()],
            browser: None,
            profile: None,
            profile_dir,
            sessions_dir: None,
        });
    }

    let user_data = if let Some(dir) = &opts.user_data_dir {
        dir.clone()
    } else {
        let home = platform::home_dir().ok_or(Error::NoHome)?;
        platform::browser(platform::CHROME)
            .map(|b| b.user_data_dir(&home))
            .ok_or(Error::NoHome)?
    };
    let profile = platform::resolve_profile(&user_data, opts.profile.as_deref())?;
    log.note(&format!(
        "profile {} ({}) under {}",
        profile.dir_name,
        profile.display.as_deref().unwrap_or("no display name"),
        user_data.display()
    ));
    let sessions_dir = profile.path.join(SESSIONS_DIR);
    let candidates =
        platform::session_candidates(&sessions_dir).map_err(Error::io("list", &sessions_dir))?;
    Ok(Located {
        candidates: candidates.into_iter().map(|c| c.path).collect(),
        browser: Some(platform::CHROME.to_owned()),
        profile_dir: Some(profile.path.clone()),
        profile: Some(profile),
        sessions_dir: Some(sessions_dir),
    })
}

#[derive(Debug)]
struct SessionRead {
    path: PathBuf,
    bytes: Vec<u8>,
    modified: Option<std::time::SystemTime>,
    parsed: Parsed,
}

/// Chrome's own `FindLastSessionFile` walks newest-first and takes the first
/// readable file. A candidate whose header is bad is skipped with a warning;
/// an I/O failure is not, because it says nothing about the next file.
fn read_first_parseable(candidates: &[PathBuf], log: Log) -> Result<SessionRead, Error> {
    let mut first_failure: Option<(PathBuf, HeaderError)> = None;
    for path in candidates {
        let (bytes, modified) = read_stable(path)?;
        match session::parse(&bytes) {
            Ok(parsed) => {
                if let Some((skipped, reason)) = &first_failure {
                    log.warn(&format!(
                        "skipped newer file {}: {reason}",
                        skipped.display()
                    ));
                }
                return Ok(SessionRead {
                    path: path.clone(),
                    bytes,
                    modified,
                    parsed,
                });
            }
            Err(source) => {
                log.note(&format!("{}: {source}", path.display()));
                if first_failure.is_none() {
                    first_failure = Some((path.clone(), source));
                }
            }
        }
    }
    // Every candidate failed; the newest one's failure is the one to show.
    match first_failure {
        Some((path, source)) => Err(Error::Parse { path, source }),
        None => Err(Error::NoSession(PathBuf::new())),
    }
}

/// Reads the whole file and proves it did not change underneath: the
/// metadata before, after, and by path must agree, and the byte count must
/// match. Three attempts, then a clean failure.
fn read_stable(path: &Path) -> Result<(Vec<u8>, Option<std::time::SystemTime>), Error> {
    for _ in 0..3 {
        let mut file = File::open(path).map_err(Error::io("read", path))?;
        let before = file.metadata().map_err(Error::io("read", path))?;
        let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
        file.read_to_end(&mut bytes)
            .map_err(Error::io("read", path))?;
        let after = file.metadata().map_err(Error::io("read", path))?;
        let current = fs::metadata(path).map_err(Error::io("read", path))?;
        if same_file_state(&before, &after)
            && same_file_state(&after, &current)
            && bytes.len() as u64 == after.len()
        {
            return Ok((bytes, after.modified().ok()));
        }
    }
    Err(Error::Unstable(path.to_path_buf()))
}

fn same_file_state(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    let basic = a.len() == b.len() && a.modified().ok() == b.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        basic && a.ino() == b.ino() && a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        basic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_read_returns_the_bytes_and_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Session_1");
        fs::write(&path, b"SNSS\x03\0\0\0").unwrap();
        let (bytes, modified) = read_stable(&path).unwrap();
        assert_eq!(bytes, b"SNSS\x03\0\0\0");
        assert!(modified.is_some());
        assert!(matches!(
            read_stable(&tmp.path().join("missing")),
            Err(Error::Io { what: "read", .. })
        ));
    }

    #[test]
    fn newest_unparseable_candidate_falls_back_to_the_next() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("Session_2");
        let good = tmp.path().join("Session_1");
        fs::write(&bad, b"garbage!").unwrap();
        fs::write(&good, b"SNSS\x03\0\0\0").unwrap();
        let read = read_first_parseable(&[bad.clone(), good.clone()], Log::default()).unwrap();
        assert_eq!(read.path, good);
        let err = read_first_parseable(std::slice::from_ref(&bad), Log::default()).unwrap_err();
        assert!(matches!(err, Error::Parse { path, .. } if path == bad));
    }

    #[test]
    fn explicit_session_infers_profile_dir_only_inside_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let inside = tmp
            .path()
            .join("Profile 1")
            .join(SESSIONS_DIR)
            .join("Session_5");
        let opts = Options {
            root: tmp.path().join("root"),
            session: Some(inside.clone()),
            profile: None,
            user_data_dir: None,
            force: false,
        };
        let located = locate(&opts, Log::default()).unwrap();
        assert_eq!(located.profile_dir, Some(tmp.path().join("Profile 1")));
        assert_eq!(located.candidates, vec![inside]);
        assert!(located.browser.is_none());

        let copy = tmp.path().join("backup").join("Session_5");
        let located = locate(
            &Options {
                session: Some(copy),
                ..opts.clone()
            },
            Log::default(),
        )
        .unwrap();
        assert_eq!(located.profile_dir, None);

        let tabs = tmp.path().join("Tabs_5");
        let err = locate(
            &Options {
                session: Some(tabs),
                ..opts
            },
            Log::default(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::TabsFile(_)));
    }
}
