//! `knowmoretabs save`, end to end: find the session, refuse if stale, read
//! it stably, parse, skip if unchanged, read History, stage, publish.
//!
//! slice: capture, browsers
//! why: The order of these steps is the product's safety story. Discovery
//!      and the staleness check run before the archive is touched, so a
//!      machine without Chrome never gets an empty archive; the lock is taken
//!      before anything is read, so two runs never interleave; the copy is
//!      verified stable before it is parsed, so what we archive is what we
//!      describe; and the unchanged-session check runs before History is
//!      read and before staging, so a no-op run costs and leaves nothing.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::archive::{Archive, SNAPSHOT_JSON};
use crate::error::Error;
use crate::history;
use crate::local;
use crate::model::{SCHEMA_VERSION, SESSION_FILE_NAME, Snapshot, Source};
use crate::platform::{self, BrowserCandidate, Profile, SESSIONS_DIR};
use crate::session::{self, CommandTable, Parsed};
use crate::snss::HeaderError;
use crate::staleness;

/// Everything the CLI decides; nothing here reads flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub session: Option<PathBuf>,
    pub browser: Option<String>,
    pub profile: Option<String>,
    pub user_data_dir: Option<PathBuf>,
    pub force: bool,
    /// Read the browser's `History` for each tab; `--no-history` clears it.
    pub history: bool,
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
            emit(message);
        }
    }

    pub fn note(self, message: &str) {
        if self.verbose {
            emit(message);
        }
    }
}

/// Through `out` rather than `eprintln!`, which would panic if stderr had
/// gone away. `serve` logs from the connection thread before it answers, so
/// that panic unwound the thread and handed the client a closed socket
/// instead of its response. Losing the log line is the lesser harm.
fn emit(message: &str) {
    crate::out::problem(&format!("knowmoretabs: {message}"));
}

#[derive(Debug)]
pub enum Outcome {
    Saved {
        path: PathBuf,
        snapshot: Snapshot,
        also_found: Vec<BrowserCandidate>,
    },
    /// The layout matched `previous_id`; `snapshot` was built but not written.
    Skipped {
        previous_id: String,
        snapshot: Snapshot,
        also_found: Vec<BrowserCandidate>,
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
    also_found: Vec<BrowserCandidate>,
}

/// Debug-test rendezvous: write a readiness file, then wait for the parent
/// to kill this process after staging. Release builds ignore it.
pub const PAUSE_BEFORE_PUBLISH_ENV: &str = "KNOWMORETABS_PAUSE_BEFORE_PUBLISH";

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
        history: None,
    };
    leave_out_this_machine(&mut snapshot);

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
                also_found: located.also_found,
            });
        }
    }

    // After the unchanged check, so a skipped run reads nothing; before
    // staging, so the History copy never shares a directory with the snapshot.
    add_history(
        &mut snapshot,
        opts.history,
        located.profile_dir.as_deref(),
        &archive,
        log,
    );

    snapshot.id = archive.allocate_id(captured_at)?;
    let staging = archive.stage()?;
    staging.write(SESSION_FILE_NAME, &read.bytes)?;
    let json = serde_json::to_vec_pretty(&snapshot).map_err(|source| Error::Json {
        path: staging.path().join(SNAPSHOT_JSON),
        source,
    })?;
    staging.write(SNAPSHOT_JSON, &json)?;
    if cfg!(debug_assertions)
        && let Some(ready) = std::env::var_os(PAUSE_BEFORE_PUBLISH_ENV)
    {
        fs::write(&ready, []).map_err(Error::io("signal staged snapshot", Path::new(&ready)))?;
        loop {
            std::thread::park();
        }
    }
    let path = archive.publish(staging, &snapshot.id)?;
    Ok(Outcome::Saved {
        path,
        snapshot,
        also_found: located.also_found,
    })
}

