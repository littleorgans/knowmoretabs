//! The encrypted-sessions preflight: refuses to snapshot cleartext files that
//! Chrome has visibly stopped keeping current.
//!
//! slice: capture
//! why: Chrome is mid-migration to encrypted session storage. Today it writes
//!      both `Sessions/` and `Sessions_Encrypted/`; at the last stage it stops
//!      writing the cleartext copy and deletes it. The failure that follows is
//!      not a crash but silently stale data presented as current, which is
//!      worse. This module compares modification times only, opens no file,
//!      and cannot be bypassed by `--force`, because a forced stale snapshot
//!      is exactly the corrupt archive the check exists to prevent.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::platform::{ENCRYPTED_SESSIONS_DIR, SESSIONS_DIR};

/// Chrome writes the two backends seconds apart and filesystems round mtimes;
/// five minutes filters that and still catches a migration.
pub const THRESHOLD: Duration = Duration::from_secs(300);
/// The file families Chrome writes to both directories.
pub const PREFIXES: [&str; 3] = ["Session_", "Tabs_", "Apps_"];

/// Shown verbatim when the check trips. The research document's version
/// offered a fallback that does not exist; this one points at the tracker.
pub const MESSAGE: &str = "Chrome's encrypted session files are newer than its cleartext session files. \
knowmoretabs cannot read the encrypted files, so this capture would be stale and no snapshot was saved. \
Update knowmoretabs when encrypted-session support is available; progress is tracked at \
https://github.com/littleorgans/knowmoretabs/issues.";

/// Why a profile was judged stale. Each variant is one of the three rules in
/// `docs/research/encrypted-sessions.md` §6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    /// `Sessions_Encrypted/` has files and `Sessions/` has none.
    NoCleartext,
    /// A family exists encrypted but not in cleartext.
    MissingCleartext { prefix: &'static str },
    /// The encrypted file is newer by at least [`THRESHOLD`].
    EncryptedNewer { prefix: &'static str, gap: Duration },
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCleartext => write!(f, "Sessions_Encrypted/ has files and Sessions/ has none"),
            Self::MissingCleartext { prefix } => {
                write!(
                    f,
                    "{prefix}* exists in Sessions_Encrypted/ but not in Sessions/"
                )
            }
            Self::EncryptedNewer { prefix, gap } => write!(
                f,
                "{prefix}* in Sessions_Encrypted/ is {}s newer than in Sessions/",
                gap.as_secs()
            ),
        }
    }
}

/// Runs the preflight for one profile directory. `Ok(None)` means proceed.
/// Only directory listings and `stat` are used.
pub fn check(profile_dir: &Path) -> std::io::Result<Option<StaleReason>> {
    let encrypted_dir = profile_dir.join(ENCRYPTED_SESSIONS_DIR);
    if !encrypted_dir.is_dir() {
        return Ok(None);
    }
    let encrypted = newest_by_prefix(&encrypted_dir)?;
    let clear = newest_by_prefix(&profile_dir.join(SESSIONS_DIR))?;
    Ok(compare(&clear, &encrypted))
}

/// The three rules, on already-collected mtimes.
pub fn compare(
    clear: &BTreeMap<&'static str, SystemTime>,
    encrypted: &BTreeMap<&'static str, SystemTime>,
) -> Option<StaleReason> {
    if !encrypted.is_empty() && clear.is_empty() {
        return Some(StaleReason::NoCleartext);
    }
    for (&prefix, &encrypted_at) in encrypted {
        let Some(&clear_at) = clear.get(prefix) else {
            return Some(StaleReason::MissingCleartext { prefix });
        };
        if let Ok(gap) = encrypted_at.duration_since(clear_at)
            && gap >= THRESHOLD
        {
            return Some(StaleReason::EncryptedNewer { prefix, gap });
        }
    }
    None
}

