//! The archive on disk: root creation, the advisory lock, staged writes,
//! atomic publish, and reading back what was published.
//!
//! slice: capture, triage, platforms
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
        let mut builder = tempfile::Builder::new();
        builder.prefix(STAGING_PREFIX);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let staging = builder
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
/// and it is a **file**, which is the case where replacement is atomic
/// everywhere. Either the old bytes or the new ones are at `path` at every
/// instant, never neither and never a mixture.
///
/// It goes through `fs::rename` rather than `tempfile`'s `persist`, and the
/// difference is not cosmetic. Both issue `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING` on Windows, which is what makes the call
/// replace rather than fail with `ERROR_ALREADY_EXISTS`. But that path goes
/// through the classic `FileRenameInformation`, and replacing a destination
/// that **any** process currently has open fails there with
/// `ERROR_ACCESS_DENIED` — sharing flags do not help, because the target is
/// being deleted, not shared. POSIX `rename` has no such rule. Only
/// `fs::rename` retries with `FileRenameInfoEx` and
/// `FILE_RENAME_FLAG_POSIX_SEMANTICS`, the flag that exists precisely to
/// unlink an open target, which gives Windows the same behaviour as the
/// other two. `persist` has no retry, so a virus scanner, a backup agent or
/// an editor holding `library.json` open for a moment was enough to fail a
/// `forget`. The staged file's own handle is closed first so that the retry
/// can open the source for `DELETE`.
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
    // `keep` also clears the temporary attribute Windows marks the file
    // with, which the published file must not carry.
    let (file, staged_path) = staged
        .keep()
        .map_err(|err| Error::io("stage beside", path)(err.error))?;
    drop(file);
    if let Err(source) = fs::rename(&staged_path, path) {
        // `keep` took cleanup away from the destructor; nothing else will
        // remove the stage now, and a failed write must leave no litter.
        let _ = fs::remove_file(&staged_path);
        return Err(Error::io("replace", path)(source));
    }
    sync_dir(parent)
}

/// Removes what interrupted runs left staged in `dir`: snapshot staging
/// directories and files staged by [`replace_file`] alike. Call with the
/// lock held. Returns how many were removed.
pub fn clean_stale_staging(dir: &Path) -> Result<usize, Error> {
    let mut removed = 0;
    for entry in fs::read_dir(dir).map_err(Error::io("list", dir))? {
        let entry = entry.map_err(Error::io("list", dir))?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map_err(Error::io("list", dir))?.is_dir();
        if is_dir {
            fs::remove_dir_all(&path).map_err(Error::io("remove", &path))?;
        } else {
            fs::remove_file(&path).map_err(Error::io("remove", &path))?;
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
std::thread_local! {
    /// An independent handle used to check the real lock at each directory
    /// creation in the observing test's thread. Absent in production.
    pub static DIRECTORY_CREATION_LOCK_PROBE: std::cell::RefCell<Option<File>> = const {
        std::cell::RefCell::new(None)
    };
}

/// The archive is a record of everything the user browses, so it is created
/// private to them.
///
/// On Unix that is `0700`, set here. Windows has no mode bit and setting a
/// DACL needs the Win32 security APIs, which this crate cannot reach —
/// `unsafe_code` is forbidden and a `windows-sys` dependency for one call is
/// not worth it. What it gets instead is inheritance: a directory created
/// under `%LOCALAPPDATA%` inherits that folder's ACL. On a standard Windows
/// installation that is the per-user protection intended for this location,
/// so the default root from [`crate::platform::default_root`] is private
/// without our help. **A
/// `--root` chosen outside the user profile is not**: it inherits whatever
/// its parent grants, and a root directory may grant ordinary users read
/// access. That difference is real and is in the README rather than papered
/// over.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    #[cfg(test)]
    DIRECTORY_CREATION_LOCK_PROBE.with(|probe| {
        if let Some(file) = probe.borrow().as_ref() {
            assert!(
                matches!(file.try_lock(), Err(TryLockError::WouldBlock)),
                "directory creation must hold the archive lock: {}",
                path.display()
            );
        }
    });
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Repairs an existing store entry's Unix permissions without replacing
/// it: directories are `0700`, files `0600`. Caller holds the archive lock.
/// Windows entries retain the ACL inherited from their parent.
// Windows has no Unix permissions to repair; callers share one fallible API.
#[allow(clippy::unnecessary_wraps)]
pub fn make_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)?;
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        if metadata.permissions().mode() & 0o7777 != mode {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Directory fsync is what makes a rename survive a crash on Unix. Windows
/// has no directory-flush operation exposed by the standard library, so this
/// is the strongest portable sequence available there: each staged file is
/// flushed before the rename and NTFS journals the metadata change. Journaling
/// is recovery support, not a promise that a sudden power loss has persisted
/// the rename, so callers must not describe the Windows step as equivalent to
/// a Unix directory fsync.
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
#[path = "archive_tests.rs"]
mod tests;