/// Reads History into the snapshot, or records that `--no-history` asked
/// for it not to be, and says which in one line: a warning when History
/// could not be read, because the snapshot is missing something it would
/// otherwise have, and a note under `-v` otherwise. Reading and saying so are
/// one step, so that no run reads History without the note.
fn add_history(
    snapshot: &mut Snapshot,
    read: bool,
    profile_dir: Option<&Path>,
    archive: &Archive,
    log: Log,
) {
    if read {
        history::record(snapshot, profile_dir, archive);
    } else {
        snapshot.history = Some(history::skipped(profile_dir));
    }
    let Some(source) = &snapshot.history else {
        return;
    };
    if let Some(reason) = &source.error {
        log.warn(&format!(
            "History not read, so this snapshot has no History signals: {reason}"
        ));
    } else if source.skipped_by_request {
        log.note("History not read (--no-history)");
    } else {
        log.note(&format!(
            "History read from {}: {} of {} tabs found{}",
            source
                .path
                .as_deref()
                .map_or_else(String::new, |p| p.display().to_string()),
            source.tabs_found,
            snapshot.tabs.len(),
            if source.unavailable.is_empty() {
                String::new()
            } else {
                format!("; not available: {}", source.unavailable.join(", "))
            }
        ));
    }
}

/// Removes tabs on this machine (development servers, this tool's own library)
/// and makes the rest read as if they had never been open: a window left
/// empty is dropped and the rest renumbered, positions close up, an active
/// tab that was removed leaves its window with none, and a group left without
/// tabs is dropped. The unchanged-session check then sees opening or closing
/// a localhost tab as no change at all. `session.snss` stays verbatim.
fn leave_out_this_machine(snapshot: &mut Snapshot) {
    let before = snapshot.tabs.len();
    snapshot
        .tabs
        .retain(|tab| !local::is_this_machine_url(&tab.url));
    let excluded = before - snapshot.tabs.len();
    if excluded == 0 {
        return;
    }

    // Windows that still hold a tab, or held none to begin with, keep their
    // order and take consecutive numbers.
    let occupied: HashSet<u32> = snapshot.tabs.iter().map(|tab| tab.window).collect();
    snapshot
        .windows
        .retain(|window| window.tabs == 0 || occupied.contains(&window.number));
    let renumber: HashMap<u32, u32> = snapshot
        .windows
        .iter()
        .zip(1..)
        .map(|(window, number)| (window.number, number))
        .collect();
    for tab in &mut snapshot.tabs {
        tab.window = renumber[&tab.window];
    }
    snapshot.groups.retain(|group| {
        snapshot
            .tabs
            .iter()
            .any(|tab| tab.group.as_ref() == Some(&group.id))
    });
    for group in &mut snapshot.groups {
        if let Some(&number) = renumber.get(&group.window) {
            group.window = number;
        }
    }

    // The parser sorted tabs by window and position; close the gaps in the
    // known positions and leave unknown ones (-1) as they were.
    let mut next: HashMap<u32, i32> = HashMap::new();
    for tab in &mut snapshot.tabs {
        if tab.position >= 0 {
            let position = next.entry(tab.window).or_default();
            tab.position = *position;
            *position += 1;
        }
    }
    let kept: HashSet<i32> = snapshot.tabs.iter().map(|tab| tab.tab_id).collect();
    for window in &mut snapshot.windows {
        window.number = renumber[&window.number];
        window.tabs = u32::try_from(
            snapshot
                .tabs
                .iter()
                .filter(|t| t.window == window.number)
                .count(),
        )
        .unwrap_or(u32::MAX);
        window.active_tab = window.active_tab.filter(|id| kept.contains(id));
    }

    let stats = &mut snapshot.stats;
    stats.windows = snapshot.windows.len() as u64;
    stats.tabs = snapshot.tabs.len() as u64;
    stats.groups = snapshot.groups.len() as u64;
    stats.excluded_tabs = excluded as u64;
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
            also_found: Vec::new(),
        });
    }

    let roots = platform::Roots::detect().ok_or(Error::NoHome)?;
    let spec = match opts.browser.as_deref() {
        Some("arc") => return Err(Error::ArcUnsupported),
        Some(id) => platform::browser(id).ok_or_else(|| Error::UnknownBrowser {
            requested: id.to_owned(),
            installed: installed_names(&roots),
        })?,
        None => platform::browser(platform::CHROME).ok_or(Error::NoHome)?,
    };
    let explicit = opts.browser.is_some() || opts.user_data_dir.is_some();

    let scan = discover_candidates(opts, &roots, spec, log)?;
    let mut candidates = scan.candidates;

    if candidates.is_empty() {
        if explicit {
            let Some(user_data) = scan.user_data.iter().find(|d| d.is_dir()).cloned() else {
                return Err(Error::BrowserNotInstalled {
                    requested: spec.id.to_owned(),
                    paths: join_paths(&scan.user_data, &roots.home),
                    installed: installed_names(&roots),
                });
            };
            let profile = platform::resolve_profile(&user_data, opts.profile.as_deref())?;
            return Ok(Located {
                candidates: Vec::new(),
                browser: Some(spec.id.to_owned()),
                profile: Some(profile.clone()),
                profile_dir: Some(profile.path.clone()),
                sessions_dir: Some(profile.path.join(SESSIONS_DIR)),
                also_found: Vec::new(),
            });
        }
        return Err(Error::NoBrowserSession {
            looked_at: group_by_reason(scan.looked_at),
        });
    }

    candidates.sort_by(platform::candidate_cmp);
    let winner = candidates.pop().expect("checked non-empty");
    // The browser used most recently is the one to refuse on, not to route
    // around: a stale winner means the user's real session is unreadable, and
    // saving the runner-up would present another browser as that session.
    if let Some(reason) = winner.stale {
        return Err(Error::Stale {
            profile: winner.profile.path,
            reason,
        });
    }
    let also_found = candidates;
    let profile = winner.profile.clone();
    let user_data = profile
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    log.note(&format!(
        "profile {} ({}) under {}",
        profile.dir_name,
        profile.display.as_deref().unwrap_or("no display name"),
        user_data.display()
    ));
    let sessions_dir = profile.path.join(SESSIONS_DIR);
    let session_candidates =
        platform::session_candidates(&sessions_dir).map_err(Error::io("list", &sessions_dir))?;
    Ok(Located {
        candidates: session_candidates.into_iter().map(|c| c.path).collect(),
        browser: Some(winner.browser.id.to_owned()),
        profile_dir: Some(profile.path.clone()),
        profile: Some(profile),
        sessions_dir: Some(sessions_dir),
        also_found,
    })
}

