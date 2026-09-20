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
    ///
    /// `File::lock` is `flock` on Unix and `LockFileEx` on Windows. The two
    /// differ in a way that does not matter here and one that is worth
    /// knowing: `flock` is advisory, so a process that never asks for the
    /// lock is unaffected by it, while `LockFileEx` is mandatory and also
    /// fails other processes' reads of the locked range. Both are per open
    /// handle and both release when it closes, including when the process
    /// dies, which is the whole of what this is used for: two knowmoretabs
    /// runs never interleave, and a killed run does not leave the archive
    /// locked. Windows is, if anything, the stronger of the two.
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
    /// is durable.
    ///
    /// **Never renames over an existing path, on any platform, by design.**
    /// Renaming a directory onto an existing one is the operation whose
    /// semantics differ: POSIX replaces an empty destination and fails
    /// `ENOTEMPTY` otherwise, while Windows' `MoveFileExW` cannot replace a
    /// directory at all and the standard library's `FILE_RENAME_POSIX_
    /// SEMANTICS` fallback only reaches the empty case. Publication does not
    /// depend on any of that: the destination must not exist, which is
    /// checked under the archive lock, and what rename has to do is create a
    /// name that is not there — which is a single atomic metadata operation
    /// everywhere. An interrupted run therefore leaves the archive exactly as
    /// it was on Linux, macOS and Windows alike.
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
///
/// Unlike [`Archive::publish`] this one does rename onto an existing name,
/// and it is a **file**, which is the case where replacement is atomic on
/// every platform we ship. `tempfile`'s `persist` is `rename` on Unix and
/// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows, and that flag
/// is what makes the Windows call replace rather than fail with
/// `ERROR_ALREADY_EXISTS`; bare `rename(3)` and `MoveFileW`, the two calls
/// that would fail, are not what is being used here. Either the old bytes or
/// the new ones are at `path` at every instant, never neither and never a
/// mixture. A reader holding the file open cannot block it either: the
/// standard library opens with `FILE_SHARE_DELETE`.
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

