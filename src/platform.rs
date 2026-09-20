//! Where Chrome keeps its files: the user-data directory, profile discovery
//! through `Local State`, and picking the newest `Session_*` log.
//!
//! slice: capture
//! why: The reference implementation hardcoded one macOS path and trusted a
//!      profile name straight onto the filesystem. This module is the one
//!      place that knowledge lives, shaped as a browser table with a single
//!      row today so that slice 4 adds browsers and slice 5 adds platforms
//!      by adding rows, not code paths. It never opens a session file: it
//!      reads `Local State`, lists directories, and returns paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Deserialize;

/// The `--browser` id of the only browser this slice knows.
pub const CHROME: &str = "chrome";
/// Chrome's own name for the session directory (`kSessionsDirectory`).
pub const SESSIONS_DIR: &str = "Sessions";
/// Where it writes the version-5 files it will one day prefer.
pub const ENCRYPTED_SESSIONS_DIR: &str = "Sessions_Encrypted";
/// The archive directory under the home directory.
pub const DEFAULT_ROOT_NAME: &str = ".knowmoretabs";

/// One row of the browser table: the id and where its user data lives,
/// relative to the home directory (or `%LOCALAPPDATA%` on Windows).
#[derive(Debug, Clone, Copy)]
pub struct BrowserSpec {
    pub id: &'static str,
    pub macos: &'static str,
    pub linux: &'static str,
    pub windows: &'static str,
}

pub const BROWSERS: [BrowserSpec; 1] = [BrowserSpec {
    id: CHROME,
    macos: "Library/Application Support/Google/Chrome",
    linux: ".config/google-chrome",
    windows: "AppData/Local/Google/Chrome/User Data",
}];

impl BrowserSpec {
    /// The user-data directory on the platform this binary was built for.
    pub fn user_data_dir(&self, home: &Path) -> PathBuf {
        let relative = if cfg!(target_os = "macos") {
            self.macos
        } else if cfg!(windows) {
            self.windows
        } else {
            self.linux
        };
        home.join(relative)
    }
}

pub fn browser(id: &str) -> Option<&'static BrowserSpec> {
    BROWSERS.iter().find(|b| b.id == id)
}

pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("cannot read {path}: {source}")]
    LocalState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    LocalStateJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(
        "profile name {0:?} is not a Chrome profile directory name; expected something like \"Default\" or \"Profile 1\""
    )]
    InvalidName(String),
    #[error("profile {0:?} is not a browsing profile (Chrome keeps no session for it)")]
    NotBrowsing(String),
    #[error("profile name {name:?} is used by {dirs}; pass the directory name instead")]
    Ambiguous { name: String, dirs: String },
    #[error("no profile named {name:?} under {user_data}; known profiles: {known}")]
    NotFound {
        name: String,
        user_data: PathBuf,
        known: String,
    },
    #[error("no browsing profile with a session under {0}")]
    NoDefault(PathBuf),
}

/// A resolved profile: the directory Chrome uses and the name people see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub dir_name: String,
    pub display: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct LocalState {
    #[serde(default)]
    profile: ProfilePrefs,
}

#[derive(Debug, Default, Deserialize)]
struct ProfilePrefs {
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default)]
    last_active_profiles: Vec<String>,
    #[serde(default)]
    info_cache: BTreeMap<String, ProfileEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_ephemeral: bool,
}

/// Resolves `--profile` (a directory name or a display name), or picks the
/// profile Chrome itself used last. Every candidate goes through
/// [`guard_dir_name`] before it is joined onto a path.
pub fn resolve_profile(user_data: &Path, requested: Option<&str>) -> Result<Profile, ProfileError> {
    let state = read_local_state(user_data)?;
    let cache = &state.profile.info_cache;
    let dir_name = match requested {
        Some(name) => resolve_requested(user_data, cache, name)?,
        None => resolve_default(user_data, &state.profile)?,
    };
    guard_dir_name(&dir_name, cache)?;
    Ok(Profile {
        display: cache.get(&dir_name).and_then(|e| e.name.clone()),
        path: user_data.join(&dir_name),
        dir_name,
    })
}