/// Newest modification time per prefix among the directory's immediate
/// regular files. A missing directory has no files.
fn newest_by_prefix(dir: &Path) -> std::io::Result<BTreeMap<&'static str, SystemTime>> {
    let mut newest = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(newest),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(&prefix) = PREFIXES.iter().find(|p| name.starts_with(*p)) else {
            continue;
        };
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified()?;
        newest
            .entry(prefix)
            .and_modify(|t: &mut SystemTime| *t = (*t).max(modified))
            .or_insert(modified);
    }
    Ok(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn map(pairs: &[(&'static str, u64)]) -> BTreeMap<&'static str, SystemTime> {
        pairs.iter().map(|&(p, s)| (p, at(s))).collect()
    }

    #[test]
    fn no_encrypted_files_is_fresh() {
        assert_eq!(compare(&map(&[("Session_", 100)]), &map(&[])), None);
        assert_eq!(compare(&map(&[]), &map(&[])), None);
    }

    #[test]
    fn encrypted_without_any_cleartext_is_stale() {
        assert_eq!(
            compare(&map(&[]), &map(&[("Session_", 100)])),
            Some(StaleReason::NoCleartext)
        );
    }

    #[test]
    fn missing_cleartext_family_is_stale() {
        assert_eq!(
            compare(
                &map(&[("Session_", 100)]),
                &map(&[("Session_", 100), ("Tabs_", 100)])
            ),
            Some(StaleReason::MissingCleartext { prefix: "Tabs_" })
        );
    }

    #[test]
    fn threshold_is_exactly_five_minutes() {
        let clear = map(&[("Session_", 1000)]);
        assert_eq!(compare(&clear, &map(&[("Session_", 1299)])), None);
        assert_eq!(compare(&clear, &map(&[("Session_", 900)])), None);
        assert_eq!(
            compare(&clear, &map(&[("Session_", 1300)])),
            Some(StaleReason::EncryptedNewer {
                prefix: "Session_",
                gap: Duration::from_secs(300)
            })
        );
    }

    fn touch(path: &Path, secs: u64) {
        std::fs::write(path, b"SNSS").unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at(secs))
            .unwrap();
    }

    #[test]
    fn check_reads_only_immediate_regular_files_with_known_prefixes() {
        let profile = tempfile::tempdir().unwrap();
        let clear = profile.path().join(SESSIONS_DIR);
        let enc = profile.path().join(ENCRYPTED_SESSIONS_DIR);
        std::fs::create_dir_all(&clear).unwrap();
        assert_eq!(check(profile.path()).unwrap(), None, "no encrypted dir");

        std::fs::create_dir_all(&enc).unwrap();
        touch(&clear.join("Session_1"), 10_000);
        touch(&clear.join("Session_2"), 10_100);
        touch(&enc.join("Session_3"), 10_200);
        touch(&enc.join("unrelated"), 99_999);
        std::fs::create_dir(enc.join("Tabs_dir")).unwrap();
        assert_eq!(check(profile.path()).unwrap(), None, "100s gap is fine");

        touch(&enc.join("Session_4"), 10_400);
        assert_eq!(
            check(profile.path()).unwrap(),
            Some(StaleReason::EncryptedNewer {
                prefix: "Session_",
                gap: Duration::from_secs(300)
            })
        );
    }

    #[test]
    fn check_with_cleartext_dir_removed_is_stale() {
        let profile = tempfile::tempdir().unwrap();
        let enc = profile.path().join(ENCRYPTED_SESSIONS_DIR);
        std::fs::create_dir_all(&enc).unwrap();
        touch(&enc.join("Tabs_1"), 5);
        assert_eq!(
            check(profile.path()).unwrap(),
            Some(StaleReason::NoCleartext)
        );
    }

    #[test]
    fn message_has_no_fallback_promise() {
        assert!(!MESSAGE.contains("fallback"));
        assert!(!MESSAGE.contains("DevTools"));
        assert!(MESSAGE.contains("no snapshot was saved"));
    }
}