/// The archive is a record of everything the user browses, so it is created
/// private to them.
///
/// On Unix that is `0700`, set here. Windows has no mode bit and setting a
/// DACL needs the Win32 security APIs, which this crate cannot reach —
/// `unsafe_code` is forbidden and a `windows-sys` dependency for one call is
/// not worth it. What it gets instead is inheritance: a directory created
/// under `%LOCALAPPDATA%` inherits that folder's ACL, which grants the user,
/// SYSTEM and Administrators and no one else, so the default root from
/// [`crate::platform::default_root`] is private without our help. **A
/// `--root` chosen outside the user profile is not**: it inherits whatever
/// its parent grants, and `C:\` grants `Users` read access. That difference
/// is real and is in the README rather than papered over.
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
/// has no equivalent: a directory handle cannot be flushed, and it does not
/// need to be, because NTFS logs the metadata change that a rename is and
/// replays it on recovery. The file contents are already durable either way,
/// since [`Staging::write`] syncs each file before the rename.
// The `Result` is what every caller handles and what Unix genuinely returns.
// Collapsing it where the body happens to be empty would put a `cfg` in each
// of the four call sites to say the same thing this one comment says.
#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
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

    /// The privacy guarantee is not the same sentence on both platforms, so
    /// each one asserts its own. Unix: the mode is `0700`, set by us.
    /// Windows: there is no mode, and we cannot set an ACL without the Win32
    /// security APIs, so the guarantee is that creating the root grants
    /// nobody anything its parent did not already grant — which is what
    /// makes the default root under `%LOCALAPPDATA%` private and what makes
    /// a `--root` elsewhere only as private as where it was put.
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
        #[cfg(windows)]
        {
            // `(I)` is icacls' flag for an inherited entry, and it is a flag
            // letter rather than prose, so this reads the same on a
            // non-English Windows. Every entry being inherited is the claim:
            // creating the archive granted nobody anything, so the root is
            // exactly as private as the directory it was created in, and
            // `default_root` puts that directory under `%LOCALAPPDATA%`.
            let out = std::process::Command::new("icacls")
                .arg(&root)
                .output()
                .expect("icacls");
            assert!(out.status.success(), "icacls failed");
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let entries: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
            assert!(!entries.is_empty(), "icacls listed no entries: {text}");
            assert!(
                entries.iter().all(|line| line.contains("(I)")),
                "the archive root has an entry of its own rather than inheriting: {text}"
            );
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
        // "Moved, not copied" is the same claim on both platforms, but only
        // Unix can be asked it on stable: `MetadataExt::file_index` is the
        // Windows equivalent of an inode and is still unstable there. The
        // assertions around this one hold everywhere.
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

    /// The durability promise in the one shape where `rename` does not mean
    /// the same thing on POSIX and Windows. POSIX `rename` replaces an empty
    /// destination directory and takes the whole snapshot with it; Windows
    /// refuses. Publication depends on neither, because it refuses first —
    /// so this asserts the same outcome on all three platforms, against an
    /// empty directory, a full one, and a file wearing a snapshot's name.
    /// What is already sitting at the snapshot's name.
    enum Occupied {
        EmptyDirectory,
        FullDirectory,
        File,
    }

    #[test]
    fn publish_never_renames_over_anything_that_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = Archive::open(tmp.path()).unwrap();
        let occupied = [
            ("2026-01-01-000001Z", Occupied::EmptyDirectory),
            ("2026-01-01-000002Z", Occupied::FullDirectory),
            ("2026-01-01-000003Z", Occupied::File),
        ];
        for (id, what) in occupied {
            let destination = archive.snapshots_dir().join(id);
            match what {
                Occupied::EmptyDirectory => fs::create_dir(&destination).unwrap(),
                Occupied::FullDirectory => {
                    fs::create_dir(&destination).unwrap();
                    fs::write(destination.join("keep.txt"), b"mine").unwrap();
                }
                Occupied::File => fs::write(&destination, b"not a directory").unwrap(),
            }
            let before = fs::symlink_metadata(&destination).unwrap();
            let staging = archive.stage().unwrap();
            staging.write("snapshot.json", b"the new snapshot").unwrap();

            let err = archive.publish(staging, id).unwrap_err();
            assert!(err.to_string().contains("already exists"), "{id}: {err}");
            let after = fs::symlink_metadata(&destination).unwrap();
            assert_eq!(
                before.file_type().is_dir(),
                after.file_type().is_dir(),
                "{id}"
            );
            assert!(
                !destination.join("snapshot.json").exists(),
                "{id}: the staged snapshot reached the destination"
            );
            assert_eq!(archive.clean_stale_staging().unwrap(), 0, "{id}");
        }
        assert_eq!(
            fs::read(
                archive
                    .snapshots_dir()
                    .join("2026-01-01-000002Z")
                    .join("keep.txt")
            )
            .unwrap(),
            b"mine"
        );
        assert_eq!(
            fs::read(archive.snapshots_dir().join("2026-01-01-000003Z")).unwrap(),
            b"not a directory"
        );
    }

    /// The other half of the rename answer: here the destination does exist
    /// and has to be replaced. `MOVEFILE_REPLACE_EXISTING` is what makes that
    /// work on Windows, and a reader holding the file open must not be able
    /// to stop it — so the replacement happens with the old content open.
    #[test]
    fn replace_file_swaps_content_in_one_rename_and_leaves_no_stage_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        replace_file(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");

        let reader = File::open(&path).unwrap();
        replace_file(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        drop(reader);

        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // A replacement that cannot complete leaves the old bytes in place:
        // a directory in the way is the cheapest way to make rename refuse
        // on every platform.
        let blocked = tmp.path().join("blocked");
        replace_file(&blocked, b"original").unwrap();
        fs::remove_file(&blocked).unwrap();
        fs::create_dir(&blocked).unwrap();
        assert!(replace_file(&blocked, b"replacement").is_err());
        assert!(blocked.is_dir());

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
