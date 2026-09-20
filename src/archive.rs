//! The archive on disk: root creation, the advisory lock, staged writes,
//! atomic publish, and reading back what was published.
//!
//! slice: capture
//! why: The archive is sacred. A snapshot directory either exists in full or
//!      does not exist, and once renamed into place it is never written to
//!      again. Getting that promise right means one place owns the sequence
//!      "stage beside the destination, fsync, rename once, fsync the parent"
//!      and the lock that keeps two runs from interleaving.

use std::fs::{self, File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::error::Error;
use crate::model::Snapshot;

pub const SNAPSHOTS_DIR: &str = "snapshots";
pub const LOCK_FILE: &str = "lock";
pub const SNAPSHOT_JSON: &str = "snapshot.json";
/// Dot-prefixed so listings skip it; under the lock, any such directory
/// left over from a previous run is stale by definition.
pub const STAGING_PREFIX: &str = ".staging-";

#[derive(Debug)]
pub struct Archive {
    root: PathBuf,
}

/// Held for the duration of a run. Releases when dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

/// A snapshot being assembled. Deleted on drop unless published.
#[derive(Debug)]
pub struct Staging {
    dir: tempfile::TempDir,
}

/// The newest earlier snapshot that matched, plus any that could not be read.
#[derive(Debug, Default)]
pub struct Previous {
    pub snapshot: Option<Snapshot>,
    pub unreadable: Vec<PathBuf>,
}

impl Archive {
    /// A read-only handle; listing an absent archive must not create it.
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Creates the root (mode `0700` on Unix) and `snapshots/` if absent.
    pub fn open(root: &Path) -> Result<Self, Error> {
        create_private_dir(root).map_err(Error::io("create", root))?;
        let snapshots = root.join(SNAPSHOTS_DIR);
        create_private_dir(&snapshots).map_err(Error::io("create", &snapshots))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.root.join(SNAPSHOTS_DIR)
    }

    /// Takes the exclusive lock, calling `on_wait` once if another run holds
    /// it, then blocking until it is free.
    pub fn lock(&self, on_wait: impl FnOnce()) -> Result<Lock, Error> {
        let path = self.root.join(LOCK_FILE);
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(Error::io("open", &path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                on_wait();
                file.lock().map_err(Error::io("lock", &path))?;
            }
            Err(TryLockError::Error(source)) => {
                return Err(Error::io("lock", &path)(source));
            }
        }
        Ok(Lock { _file: file })
    }

    /// Removes staging directories left by interrupted runs. Call with the
    /// lock held. Returns how many were removed.
    pub fn clean_stale_staging(&self) -> Result<usize, Error> {
        let dir = self.snapshots_dir();
        let mut removed = 0;
        for entry in fs::read_dir(&dir).map_err(Error::io("list", &dir))? {
            let entry = entry.map_err(Error::io("list", &dir))?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(STAGING_PREFIX) {
                fs::remove_dir_all(entry.path()).map_err(Error::io("remove", entry.path()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Published snapshot ids, oldest first. Hidden entries are not snapshots.
    pub fn snapshot_ids(&self) -> Result<Vec<String>, Error> {
        let dir = self.snapshots_dir();
        let mut ids = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(err) => return Err(Error::io("list", &dir)(err)),
        };
        for entry in entries {
            let entry = entry.map_err(Error::io("list", &dir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            ids.push(name);
        }
        ids.sort_by_cached_key(|id| {
            let (base, ordinal) = id.split_once("Z-").map_or_else(
                || (id.clone(), 1),
                |(base, suffix)| (format!("{base}Z"), suffix.parse::<u32>().unwrap_or(0)),
            );
            (base, ordinal)
        });
        Ok(ids)
    }

    /// The newest snapshot whose source has the same browser and profile.
    pub fn latest_matching(
        &self,
        browser: Option<&str>,
        profile: Option<&str>,
    ) -> Result<Previous, Error> {
        let mut previous = Previous::default();
        for id in self.snapshot_ids()?.into_iter().rev() {
            let path = self.snapshots_dir().join(&id).join(SNAPSHOT_JSON);
            let Ok(snapshot) = read_snapshot(&path) else {
                previous.unreadable.push(path);
                continue;
            };
            if snapshot.source.browser.as_deref() == browser
                && snapshot.source.profile.as_deref() == profile
            {
                previous.snapshot = Some(snapshot);
                break;
            }
        }
        Ok(previous)
    }

    /// The id for a snapshot captured at `at`: the UTC second, then `-2`,
    /// `-3` and so on when that second already has one.
    pub fn allocate_id(&self, at: Timestamp) -> Result<String, Error> {
        let base = format_id(at);
        let dir = self.snapshots_dir();
        if !dir.join(&base).exists() {
            return Ok(base);
        }
        (2..u32::MAX)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !dir.join(id).exists())
            .ok_or_else(|| Error::io("allocate", &dir)(std::io::Error::other("no free id")))
    }

    /// A fresh staging directory beside the snapshots, on the same volume.
    pub fn stage(&self) -> Result<Staging, Error> {
        let dir = self.snapshots_dir();
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&dir)
            .map_err(Error::io("create staging directory in", &dir))?;
        Ok(Staging { dir: staging })
    }

    /// One rename, then the parent directory is synced so the rename itself
    /// is durable. Never renames over an existing path.
    pub fn publish(&self, staging: Staging, id: &str) -> Result<PathBuf, Error> {
        let destination = self.snapshots_dir().join(id);
        if destination.exists() {
            return Err(Error::io("publish", &destination)(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "snapshot already exists",
            )));
        }
        sync_dir(staging.dir.path())?;
        fs::rename(staging.dir.path(), &destination)
            .map_err(Error::io("rename into", &destination))?;
        // The rename succeeded, so the temp path no longer exists; forget it
        // rather than let the drop try to remove something else.
        let _ = staging.dir.keep();
        sync_dir(&self.snapshots_dir())?;
        Ok(destination)
    }
}

impl Staging {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes and fsyncs one file inside the staging directory.
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let path = self.dir.path().join(name);
        let mut file = File::create(&path).map_err(Error::io("create", &path))?;
        file.write_all(bytes).map_err(Error::io("write", &path))?;
        file.sync_all().map_err(Error::io("sync", &path))?;
        Ok(())
    }
}

/// `2026-09-20-084415Z`: UTC, second resolution, sorts as text, contains no
/// character any filesystem rejects.
pub fn format_id(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d-%H%M%SZ").to_string()
}

pub fn read_snapshot(path: &Path) -> Result<Snapshot, Error> {
    let bytes = fs::read(path).map_err(Error::io("read", path))?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Replaces one file in a single rename: staged beside it on the same
/// volume, fsynced, renamed over the old content, then the parent synced.
/// This is the snapshot publish sequence for a file that is allowed to
/// change, which is what user state is. Caller holds the archive lock.
pub fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempfile_in(parent)
        .map_err(Error::io("stage beside", path))?;
    staged
        .write_all(bytes)
        .map_err(Error::io("write staged", path))?;
    staged
        .as_file()
        .sync_all()
        .map_err(Error::io("sync staged", path))?;
    staged
        .persist(path)
        .map_err(|err| Error::io("replace", path)(err.error))?;
    sync_dir(parent)
}

pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Directory fsync is what makes a rename survive a crash on Unix. Windows
/// has no equivalent and does not need one for this purpose.
pub fn sync_dir(dir: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(Error::io("sync", dir))?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn id_format_is_utc_and_sortable() {
        let at: Timestamp = "2026-09-20T08:44:15.999Z".parse().unwrap();
        assert_eq!(format_id(at), "2026-09-20-084415Z");
        let later: Timestamp = "2026-09-20T08:44:16Z".parse().unwrap();
        assert!(format_id(at) < format_id(later));
        assert!(format_id(at) < format!("{}-2", format_id(at)));
    }

    #[test]
    fn open_creates_root_privately_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("nested").join("root");
        let archive = Archive::open(&root).unwrap();
        assert!(archive.snapshots_dir().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        Archive::open(&root).unwrap();
        assert!(archive.snapshot_ids().unwrap().is_empty());
    }

    #[test]
    fn allocate_id_suffixes_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let at: Timestamp = "2026-09-20T08:44:15Z".parse().unwrap();
        assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z");
        fs::create_dir(archive.snapshots_dir().join("2026-09-20-084415Z")).unwrap();
        assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z-2");
        fs::create_dir(archive.snapshots_dir().join("2026-09-20-084415Z-2")).unwrap();
        assert_eq!(archive.allocate_id(at).unwrap(), "2026-09-20-084415Z-3");
    }

    #[test]
    fn staging_is_hidden_until_published_and_cleaned_if_abandoned() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let staging = archive.stage().unwrap();
        staging.write("a.txt", b"hello").unwrap();
        assert!(staging.path().starts_with(archive.snapshots_dir()));
        assert!(archive.snapshot_ids().unwrap().is_empty());

        let abandoned = archive.stage().unwrap();
        let abandoned_path = abandoned.dir.keep();
        assert!(abandoned_path.is_dir());
        assert_eq!(archive.clean_stale_staging().unwrap(), 2);
        assert!(!abandoned_path.is_dir());
        assert!(!staging.path().is_dir());
        drop(staging);
    }

    #[test]
    fn publish_renames_once_and_refuses_to_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let staging = archive.stage().unwrap();
        staging.write("a.txt", b"hello").unwrap();
        #[cfg(unix)]
        let staging_inode = std::fs::metadata(staging.path()).unwrap().ino();
        let published = archive.publish(staging, "2026-01-01-000000Z").unwrap();
        assert_eq!(fs::read(published.join("a.txt")).unwrap(), b"hello");
        #[cfg(unix)]
        assert_eq!(std::fs::metadata(&published).unwrap().ino(), staging_inode);
        assert_eq!(archive.snapshot_ids().unwrap(), vec!["2026-01-01-000000Z"]);
        assert_eq!(archive.clean_stale_staging().unwrap(), 0);

        let again = archive.stage().unwrap();
        again.write("a.txt", b"other").unwrap();
        let err = archive.publish(again, "2026-01-01-000000Z").unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert_eq!(fs::read(published.join("a.txt")).unwrap(), b"hello");
        assert_eq!(
            archive.clean_stale_staging().unwrap(),
            0,
            "failed publish cleans itself"
        );
    }

    #[test]
    fn replace_file_swaps_content_in_one_rename_and_leaves_no_stage_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        replace_file(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        replace_file(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // The destination's parent must exist; nothing is created above it.
        assert!(replace_file(&tmp.path().join("missing/state.json"), b"x").is_err());
    }

    #[test]
    fn lock_is_exclusive_across_handles() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let held = archive.lock(|| panic!("should not wait")).unwrap();
        let other = File::open(tmp.path().join(LOCK_FILE)).unwrap();
        assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
        drop(held);
        assert!(other.try_lock().is_ok());
    }

    #[test]
    fn latest_matching_skips_unreadable_and_other_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let make = |id: &str, body: &str| {
            let dir = archive.snapshots_dir().join(id);
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join(SNAPSHOT_JSON), body).unwrap();
        };
        let snapshot = |id: &str, profile: &str| {
            format!(
                r#"{{"schema_version":1,"id":"{id}","captured_at":"2026-01-01T00:00:00Z",
                "source":{{"browser":"chrome","profile":"{profile}","profile_display":null,"path":"/x",
                "file":"session.snss","sha256":"","bytes":0,"saved_at":null,"session_started_at":null}},
                "stats":{{"file_version":3,"command_table":"session","commands":0,"commands_by_id":{{}},
                "unknown_commands":0,"unknown_command_ids":[],"malformed_commands":0,"truncated_bytes":0,
                "marker_count":1,"marker_ok":true,"windows":0,"tabs":0,"groups":0,"dropped_tabs":0,
                "dropped_tab_reasons":{{"no_navigations":0,"window_missing":0,"window_closed":0}},
                "navigation_fallbacks":0,"groups_without_metadata":0}},"windows":[],"groups":[],"tabs":[]}}"#
            )
        };
        make(
            "2026-01-01-000001Z",
            &snapshot("2026-01-01-000001Z", "Default"),
        );
        make(
            "2026-01-01-000002Z",
            &snapshot("2026-01-01-000002Z", "Profile 1"),
        );
        make("2026-01-01-000003Z", "{not json");
        fs::create_dir(archive.snapshots_dir().join(".staging-x")).unwrap();

        let found = archive
            .latest_matching(Some("chrome"), Some("Default"))
            .unwrap();
        assert_eq!(found.snapshot.unwrap().id, "2026-01-01-000001Z");
        assert_eq!(found.unreadable.len(), 1);
        let none = archive.latest_matching(None, None).unwrap();
        assert!(none.snapshot.is_none());

        // Lexicographic ordering puts -9 after -10, which would compare the
        // new layout against the wrong snapshot after a busy capture burst.
        for suffix in [9, 10] {
            let id = format!("2026-01-01-000004Z-{suffix}");
            make(&id, &snapshot(&id, "Default"));
        }
        let latest = archive
            .latest_matching(Some("chrome"), Some("Default"))
            .unwrap();
        assert_eq!(latest.snapshot.unwrap().id, "2026-01-01-000004Z-10");
    }
}