fn read_local_state(user_data: &Path) -> Result<LocalState, ProfileError> {
    let path = user_data.join("Local State");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|source| ProfileError::LocalStateJson { path, source }),
        // A user-data dir without Local State is unusual but `--profile
        // Default` should still work against it.
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(LocalState::default()),
        Err(source) => Err(ProfileError::LocalState { path, source }),
    }
}

fn resolve_requested(
    user_data: &Path,
    cache: &BTreeMap<String, ProfileEntry>,
    name: &str,
) -> Result<String, ProfileError> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(ProfileError::InvalidName(name.to_owned()));
    }
    if cache.contains_key(name) || is_chromium_dir_name(name) {
        return Ok(name.to_owned());
    }
    let exact: Vec<&String> = cache
        .iter()
        .filter(|(_, e)| e.name.as_deref() == Some(name))
        .map(|(dir, _)| dir)
        .collect();
    match exact.as_slice() {
        [dir] => return Ok((*dir).clone()),
        [] => {}
        many => {
            return Err(ProfileError::Ambiguous {
                name: name.to_owned(),
                dirs: many
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
                    .join(" and "),
            });
        }
    }
    let folded: Vec<&String> = cache
        .iter()
        .filter(|(dir, e)| {
            dir.eq_ignore_ascii_case(name)
                || e.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .map(|(dir, _)| dir)
        .collect();
    if let [dir] = folded.as_slice() {
        return Ok((*dir).clone());
    }
    Err(ProfileError::NotFound {
        name: name.to_owned(),
        user_data: user_data.to_path_buf(),
        known: describe_profiles(cache),
    })
}

fn resolve_default(user_data: &Path, prefs: &ProfilePrefs) -> Result<String, ProfileError> {
    let usable =
        |dir: &str| guard_dir_name(dir, &prefs.info_cache).is_ok() && user_data.join(dir).is_dir();
    let candidates = prefs
        .last_used
        .iter()
        .chain(prefs.last_active_profiles.first())
        .map(String::as_str)
        .chain(std::iter::once("Default"));
    for dir in candidates {
        if usable(dir) {
            return Ok(dir.to_owned());
        }
    }
    prefs
        .info_cache
        .keys()
        .find(|dir| {
            usable(dir)
                && session_candidates(&user_data.join(dir).join(SESSIONS_DIR))
                    .is_ok_and(|c| !c.is_empty())
        })
        .cloned()
        .ok_or_else(|| ProfileError::NoDefault(user_data.to_path_buf()))
}

fn describe_profiles(cache: &BTreeMap<String, ProfileEntry>) -> String {
    if cache.is_empty() {
        return "none".to_owned();
    }
    cache
        .iter()
        .map(|(dir, e)| match &e.name {
            Some(name) => format!("{dir} ({name})"),
            None => dir.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `Default` or `Profile N`: the only names Chromium itself creates.
fn is_chromium_dir_name(name: &str) -> bool {
    name == "Default"
        || name
            .strip_prefix("Profile ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The path-traversal guard. A directory name is joined onto the user-data
/// path only if it is one plain component, is not a pseudo-profile, and is
/// either something Chromium would create or something `Local State` lists.
fn guard_dir_name(name: &str, cache: &BTreeMap<String, ProfileEntry>) -> Result<(), ProfileError> {
    let invalid = || ProfileError::InvalidName(name.to_owned());
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.ends_with(['.', ' '])
    {
        return Err(invalid());
    }
    if Path::new(name).components().count() != 1 {
        return Err(invalid());
    }
    if name == "Guest Profile" || name == "System Profile" {
        return Err(ProfileError::NotBrowsing(name.to_owned()));
    }
    if let Some(entry) = cache.get(name) {
        if entry.is_ephemeral {
            return Err(ProfileError::NotBrowsing(name.to_owned()));
        }
        return Ok(());
    }
    if is_chromium_dir_name(name) {
        return Ok(());
    }
    Err(invalid())
}

/// A `Session_<n>` file and its suffix, which is the key Chrome sorts by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCandidate {
    pub path: PathBuf,
    pub suffix: i64,
}

/// Every `Session_<digits>` regular file, newest suffix first. Missing
/// directory counts as no candidates, not as an error.
pub fn session_candidates(sessions_dir: &Path) -> std::io::Result<Vec<SessionCandidate>> {
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|n| session_suffix(n, "Session_")) else {
            continue;
        };
        if entry.file_type()?.is_file() {
            found.push(SessionCandidate {
                path: entry.path(),
                suffix,
            });
        }
    }
    found.sort_by_key(|c| std::cmp::Reverse(c.suffix));
    Ok(found)
}

/// `TimestampFromPath`: exactly `<prefix><int64>`, nothing else.
pub fn session_suffix(name: &str, prefix: &str) -> Option<i64> {
    let digits = name.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The suffix is microseconds since 1601-01-01 UTC: when Chrome created the log.
pub fn suffix_timestamp(suffix: i64) -> Option<Timestamp> {
    crate::session::chrome_time(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(local_state: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Local State"), local_state).unwrap();
        dir
    }

    const STATE: &str = r#"{"profile": {
        "info_cache": {
            "Default": {"name": "Person 1", "is_using_default_name": true},
            "Profile 1": {"name": "Work"},
            "Profile 4": {"name": "work"},
            "Profile 9": {"name": "Ghost", "is_ephemeral": true}
        },
        "last_active_profiles": ["Profile 1"],
        "last_used": "Profile 1"
    }}"#;

    #[test]
    fn requested_profile_by_directory_or_display_name() {
        let dir = setup(STATE);
        for d in ["Default", "Profile 1", "Profile 4"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        let p = resolve_profile(dir.path(), Some("Profile 1")).unwrap();
        assert_eq!(p.dir_name, "Profile 1");
        assert_eq!(p.display.as_deref(), Some("Work"));
        assert_eq!(p.path, dir.path().join("Profile 1"));

        let p = resolve_profile(dir.path(), Some("Work")).unwrap();
        assert_eq!(p.dir_name, "Profile 1");

        // Case-insensitive retry only when it is unambiguous.
        assert!(matches!(
            resolve_profile(dir.path(), Some("WORK")),
            Err(ProfileError::NotFound { .. })
        ));
        let p = resolve_profile(dir.path(), Some("person 1")).unwrap();
        assert_eq!(p.dir_name, "Default");

        assert!(matches!(
            resolve_profile(dir.path(), Some("Nope")),
            Err(ProfileError::NotFound { known, .. }) if known.contains("Profile 1 (Work)")
        ));
    }

    #[test]
    fn ambiguous_display_names_are_an_error() {
        let state = r#"{"profile": {"info_cache": {
            "Profile 1": {"name": "Same"}, "Profile 2": {"name": "Same"}}}}"#;
        let dir = setup(state);
        assert!(matches!(
            resolve_profile(dir.path(), Some("Same")),
            Err(ProfileError::Ambiguous { dirs, .. }) if dirs.contains("Profile 1") && dirs.contains("Profile 2")
        ));
    }

    #[test]
    fn default_profile_follows_chromes_own_preference_order() {
        let dir = setup(STATE);
        std::fs::create_dir(dir.path().join("Default")).unwrap();
        // last_used names a directory that does not exist; last_active does
        // not exist either; Default does.
        assert_eq!(
            resolve_profile(dir.path(), None).unwrap().dir_name,
            "Default"
        );
        std::fs::create_dir(dir.path().join("Profile 1")).unwrap();
        assert_eq!(
            resolve_profile(dir.path(), None).unwrap().dir_name,
            "Profile 1"
        );
    }

    #[test]
    fn default_profile_falls_back_to_one_with_a_session() {
        let state = r#"{"profile": {"info_cache": {"Profile 3": {"name": "Only"}}}}"#;
        let dir = setup(state);
        assert!(matches!(
            resolve_profile(dir.path(), None),
            Err(ProfileError::NoDefault(_))
        ));
        let sessions = dir.path().join("Profile 3").join(SESSIONS_DIR);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("Session_5"), b"x").unwrap();
        assert_eq!(
            resolve_profile(dir.path(), None).unwrap().dir_name,
            "Profile 3"
        );
    }

    #[test]
    fn missing_local_state_still_allows_chromium_names() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_profile(dir.path(), Some("Profile 12"))
                .unwrap()
                .dir_name,
            "Profile 12"
        );
        assert!(matches!(
            resolve_profile(dir.path(), Some("Work")),
            Err(ProfileError::NotFound { .. })
        ));
    }

    #[test]
    fn guard_rejects_traversal_and_pseudo_profiles() {
        let cache = BTreeMap::new();
        for bad in [
            "",
            ".",
            "..",
            "../Default",
            "Default/",
            "Default\\x",
            "/etc",
            "Profile",
            "Profile x",
            "Default.",
            "Default ",
            "CON",
            "Def\0ault",
        ] {
            assert!(
                matches!(
                    guard_dir_name(bad, &cache),
                    Err(ProfileError::InvalidName(_))
                ),
                "{bad:?}"
            );
        }
        for pseudo in ["Guest Profile", "System Profile"] {
            assert!(matches!(
                guard_dir_name(pseudo, &cache),
                Err(ProfileError::NotBrowsing(_))
            ));
        }
        assert!(guard_dir_name("Default", &cache).is_ok());
        assert!(guard_dir_name("Profile 1", &cache).is_ok());
    }

    #[test]
    fn guard_allows_names_local_state_lists() {
        let state: LocalState = serde_json::from_str(
            r#"{"profile": {"info_cache": {"custom-dir": {}, "Ghost": {"is_ephemeral": true}}}}"#,
        )
        .unwrap();
        let cache = &state.profile.info_cache;
        assert!(guard_dir_name("custom-dir", cache).is_ok());
        assert!(matches!(
            guard_dir_name("Ghost", cache),
            Err(ProfileError::NotBrowsing(_))
        ));
        assert!(matches!(
            guard_dir_name("../custom-dir", cache),
            Err(ProfileError::InvalidName(_))
        ));
    }

    #[test]
    fn newest_session_is_the_largest_suffix() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "Session_20",
            "Session_9",
            "Session_100",
            "Session_junk",
            "Session_",
            "Tabs_500",
            "Session_7x",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("Session_999")).unwrap();
        let found = session_candidates(dir.path()).unwrap();
        let suffixes: Vec<i64> = found.iter().map(|c| c.suffix).collect();
        assert_eq!(suffixes, vec![100, 20, 9]);
        assert_eq!(found[0].path, dir.path().join("Session_100"));
        assert!(
            session_candidates(&dir.path().join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn suffix_decodes_to_a_timestamp() {
        let ts = suffix_timestamp(13_434_341_530_659_553).unwrap();
        assert_eq!(ts.to_string(), "2026-09-20T01:32:10.659553Z");
        assert_eq!(session_suffix("Session_12", "Session_"), Some(12));
        assert_eq!(session_suffix("Session_-1", "Session_"), None);
        assert_eq!(
            session_suffix("Session_99999999999999999999", "Session_"),
            None
        );
    }

    #[test]
    fn browser_table_has_chrome() {
        let chrome = browser(CHROME).unwrap();
        let path = chrome.user_data_dir(Path::new("/home/x"));
        assert!(path.starts_with("/home/x"));
        assert!(
            path.ends_with("Chrome")
                || path.ends_with("google-chrome")
                || path.ends_with("User Data")
        );
        assert!(browser("arc").is_none());
    }
}
