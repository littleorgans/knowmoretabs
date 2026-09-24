//! The browser's own `History` database, read from a private copy: what it
//! knew about each tab's URL when the snapshot was taken.
//!
//! slice: capture
//! why: Chrome keeps about 90 days of visits and then forgets them; a
//!      snapshot keeps them for as long as the archive exists. Reading them
//!      must never put the browser's file at risk or cost the user a
//!      snapshot, so the live file is never opened, only copied with its
//!      journal and WAL into the archive's staging area and opened there, and
//!      every failure becomes a recorded reason instead of an error. What the
//!      rules are (three hops of search, positive foreground durations only,
//!      join by URL) and why is `docs/briefs/slice-07b-history.md`.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::archive::Archive;
use crate::local;
use crate::model::{HistorySource, Search, Snapshot, TabHistory};
use crate::session::chrome_time;

/// The database's name in every Chromium profile, on every platform.
pub const HISTORY_FILE: &str = "History";
/// Copied beside `History` when present, before it. The rollback journal
/// holds the originals of pages an uncommitted transaction has already
/// written into `History`; the WAL holds commits not yet in it. `-shm` is
/// never copied: opening the copy rebuilds it from the WAL.
const COMPANIONS: [&str; 2] = ["-journal", "-wal"];
/// How far back through the visits that led to a page a search counts.
pub const MAX_HOPS: u8 = 3;
/// Copies attempted before a History that keeps changing is given up on.
const ATTEMPTS: usize = 3;

/// Columns without which no signal can be read. They predate schema 15, the
/// oldest History Chromium still migrates.
const REQUIRED: [(&str, &[&str]); 2] = [
    (
        "urls",
        &["id", "url", "visit_count", "typed_count", "last_visit_time"],
    ),
    ("visits", &["id", "url", "visit_time", "from_visit"]),
];

/// Columns that arrived later, each feeding one signal. A database without
/// one gives that signal up and says so in `unavailable`.
const OPENER: (&str, &str) = ("visits", "opener_visit");
const EXTERNAL_REFERRER: (&str, &str) = ("visits", "external_referrer_url");
const FOREGROUND: [(&str, &str); 2] = [
    ("context_annotations", "visit_id"),
    ("context_annotations", "total_foreground_duration"),
];
const SEARCH_TERMS: [(&str, &str); 2] = [
    ("keyword_search_terms", "url_id"),
    ("keyword_search_terms", "term"),
];

/// `History` beside a profile directory.
pub fn path_in(profile_dir: &Path) -> PathBuf {
    profile_dir.join(HISTORY_FILE)
}

/// What `save --no-history` records: nothing read, and that it was asked for.
pub fn skipped(profile_dir: Option<&Path>) -> HistorySource {
    HistorySource {
        path: profile_dir.map(path_in),
        skipped_by_request: true,
        ..HistorySource::default()
    }
}

/// Gives each tab whose URL History knows its signals, and the snapshot a
/// record of where they came from. Never fails: when History cannot be read,
/// the reason is in `error` and no tab has signals.
pub fn record(snapshot: &mut Snapshot, profile_dir: Option<&Path>, archive: &Archive) {
    let mut source = HistorySource {
        path: profile_dir.map(path_in),
        ..HistorySource::default()
    };
    let Some(path) = source.path.clone() else {
        source.error = Some(
            "the session file is not inside a browser profile, so there is no History beside it"
                .to_owned(),
        );
        snapshot.history = Some(source);
        return;
    };
    let urls: BTreeSet<&str> = snapshot.tabs.iter().map(|tab| tab.url.as_str()).collect();
    match read(&path, archive, &urls) {
        Ok((bytes, found)) => {
            for tab in &mut snapshot.tabs {
                tab.history = found.signals.get(&tab.url).cloned();
                if tab.history.is_some() {
                    source.tabs_found += 1;
                }
            }
            source.bytes = Some(bytes);
            source.schema_version = found.schema_version;
            source.newest_visit = found.newest_visit;
            source.unavailable = found.unavailable;
        }
        Err(reason) => source.error = Some(reason),
    }
    snapshot.history = Some(source);
}