/// What the browser table yielded: the candidates worth ranking, the
/// user-data directories of the selected browser, and one line per browser
/// that produced nothing, saying why, for the error when nothing did.
struct Scan {
    candidates: Vec<BrowserCandidate>,
    user_data: Vec<PathBuf>,
    looked_at: Vec<(String, String)>,
}

/// Every directory a browser could be in, on one line. Linux gives a browser
/// up to four — native, two Snap layouts, Flatpak — and seven browsers is
/// sixteen paths, so the home directory is written `~` rather than repeated
/// sixteen times. The paths themselves are all there: a "we looked and found
/// nothing" error is only actionable if it says where it looked.
fn join_paths(paths: &[PathBuf], home: &Path) -> String {
    paths
        .iter()
        .map(|path| match path.strip_prefix(home) {
            Ok(rest) => Path::new("~").join(rest).display().to_string(),
            Err(_) => path.display().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `chrome (path), edge (path): no user-data directory; brave (path): …`.
/// Seven browsers usually fail for two or three reasons; one line per reason
/// keeps the error readable and still names every directory looked at.
fn group_by_reason(looked_at: Vec<(String, String)>) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (where_, reason) in looked_at {
        match groups.iter_mut().find(|(r, _)| *r == reason) {
            Some((_, places)) => places.push(where_),
            None => groups.push((reason, vec![where_])),
        }
    }
    groups
        .into_iter()
        .map(|(reason, places)| format!("{}: {reason}", places.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// One browser's answer. `Nothing` is the ordinary "not here"; `Failed` is
/// something about that browser's files that a person may want to fix.
enum Probe {
    Found(BrowserCandidate),
    Nothing(&'static str),
    Failed(Error),
}

/// The two ordinary "not here" answers, named because `probe_browser` has to
/// tell them apart when one browser has several directories.
const NOT_INSTALLED: &str = "no user-data directory";
const NO_SESSION_FILE: &str = "no Session_* file";

fn discover_candidates(
    opts: &Options,
    roots: &platform::Roots,
    selected: &'static platform::BrowserSpec,
    log: Log,
) -> Result<Scan, Error> {
    let requested_profile = opts.profile.as_deref();
    if opts.browser.is_some() || opts.user_data_dir.is_some() {
        let user_data = match &opts.user_data_dir {
            Some(dir) => vec![dir.clone()],
            None => selected.user_data_dirs(roots),
        };
        // Asked for by name: every failure is the user's to see.
        let candidates = match probe_browser(selected, &user_data, requested_profile) {
            Probe::Found(candidate) => vec![candidate],
            Probe::Nothing(_) => Vec::new(),
            Probe::Failed(error) => return Err(error),
        };
        return Ok(Scan {
            candidates,
            user_data,
            looked_at: Vec::new(),
        });
    }

    // Scanning: a browser that cannot be read is passed over, because the
    // user did not ask for it and another browser may be the one they use.
    // Its reason is kept for the error shown if no browser works out, and
    // shown under `-v` otherwise. Two exceptions are the user's own input and
    // are the same for every browser, so they stop the scan at once: a
    // pseudo-profile such as `Guest`, and a display name one browser cannot
    // tell apart, which is warned about rather than silently dropped because
    // the fix (`--browser NAME --profile DIR`) is browser-specific.
    let mut candidates = Vec::new();
    let mut looked_at = Vec::new();
    for spec in &platform::BROWSERS {
        let user_data = spec.user_data_dirs(roots);
        let where_ = format!("{} ({})", spec.id, join_paths(&user_data, &roots.home));
        match probe_browser(spec, &user_data, requested_profile) {
            Probe::Found(candidate) => candidates.push(candidate),
            Probe::Nothing(reason) => looked_at.push((where_, reason.to_owned())),
            Probe::Failed(error) => {
                if requested_profile.is_some() && is_pseudo_profile_error(&error) {
                    return Err(error);
                }
                if is_ambiguous_profile_error(&error) {
                    log.warn(&format!(
                        "{}: {error} (with --browser {})",
                        spec.id, spec.id
                    ));
                } else {
                    log.note(&format!("{}: skipped: {error}", spec.id));
                }
                looked_at.push((where_, error.to_string()));
            }
        }
    }
    Ok(Scan {
        candidates,
        user_data: selected.user_data_dirs(roots),
        looked_at,
    })
}

/// One browser across every directory its packaging can put it in. Linux is
/// the platform where that is more than one: a native, a Snap and a Flatpak
/// install of the same browser are three installs with three user-data
/// directories, and a machine can have two of them. The newest wins, for the
/// same reason the newest browser wins — it is the one the person was using.
/// Collapsing them here rather than in the scan keeps one candidate per
/// browser, so `--browser brave` still names one thing.
fn probe_browser(
    spec: &'static platform::BrowserSpec,
    dirs: &[PathBuf],
    requested_profile: Option<&str>,
) -> Probe {
    let mut best: Option<BrowserCandidate> = None;
    let mut failure: Option<Error> = None;
    let mut nothing = NOT_INSTALLED;
    for dir in dirs {
        match probe(spec, dir, requested_profile) {
            Probe::Found(candidate) => {
                if best
                    .as_ref()
                    .is_none_or(|b| platform::candidate_cmp(b, &candidate).is_lt())
                {
                    best = Some(candidate);
                }
            }
            // "installed but empty" says more than "not installed", so it
            // survives a sibling directory that is simply absent.
            Probe::Nothing(reason) => {
                if reason != NOT_INSTALLED {
                    nothing = reason;
                }
            }
            Probe::Failed(error) => failure = failure.or(Some(error)),
        }
    }
    match (best, failure) {
        (Some(candidate), _) => Probe::Found(candidate),
        (None, Some(error)) => Probe::Failed(error),
        (None, None) => Probe::Nothing(nothing),
    }
}

/// Everything known about one browser before any file is opened: whether it
/// is installed, which profile is current, whether that profile is stale, and
/// how recent it is. Staleness is decided here, per browser, so that a stale
/// profile competes on its real recency (its encrypted files) rather than on
/// the cleartext files that stopped moving.
fn probe(
    spec: &'static platform::BrowserSpec,
    user_data: &Path,
    requested_profile: Option<&str>,
) -> Probe {
    if !user_data.is_dir() {
        return Probe::Nothing(NOT_INSTALLED);
    }
    let (profile, sessions) = match platform::profile_with_session(user_data, requested_profile) {
        Ok(found) => found,
        Err(error) => return Probe::Failed(Error::Discovery(error)),
    };
    let stale = match staleness::check(&profile.path) {
        Ok(stale) => stale,
        Err(source) => return Probe::Failed(Error::io("inspect", &profile.path)(source)),
    };
    let (mut suffix, mut modified) = match sessions.first() {
        Some(newest) => (newest.suffix, platform::file_modified(&newest.path)),
        None if stale.is_some() => (0, None),
        None => return Probe::Nothing(NO_SESSION_FILE),
    };
    if stale.is_some()
        && let Some((encrypted_suffix, encrypted_modified)) =
            platform::encrypted_recency(&profile.path)
        && encrypted_suffix > suffix
    {
        suffix = encrypted_suffix;
        modified = encrypted_modified;
    }
    Probe::Found(BrowserCandidate {
        browser: spec,
        profile,
        suffix,
        modified,
        stale,
    })
}

fn is_pseudo_profile_error(error: &Error) -> bool {
    matches!(
        error,
        Error::Discovery(platform::DiscoveryError::Profile(
            platform::ProfileError::NotBrowsing(_)
        ))
    )
}

fn is_ambiguous_profile_error(error: &Error) -> bool {
    matches!(
        error,
        Error::Discovery(platform::DiscoveryError::Profile(
            platform::ProfileError::Ambiguous { .. }
        ))
    )
}

/// The browsers a person could pass to `--browser` and get a session from.
/// Judged by the same probe as the scan, so a leftover directory with no
/// profile in it (Chromium on the research machine) is not called installed.
fn installed_names(roots: &platform::Roots) -> String {
    let names: Vec<&str> = platform::BROWSERS
        .iter()
        .filter(|spec| {
            matches!(
                probe_browser(spec, &spec.user_data_dirs(roots), None),
                Probe::Found(_)
            )
        })
        .map(|spec| spec.id)
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
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

/// Length and mtime everywhere, plus whatever else the platform can say
/// cheaply about "this is still the same file, untouched". Unix adds the
/// inode and ctime; Windows has no stable inode on `Metadata` but does have
/// the creation time. NTFS tunnelling can preserve that on replacement, and
/// write times may lag while a writer stays open, so this is a best-effort
/// change detector, not a proof of identity. History also compares a second
/// copy on Windows before opening either copy with `SQLite`.
pub fn same_file_state(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    let basic = a.len() == b.len() && a.modified().ok() == b.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        basic && a.ino() == b.ino() && a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        basic && a.creation_time() == b.creation_time()
    }
    #[cfg(not(any(unix, windows)))]
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
            browser: None,
            profile: None,
            user_data_dir: None,
            force: false,
            history: true,
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