struct Found {
    schema_version: Option<u32>,
    newest_visit: Option<Timestamp>,
    unavailable: Vec<String>,
    signals: HashMap<String, TabHistory>,
}

/// Copies, opens the copy, reads, and removes the copy. The scratch
/// directory is a staging directory, so a run killed in between leaves
/// nothing the next run's `clean_stale_staging` does not remove. Returns
/// the size of the copy with what was found in it.
fn read(path: &Path, archive: &Archive, urls: &BTreeSet<&str>) -> Result<(u64, Found), String> {
    let scratch = archive.stage().map_err(|err| err.to_string())?;
    let (copy, bytes) = copy_stable(path, scratch.path())?;
    // Read-write, so that SQLite rolls back a hot journal or replays a WAL;
    // read-only, it refuses to do either. Never created: a copy that is not
    // there is an error, not an empty database.
    let conn = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| unreadable(path, &err))?;
    let result = query(&conn, urls)
        .map(|found| (bytes, found))
        .map_err(|problem| match problem {
            Problem::Missing(what) => format!("{} has no {what}", path.display()),
            Problem::Sqlite(err) => unreadable(path, &err),
        });
    // Closed before the scratch directory goes, so that Windows, which will
    // not delete an open file, can remove the copy.
    drop(conn.close());
    drop(scratch);
    result
}

fn unreadable(path: &Path, err: &rusqlite::Error) -> String {
    format!("cannot read the copy of {}: {err}", path.display())
}

/// Copies `History` and whichever companions exist into `into`, companions
/// first, and proves none of the three moved while it did: their length and
/// modification time, and the file's identity, are the same before and
/// after. Three attempts, as for the session file. Returns the copy and its
/// size.
fn copy_stable(source: &Path, into: &Path) -> Result<(PathBuf, u64), String> {
    copy_stable_with(source, into, cfg!(windows), |from, to| fs::copy(from, to))
}

fn copy_stable_with(
    source: &Path,
    into: &Path,
    verify_contents: bool,
    mut copy: impl FnMut(&Path, &Path) -> std::io::Result<u64>,
) -> Result<(PathBuf, u64), String> {
    let name = source
        .file_name()
        .map_or_else(|| HISTORY_FILE.into(), ToOwned::to_owned);
    let mut files: Vec<(PathBuf, PathBuf)> = COMPANIONS
        .iter()
        .map(|suffix| {
            let mut companion = name.clone();
            companion.push(suffix);
            (source.with_file_name(&companion), into.join(&companion))
        })
        .collect();
    files.push((source.to_path_buf(), into.join(&name)));

    for _ in 0..ATTEMPTS {
        let before = states(&files)?;
        if before.last().is_some_and(Option::is_none) {
            return Err(format!("no History file at {}", source.display()));
        }
        let mut complete = true;
        for ((from, to), state) in files.iter().zip(&before) {
            // A companion from an earlier attempt that has since gone would
            // otherwise be read as belonging to this copy.
            remove_if_present(to)?;
            if state.is_none() {
                continue;
            }
            match copy(from, to) {
                Ok(bytes) => {
                    complete &= state.as_ref().is_some_and(|meta| meta.len() == bytes);
                    // CopyFileExW also copies the read-only attribute. Recovery
                    // must be able to write and cleanup must be able to delete.
                    make_copy_writable(to)?;
                }
                // Gone between the stat and the copy: the check below sees it.
                Err(err) if err.kind() == ErrorKind::NotFound => complete = false,
                Err(err) => return Err(format!("cannot copy {}: {err}", from.display())),
            }
        }
        // Windows may defer mtime updates until Chrome closes its writer.
        // Compare a second complete copy as well; metadata alone can accept
        // a same-sized write. Only scratch files are opened for comparison.
        if complete && verify_contents {
            for ((from, to), state) in files.iter().zip(&before) {
                if state.is_none() {
                    continue;
                }
                let verification = to.with_extension("verify");
                remove_if_present(&verification)?;
                match copy(from, &verification) {
                    Ok(_) => {
                        make_copy_writable(&verification)?;
                        complete &= copy_digest(to)? == copy_digest(&verification)?;
                    }
                    Err(err) if err.kind() == ErrorKind::NotFound => complete = false,
                    Err(err) => {
                        return Err(format!("cannot verify copy {}: {err}", from.display()));
                    }
                }
                remove_if_present(&verification)?;
            }
        }
        let after = states(&files)?;
        let unchanged = before.iter().zip(&after).all(|pair| match pair {
            (Some(a), Some(b)) => crate::capture::same_file_state(a, b),
            (None, None) => true,
            _ => false,
        });
        if complete
            && unchanged
            && let Some(Some(history)) = after.last()
        {
            return Ok((into.join(&name), history.len()));
        }
    }
    Err(format!(
        "{} kept changing while it was being copied",
        source.display()
    ))
}

fn copy_digest(path: &Path) -> Result<sha2::digest::Output<sha2::Sha256>, String> {
    use sha2::Digest;
    use std::io::Read;
    let digest = || -> std::io::Result<_> {
        let mut file = fs::File::open(path)?;
        let mut hash = sha2::Sha256::new();
        let mut buffer = [0; 16 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                return Ok(hash.finalize());
            }
            hash.update(&buffer[..read]);
        }
    };
    digest().map_err(|err| format!("cannot compare copy {}: {err}", path.display()))
}

fn make_copy_writable(path: &Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|err| format!("cannot inspect copy {}: {err}", path.display()))?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|err| format!("cannot make copy writable {}: {err}", path.display()))
}

fn states(files: &[(PathBuf, PathBuf)]) -> Result<Vec<Option<fs::Metadata>>, String> {
    files
        .iter()
        .map(|(from, _)| match fs::metadata(from) {
            Ok(meta) => Ok(Some(meta)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(format!("cannot inspect {}: {err}", from.display())),
        })
        .collect()
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {err}", path.display()))
        }
        _ => Ok(()),
    }
}

enum Problem {
    /// A required table or column, named `table` or `table.column`.
    Missing(String),
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for Problem {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

/// Which tables have which columns, found by name. The schema version is
/// recorded but never required, so a newer History with the same columns
/// just works.
struct Schema {
    columns: HashMap<&'static str, HashSet<String>>,
}

impl Schema {
    fn probe(conn: &Connection) -> rusqlite::Result<Self> {
        let mut columns = HashMap::new();
        for table in [
            "meta",
            "urls",
            "visits",
            "context_annotations",
            "keyword_search_terms",
        ] {
            let mut statement = conn.prepare("select name from pragma_table_info(?1)")?;
            let names = statement
                .query_map([table], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            columns.insert(table, names);
        }
        Ok(Self { columns })
    }

    fn has(&self, (table, column): (&str, &str)) -> bool {
        self.columns
            .get(table)
            .is_some_and(|names| names.contains(column))
    }

    fn all(&self, columns: &[(&str, &str)]) -> bool {
        columns.iter().all(|&column| self.has(column))
    }

    fn check_required(&self) -> Result<(), Problem> {
        for (table, required) in REQUIRED {
            if self.columns.get(table).is_none_or(HashSet::is_empty) {
                return Err(Problem::Missing(format!("{table} table")));
            }
            if let Some(column) = required.iter().find(|&&column| !self.has((table, column))) {
                return Err(Problem::Missing(format!("{table}.{column} column")));
            }
        }
        Ok(())
    }

    fn unavailable(&self) -> Vec<String> {
        [OPENER, EXTERNAL_REFERRER]
            .into_iter()
            .chain(FOREGROUND)
            .chain(SEARCH_TERMS)
            .filter(|&column| !self.has(column))
            .map(|(table, column)| format!("{table}.{column}"))
            .collect()
    }
}

/// The schema probe, then every URL's signals.
fn query(conn: &Connection, urls: &BTreeSet<&str>) -> Result<Found, Problem> {
    let schema = Schema::probe(conn)?;
    schema.check_required()?;
    let schema_version = if schema.all(&[("meta", "key"), ("meta", "value")]) {
        conn.query_row(
            "select cast(value as integer) from meta where key = 'version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .and_then(|version| u32::try_from(version).ok())
    } else {
        None
    };
    let newest_visit = conn
        .query_row("select max(visit_time) from visits", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?
        .and_then(chrome_time);
    let mut queries = Queries::prepare(conn, &schema)?;
    let mut signals = HashMap::new();
    for &url in urls {
        if let Some(found) = queries.signals(url)? {
            signals.insert(url.to_owned(), found);
        }
    }
    Ok(Found {
        schema_version,
        newest_visit,
        unavailable: schema.unavailable(),
        signals,
    })
}

/// One URL's worth of indexed lookups, prepared once per save. A statement
/// is `None` when the columns it reads are missing.
struct Queries<'c> {
    url_row: rusqlite::Statement<'c>,
    first_visit: rusqlite::Statement<'c>,
    foreground: Option<rusqlite::Statement<'c>>,
    search: Option<rusqlite::Statement<'c>>,
    referrers: rusqlite::Statement<'c>,
    external: Option<rusqlite::Statement<'c>>,
}

impl<'c> Queries<'c> {
    fn prepare(conn: &'c Connection, schema: &Schema) -> rusqlite::Result<Self> {
        let parents = if schema.has(OPENER) {
            "parent.id in (child.from_visit, child.opener_visit)"
        } else {
            "parent.id = child.from_visit"
        };
        Ok(Self {
            url_row: conn.prepare(
                "select id, visit_count, typed_count, last_visit_time from urls
                 where url = ?1 order by last_visit_time desc limit 1",
            )?,
            first_visit: conn.prepare("select min(visit_time) from visits where url = ?1")?,
            // Chrome writes -1,000,000 µs for "unknown", including on every
            // visit still in progress; only positive durations are time in
            // the foreground.
            foreground: schema
                .all(&FOREGROUND)
                .then(|| {
                    conn.prepare(
                        "select sum(c.total_foreground_duration) from visits v
                         join context_annotations c on c.visit_id = v.id
                         where v.url = ?1 and c.total_foreground_duration > 0",
                    )
                })
                .transpose()?,
            // Every visit to the page is hop 0; each step back through the
            // visit that led to one is a hop. The nearest results page wins,
            // and among equally near ones the one visited last.
            search: schema
                .all(&SEARCH_TERMS)
                .then(|| {
                    conn.prepare(&format!(
                        "with recursive ancestors(id, depth) as (
                             select id, 0 from visits where url = ?1
                             union
                             select parent.id, ancestors.depth + 1 from ancestors
                             join visits child on child.id = ancestors.id
                             join visits parent on {parents}
                             where ancestors.depth < ?2
                         )
                         select k.term, ancestors.depth from ancestors
                         join visits v on v.id = ancestors.id
                         join keyword_search_terms k on k.url_id = v.url
                         order by ancestors.depth, v.visit_time desc limit 1"
                    ))
                })
                .transpose()?,
            // The latest visit whose referring visit was to another page;
            // the link it followed wins over the tab that opened it.
            referrers: conn.prepare(&format!(
                "select u.url from visits child
                 join visits parent on {parents}
                 join urls u on u.id = parent.url
                 where child.url = ?1 and parent.url != ?1
                 order by child.visit_time desc, parent.id = child.from_visit desc"
            ))?,
            external: schema
                .has(EXTERNAL_REFERRER)
                .then(|| {
                    conn.prepare(
                        "select external_referrer_url from visits
                         where url = ?1 and external_referrer_url != ''
                         order by visit_time desc",
                    )
                })
                .transpose()?,
        })
    }

    /// `None` when History does not know the URL.
    fn signals(&mut self, url: &str) -> rusqlite::Result<Option<TabHistory>> {
        let database_url = database_url(url);
        let Some((id, visits, typed, last_visit)) = self
            .url_row
            .query_row([database_url.as_ref()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .optional()?
        else {
            return Ok(None);
        };
        // A row without a last visit is one whose visits have all expired:
        // History no longer knows when this page was open.
        let Some(last_visit) = chrome_time(last_visit) else {
            return Ok(None);
        };
        let first_visit = self
            .first_visit
            .query_row([id], |row| row.get::<_, Option<i64>>(0))?
            .and_then(chrome_time);
        let foreground_seconds = match &mut self.foreground {
            Some(statement) => statement
                .query_row([id], |row| row.get::<_, Option<i64>>(0))?
                .and_then(|micros| u64::try_from(micros / 1_000_000).ok()),
            None => None,
        };
        let search = match &mut self.search {
            Some(statement) => statement
                .query_row((id, MAX_HOPS), |row| {
                    Ok(Search {
                        term: row.get(0)?,
                        hops: row.get(1)?,
                    })
                })
                .optional()?,
            None => None,
        };
        let mut referrer = first_elsewhere(&mut self.referrers, id)?;
        if referrer.is_none()
            && let Some(statement) = &mut self.external
        {
            referrer = first_elsewhere(statement, id)?;
        }
        Ok(Some(TabHistory {
            visits: u64::try_from(visits).unwrap_or(0),
            typed: u64::try_from(typed).unwrap_or(0),
            first_visit,
            last_visit,
            foreground_seconds,
            search,
            referrer,
        }))
    }
}

/// Chromium's `database_utils::GurlToDatabaseUrl` removes credentials.
/// SNSS already carries canonical GURLs; preserve everything else verbatim.
fn database_url(raw: &str) -> Cow<'_, str> {
    let Ok(mut url) = url::Url::parse(raw) else {
        return Cow::Borrowed(raw);
    };
    if url.username().is_empty() && url.password().is_none() {
        return Cow::Borrowed(raw);
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Cow::Owned(url.into())
}

/// The first URL a query yields that is not on this machine. A referrer on
/// this machine is treated as if it had never been open, the same as a tab
/// there, so the next one is looked at.
fn first_elsewhere(
    statement: &mut rusqlite::Statement<'_>,
    id: i64,
) -> rusqlite::Result<Option<String>> {
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        if let Some(url) = row.get::<_, Option<String>>(0)?
            && !url.is_empty()
            && !local::is_this_machine_url(&url)
        {
            return Ok(Some(url));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_copy_retries_changes_and_removes_vanished_companions() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("History");
        let journal = source.with_file_name("History-journal");
        fs::write(&source, b"before").unwrap();
        fs::write(&journal, b"journal").unwrap();
        let dest = tempfile::tempdir().unwrap();
        let mut calls = 0;
        let (path, bytes) = copy_stable_with(&source, dest.path(), false, |from, to| {
            let count = fs::copy(from, to)?;
            calls += 1;
            if from == source && calls == 2 {
                fs::write(&source, b"after transaction")?;
                fs::remove_file(&journal)?;
            }
            Ok(count)
        })
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(bytes, 17);
        assert_eq!(fs::read(path).unwrap(), b"after transaction");
        assert!(!dest.path().join("History-journal").exists());
    }

    #[test]
    fn unstable_or_incomplete_copies_are_never_accepted() {
        for missing in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let source = tmp.path().join("History");
            fs::write(&source, b"complete").unwrap();
            let dest = tempfile::tempdir().unwrap();
            let mut calls = 0;
            let result = copy_stable_with(&source, dest.path(), false, |_, to| {
                calls += 1;
                if missing {
                    Err(std::io::Error::from(ErrorKind::NotFound))
                } else {
                    fs::write(to, b"short")?;
                    Ok(5)
                }
            });
            assert!(result.is_err());
            assert_eq!(calls, ATTEMPTS);
        }
    }

    #[test]
    fn verification_rejects_same_size_torn_copies_despite_stable_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("History");
        fs::write(&source, b"good").unwrap();
        let dest = tempfile::tempdir().unwrap();
        let mut calls = 0;
        let (path, _) = copy_stable_with(&source, dest.path(), true, |from, to| {
            calls += 1;
            let count = fs::copy(from, to)?;
            if calls == 1 {
                fs::write(to, b"torn")?;
            }
            Ok(count)
        })
        .unwrap();
        assert_eq!(calls, 4);
        assert_eq!(fs::read(path).unwrap(), b"good");
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 1);
    }
}
