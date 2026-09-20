//! Where Chrome keeps its files: the user-data directory, profile discovery
//! through `Local State`, and picking the newest `Session_*` log.
//!
//! slice: browsers
//! why: The reference implementation hardcoded one macOS path and trusted a
//!      profile name straight onto the filesystem. This module is the one
//!      place that knowledge lives, shaped as a table whose rows are browsers
//!      and whose columns are operating systems, so that reach is bought by
//!      adding data rather than code paths. The platform is a value
//!      ([`Os`]) rather than a `cfg!`, so every column is exercised by the
//!      tests wherever they run. It never opens a session file: it reads
//!      `Local State`, lists directories, and returns paths.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Deserialize;

/// The stable `--browser` ids supported by this slice.
pub const CHROME: &str = "chrome";
pub const CHROME_BETA: &str = "chrome-beta";
pub const CHROME_CANARY: &str = "chrome-canary";
pub const CHROMIUM: &str = "chromium";
pub const BRAVE: &str = "brave";
pub const EDGE: &str = "edge";
pub const VIVALDI: &str = "vivaldi";
/// Chrome's own name for the session directory (`kSessionsDirectory`).
pub const SESSIONS_DIR: &str = "Sessions";
/// Where it writes the version-5 files it will one day prefer.
pub const ENCRYPTED_SESSIONS_DIR: &str = "Sessions_Encrypted";
/// The archive directory, under the home directory on Unix.
pub const DEFAULT_ROOT_NAME: &str = ".knowmoretabs";
/// The same archive, under `%LOCALAPPDATA%` on Windows: see [`default_root`].
pub const DEFAULT_ROOT_NAME_WINDOWS: &str = "knowmoretabs";

/// Which column of the browser table applies. A value rather than a `cfg!`
/// so that the Linux and Windows rows are covered by tests run on any
/// machine; [`Os::HOST`] is the one the binary actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub const HOST: Self = if cfg!(target_os = "macos") {
        Self::Mac
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Linux
    };
}

/// One row of the browser table: the id, and where its user data lives on
/// each platform. Every column is relative to one of [`Roots`]'s directories,
/// named in the field comments, because "relative to home" is only true on
/// macOS.
///
/// A column is a list of **directory names**, not a path. `browsers.md`
/// spells these with `/` because a document has to spell them somehow, and
/// carrying that spelling into the code produced
/// `…\AppData\Local\Google/Chrome/User Data\Default\…` on Windows, which
/// works — Windows accepts either separator — and is wrong, and which reached
/// `snapshot.json`. Names are joined one at a time, so the separator is
/// always the platform's own, and [`no_column_hides_a_separator_inside_a_name`]
/// stops one creeping back in.
#[derive(Debug, Clone, Copy)]
pub struct BrowserSpec {
    pub id: &'static str,
    pub channel_rank: u8,
    /// Under the home directory.
    pub macos: &'static [&'static str],
    /// Under the config home: `$CHROME_CONFIG_HOME` for the Chrome family,
    /// else `$XDG_CONFIG_HOME`, else `~/.config`.
    pub linux: &'static [&'static str],
    /// Snap and Flatpak, under the home directory. These are separate
    /// installs with their own user data, not aliases of the native path
    /// (`browsers.md` §2.1), so they are extra candidates and the host's XDG
    /// variables do not apply to them.
    pub linux_packaged: &'static [&'static [&'static str]],
    /// Under `%LOCALAPPDATA%`. The trailing `User Data` is Chromium's
    /// `kUserDataDirname`, which exists on Windows only (`browsers.md` §2.4).
    pub windows: &'static [&'static str],
}

/// The one place a table column becomes a path.
fn under(base: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .fold(base.to_path_buf(), |path, name| path.join(name))
}

/// macOS puts every browser under the same two directories.
const APP_SUPPORT: [&str; 2] = ["Library", "Application Support"];
/// Windows appends Chromium's `kUserDataDirname` to the product directory.
const USER_DATA: &str = "User Data";

pub const BROWSERS: [BrowserSpec; 7] = [
    BrowserSpec {
        id: CHROME,
        channel_rank: 0,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Google", "Chrome"],
        linux: &["google-chrome"],
        linux_packaged: &[&[
            ".var",
            "app",
            "com.google.Chrome",
            "config",
            "google-chrome",
        ]],
        windows: &["Google", "Chrome", USER_DATA],
    },
    BrowserSpec {
        id: CHROME_BETA,
        channel_rank: 1,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Google", "Chrome Beta"],
        linux: &["google-chrome-beta"],
        linux_packaged: &[],
        windows: &["Google", "Chrome Beta", USER_DATA],
    },
    BrowserSpec {
        id: CHROME_CANARY,
        channel_rank: 3,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Google", "Chrome Canary"],
        linux: &["google-chrome-canary"],
        linux_packaged: &[],
        // `Chrome SxS`, not `Chrome Canary`, on this platform alone.
        windows: &["Google", "Chrome SxS", USER_DATA],
    },
    BrowserSpec {
        id: CHROMIUM,
        channel_rank: 0,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Chromium"],
        linux: &["chromium"],
        // The second snap entry is the pre-migration layout, still in place
        // on machines that installed the snap before it moved to `common`.
        linux_packaged: &[
            &["snap", "chromium", "common", "chromium"],
            &["snap", "chromium", "current", ".config", "chromium"],
            &[".var", "app", "org.chromium.Chromium", "config", "chromium"],
        ],
        windows: &["Chromium", USER_DATA],
    },
    BrowserSpec {
        id: BRAVE,
        channel_rank: 0,
        macos: &[
            APP_SUPPORT[0],
            APP_SUPPORT[1],
            "BraveSoftware",
            "Brave-Browser",
        ],
        linux: &["BraveSoftware", "Brave-Browser"],
        linux_packaged: &[
            &[
                "snap",
                "brave",
                "common",
                ".config",
                "BraveSoftware",
                "Brave-Browser",
            ],
            &[
                "snap",
                "brave",
                "current",
                ".config",
                "BraveSoftware",
                "Brave-Browser",
            ],
            &[
                ".var",
                "app",
                "com.brave.Browser",
                "config",
                "BraveSoftware",
                "Brave-Browser",
            ],
        ],
        windows: &["BraveSoftware", "Brave-Browser", USER_DATA],
    },
    BrowserSpec {
        id: EDGE,
        channel_rank: 0,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Microsoft Edge"],
        linux: &["microsoft-edge"],
        linux_packaged: &[&[
            ".var",
            "app",
            "com.microsoft.Edge",
            "config",
            "microsoft-edge",
        ]],
        windows: &["Microsoft", "Edge", USER_DATA],
    },
    BrowserSpec {
        id: VIVALDI,
        channel_rank: 0,
        macos: &[APP_SUPPORT[0], APP_SUPPORT[1], "Vivaldi"],
        linux: &["vivaldi"],
        linux_packaged: &[&[".var", "app", "com.vivaldi.Vivaldi", "config", "vivaldi"]],
        windows: &["Vivaldi", USER_DATA],
    },
];

/// `CHROME_CONFIG_HOME` and `CHROME_USER_DATA_DIR` are Chrome's own variables.
/// A fork built from the same source reads them too, but someone who set one
/// for Chrome would then have every browser "found" at that one directory, so
/// `browsers.md` §7 restricts them to the Chrome family, and so does this.
fn honours_chrome_env(id: &str) -> bool {
    matches!(id, CHROME | CHROME_BETA | CHROME_CANARY | CHROMIUM)
}

/// The per-platform directories the browser table hangs off, resolved once
/// from the environment. Splitting this out is what makes the table data:
/// a row says "`google-chrome` under the config home" and this says where
/// the config home is on this machine.
#[derive(Debug, Clone)]
pub struct Roots {
    pub os: Os,
    pub home: PathBuf,
    /// Linux: `$XDG_CONFIG_HOME`, else `~/.config`.
    config: PathBuf,
    /// Linux, Chrome family: `$CHROME_CONFIG_HOME`, else `config`.
    chrome_config: PathBuf,
    /// Linux, Chrome family: `$CHROME_USER_DATA_DIR`, which names a whole
    /// user-data directory and so replaces the native path rather than
    /// prefixing it.
    chrome_user_data: Option<PathBuf>,
    /// Windows: `%LOCALAPPDATA%`, else `FOLDERID_LocalAppData`.
    local_app_data: PathBuf,
}

impl Roots {
    /// Reads the environment once. `None` only when there is no home
    /// directory at all, which is the one case nothing can be resolved from.
    pub fn detect() -> Option<Self> {
        let home = home_dir()?;
        // The known folder is consulted only as the fallback the variable
        // does not provide, which is the property `browsers.md` §7 asks of it.
        let known_local_app_data = match Os::HOST {
            Os::Windows => directories::BaseDirs::new().map(|d| d.data_local_dir().to_path_buf()),
            _ => None,
        };
        Some(Self::resolve(
            Os::HOST,
            &home,
            known_local_app_data,
            |name| std::env::var_os(name),
        ))
    }

    /// The environment rules, with the environment passed in so the tests can
    /// state one. `known_local_app_data` is the Windows known folder.
    fn resolve(
        os: Os,
        home: &Path,
        known_local_app_data: Option<PathBuf>,
        var: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        // Chromium's `GetXDGDirectory` takes any non-empty value and strips
        // trailing separators; it does not require an absolute path, and
        // neither does this, so that we look where Chrome would look.
        let value = |name: &str| {
            var(name)
                .filter(|v| !v.is_empty())
                .map(|v| PathBuf::from(v).components().collect::<PathBuf>())
        };
        // `chrome_main_delegate.cc` reads `CHROME_USER_DATA_DIR` on Linux and
        // ChromeOS only, and the XDG variables mean nothing to Chromium on
        // macOS or Windows.
        let (config, chrome_config, chrome_user_data) = if os == Os::Linux {
            let config = value("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
            let chrome_config = value("CHROME_CONFIG_HOME").unwrap_or_else(|| config.clone());
            (config, chrome_config, value("CHROME_USER_DATA_DIR"))
        } else {
            let config = home.join(".config");
            (config.clone(), config, None)
        };
        let profile_local = home.join("AppData").join("Local");
        let local_app_data = if os == Os::Windows {
            value("LOCALAPPDATA")
                .or(known_local_app_data)
                .unwrap_or(profile_local)
        } else {
            profile_local
        };
        Self {
            os,
            home: home.to_path_buf(),
            config,
            chrome_config,
            chrome_user_data,
            local_app_data,
        }
    }

    fn config_home(&self, id: &str) -> &Path {
        if honours_chrome_env(id) {
            &self.chrome_config
        } else {
            &self.config
        }
    }
}

/// Where the archive lives when `--root` is not passed.
///
/// `~/.knowmoretabs` on Unix. On Windows it is `%LOCALAPPDATA%\knowmoretabs`,
/// not `%USERPROFILE%\.knowmoretabs`, for two reasons. A dotfile in the
/// profile root is a Unix idiom that no Windows tool follows, and more
/// importantly `%USERPROFILE%` is the directory enterprise folder redirection
/// roams to a file server: an archive of every page the user has had open is
/// exactly the thing that must not be copied off the machine. `LocalAppData`
/// is the location Windows defines as per-user and deliberately non-roaming,
/// and a directory created there inherits an ACL that grants the user,
/// SYSTEM and Administrators and nobody else — which is the nearest Windows
/// has to the `0700` the Unix root is created with.
pub fn default_root(roots: &Roots) -> PathBuf {
    if roots.os == Os::Windows {
        roots.local_app_data.join(DEFAULT_ROOT_NAME_WINDOWS)
    } else {
        roots.home.join(DEFAULT_ROOT_NAME)
    }
}

impl BrowserSpec {
    /// Every directory this browser's user data can be in, most likely first.
    /// Linux has more than one because native, Snap and Flatpak installs of
    /// the same browser do not share a user-data directory and a machine can
    /// have two; the caller probes all of them and lets recency decide.
    pub fn user_data_dirs(&self, roots: &Roots) -> Vec<PathBuf> {
        match roots.os {
            Os::Mac => vec![under(&roots.home, self.macos)],
            Os::Windows => vec![under(&roots.local_app_data, self.windows)],
            Os::Linux => {
                let native = match &roots.chrome_user_data {
                    Some(dir) if honours_chrome_env(self.id) => dir.clone(),
                    _ => under(roots.config_home(self.id), self.linux),
                };
                std::iter::once(native)
                    .chain(
                        self.linux_packaged
                            .iter()
                            .map(|names| under(&roots.home, names)),
                    )
                    .collect()
            }
        }
    }
}

pub fn browser(id: &str) -> Option<&'static BrowserSpec> {
    BROWSERS.iter().find(|b| b.id == id)
}

/// `$HOME` on Unix and `%USERPROFILE%` on Windows, each falling back to the
/// platform's own lookup — the rule `std::env::home_dir` documents. The
/// variable has to come first: it is how a relocated profile, and how the
/// tests, name a home directory other than the logged-in user's.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

/// Why Windows could not use a directory as the archive root. Every one of
/// these names is legal on macOS and Linux, which is why the rule is applied
/// only where it is true — but the rule itself is data, and is tested
/// everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootProblem {
    /// `CON`, `NUL`, `COM3` and friends. `CreateFile` opens the device.
    Reserved(String),
    /// Windows strips these, so the directory created is not the one named.
    TrailingDotOrSpace(String),
    Illegal {
        component: String,
        character: char,
    },
    /// Nothing the archive writes under this root could be opened.
    TooLong {
        length: usize,
        limit: usize,
    },
}

impl std::fmt::Display for RootProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reserved(name) => write!(
                f,
                "{name:?} is a reserved Windows device name; pass a --root Windows can open as a directory"
            ),
            Self::TrailingDotOrSpace(name) => write!(
                f,
                "{name:?} ends with a dot or a space, which Windows strips, so the archive would not be where you asked for it"
            ),
            Self::Illegal {
                component,
                character,
            } => write!(
                f,
                "{component:?} contains {character:?}, which Windows does not allow in a file name"
            ),
            Self::TooLong { length, limit } => write!(
                f,
                "the path is {length} characters and the files the archive writes under it would pass Windows' {limit}-character limit; pass a shorter --root"
            ),
        }
    }
}

/// The reserved device names, from the Windows file-naming rules. A name is
/// reserved whatever extension follows it, so `CON.txt` is `CON`.
const RESERVED_DEVICE_NAMES: [&str; 30] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
];

/// The longest thing the archive appends to its root: `snapshots/`, a staging
/// directory (`.staging-` plus tempfile's six random characters), and the
/// longest file name inside it.
const DEEPEST_ARCHIVE_SUFFIX: usize = r"\snapshots\.staging-abcdef\snapshot.json".len();
/// Windows' own ceiling, verbatim prefix included. Rust's standard library
/// switches to `\\?\` form past 248 characters, so the classic 260 limit is
/// not ours; this one is.
const WINDOWS_PATH_LIMIT: usize = 32_767;

/// The `--root` guard. Applied through `cfg!` rather than `#[cfg]` so the
/// rules are compiled, and the type constructed, on every platform: the
/// behaviour is Windows-only, the table behind it is not.
pub fn check_root(root: &Path) -> Result<(), RootProblem> {
    match windows_root_problem(root) {
        Some(problem) if cfg!(windows) => Err(problem),
        _ => Ok(()),
    }
}

/// What Windows would make of `root`, whoever is asking.
pub fn windows_root_problem(root: &Path) -> Option<RootProblem> {
    let limit = WINDOWS_PATH_LIMIT - r"\\?\".len();
    let length = root.as_os_str().len() + DEEPEST_ARCHIVE_SUFFIX;
    if length > limit {
        return Some(RootProblem::TooLong { length, limit });
    }
    for component in root.components() {
        // A drive letter's colon and the separators are the path's own
        // syntax, not a name; only the names between them are checked.
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let name = name.to_string_lossy();
        if let Some(bad) = name
            .chars()
            .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (*c as u32) < 0x20)
        {
            return Some(RootProblem::Illegal {
                component: name.into_owned(),
                character: bad,
            });
        }
        if name.ends_with(['.', ' ']) {
            return Some(RootProblem::TrailingDotOrSpace(name.into_owned()));
        }
        let stem = name.split('.').next().unwrap_or(&name);
        if RESERVED_DEVICE_NAMES
            .iter()
            .any(|r| stem.eq_ignore_ascii_case(r))
        {
            return Some(RootProblem::Reserved(name.into_owned()));
        }
    }
    None
}

/// Whether `path` is `ancestor` or sits inside it.
///
/// `Path::starts_with` compares components byte for byte. That is right on a
/// case-sensitive filesystem and wrong on the macOS and Windows defaults,
/// where `~/.knowmoretabs` and `~/.KNOWMORETABS` are one directory — and a
/// guard that cannot see that is a guard that can be spelled around. Folding
/// is ASCII-only, which covers the paths this guards and errs towards
/// refusing rather than allowing.
pub fn contains_path(ancestor: &Path, path: &Path) -> bool {
    if Os::HOST == Os::Linux {
        return path.starts_with(ancestor);
    }
    let mut candidate = path.components();
    ancestor.components().all(|want| {
        candidate
            .next()
            .is_some_and(|have| have.as_os_str().eq_ignore_ascii_case(want.as_os_str()))
    })
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
    #[error("profile directory {0} is not a directory")]
    NotDirectory(PathBuf),
}

/// A resolved profile: the directory Chrome uses and the name people see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub dir_name: String,
    pub display: Option<String>,
    pub path: PathBuf,
}

/// A browser/profile pair that can be selected by zero-flag discovery.
///
/// `suffix` is the recency the scan ranks by. For a stale profile (see
/// `staleness`) it is the newer of the cleartext and encrypted `Session_*`
/// suffixes, so a browser whose cleartext files stopped moving still competes
/// on when it was really last used and cannot be quietly outranked by a
/// browser the user touched less recently.
#[derive(Debug, Clone)]
pub struct BrowserCandidate {
    pub browser: &'static BrowserSpec,
    pub profile: Profile,
    pub suffix: i64,
    pub modified: Option<std::time::SystemTime>,
    /// `Some` when the profile's encrypted sessions are newer than its
    /// cleartext ones. Such a candidate is never saved: if it wins the scan
    /// the run refuses, and if it loses it is reported as stale.
    pub stale: Option<crate::staleness::StaleReason>,
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
    let joined = user_data.join(&dir_name);
    let path = match std::fs::symlink_metadata(&joined) {
        Ok(metadata) if metadata.file_type().is_symlink() => match std::fs::canonicalize(&joined) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => return Err(ProfileError::NotDirectory(joined)),
            Err(_) => joined,
        },
        Ok(metadata) if metadata.is_dir() => joined,
        Ok(_) => return Err(ProfileError::NotDirectory(joined)),
        Err(_) => joined,
    };
    Ok(Profile {
        display: cache.get(&dir_name).and_then(|e| e.name.clone()),
        path,
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
    if name.is_empty() {
        return Err(ProfileError::InvalidName(name.to_owned()));
    }
    // `browsers.md` §4.1: a directory name listed in `Local State` wins, then
    // a display name, then the names Chromium would create even if `Local
    // State` is missing. A display name is user-controlled text and may
    // contain separators or `..`; it is matched here as text and only the
    // directory it resolves to goes through `guard_dir_name`.
    if cache.contains_key(name) {
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
    if is_chromium_dir_name(name) {
        return Ok(name.to_owned());
    }
    if is_pseudo_profile(name) {
        return Err(ProfileError::NotBrowsing(name.to_owned()));
    }
    if name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(ProfileError::InvalidName(name.to_owned()));
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
    if is_pseudo_profile(name) {
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

fn is_pseudo_profile(name: &str) -> bool {
    matches!(
        name,
        "Guest" | "Guest Profile" | "System" | "System Profile"
    )
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error("cannot list {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve the preferred profile, then use the first listed profile that has
/// a session if the preferred profile is empty. Profile order is intentional:
/// cross-browser recency chooses the browser, while Chromium's own preference
/// chooses the profile inside that browser.
pub fn profile_with_session(
    user_data: &Path,
    requested: Option<&str>,
) -> Result<(Profile, Vec<SessionCandidate>), DiscoveryError> {
    let preferred = resolve_profile(user_data, requested)?;
    let preferred_sessions =
        session_candidates(&preferred.path.join(SESSIONS_DIR)).map_err(|source| {
            DiscoveryError::Io {
                path: preferred.path.join(SESSIONS_DIR),
                source,
            }
        })?;
    if requested.is_some() || !preferred_sessions.is_empty() {
        return Ok((preferred, preferred_sessions));
    }

    // This is a search, not a request: a listed profile whose directory is
    // missing, unreadable, or not a directory is passed over so it cannot
    // hide a later profile that does have a session. The preferred profile's
    // own failures were reported above.
    let state = read_local_state(user_data)?;
    for dir in state.profile.info_cache.keys() {
        if dir == &preferred.dir_name || guard_dir_name(dir, &state.profile.info_cache).is_err() {
            continue;
        }
        let Ok(profile) = resolve_profile(user_data, Some(dir)) else {
            continue;
        };
        let Ok(sessions) = session_candidates(&profile.path.join(SESSIONS_DIR)) else {
            continue;
        };
        if !sessions.is_empty() {
            return Ok((profile, sessions));
        }
    }
    Ok((preferred, preferred_sessions))
}

/// The newest `Session_*` under a profile's `Sessions_Encrypted/`: its suffix
/// and mtime. Used only to rank a profile the staleness check has already
/// refused, so a listing failure here is "nothing newer known", not an error.
pub fn encrypted_recency(profile_path: &Path) -> Option<(i64, Option<std::time::SystemTime>)> {
    let newest = session_candidates(&profile_path.join(ENCRYPTED_SESSIONS_DIR))
        .ok()?
        .into_iter()
        .next()?;
    Some((newest.suffix, file_modified(&newest.path)))
}

pub fn file_modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// The candidate ordering used by zero-flag capture. A suffix is Chromium's
/// recency key; mtime resolves synthetic or copied files with equal suffixes.
pub fn candidate_cmp(a: &BrowserCandidate, b: &BrowserCandidate) -> std::cmp::Ordering {
    a.suffix
        .cmp(&b.suffix)
        .then_with(|| a.modified.cmp(&b.modified))
        .then_with(|| b.browser.channel_rank.cmp(&a.browser.channel_rank))
        .then_with(|| b.browser.id.cmp(a.browser.id))
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

    // --- The platform table ---------------------------------------------
    //
    // These run on every platform and cover every column, because the column
    // is chosen by the `Os` passed in rather than by the machine the tests
    // happen to be on. Expected paths are built by joining, so the separator
    // is the host's on both sides of each assertion.

    const HOME: &str = if cfg!(windows) {
        r"C:\Users\person"
    } else {
        "/home/person"
    };

    /// A path under the fake home, written with `/` and joined so the
    /// separator is the host's on both sides of every assertion.
    fn home(relative: &str) -> PathBuf {
        relative
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(PathBuf::from(HOME), |path, part| path.join(part))
    }

    fn roots(os: Os, env: &[(&str, &str)]) -> Roots {
        roots_with(os, None, env)
    }

    /// Environment values are home-relative so that every one of them is a
    /// path the host filesystem would accept, Windows drive letters included.
    fn roots_with(os: Os, known_local_app_data: Option<PathBuf>, env: &[(&str, &str)]) -> Roots {
        let env: Vec<(&str, OsString)> = env
            .iter()
            .map(|(k, v)| {
                (
                    *k,
                    if v.is_empty() {
                        OsString::new()
                    } else {
                        home(v).into_os_string()
                    },
                )
            })
            .collect();
        Roots::resolve(os, Path::new(HOME), known_local_app_data, |name| {
            env.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())
        })
    }

    fn dirs(os: Os, id: &str, env: &[(&str, &str)]) -> Vec<PathBuf> {
        browser(id).unwrap().user_data_dirs(&roots(os, env))
    }

    /// Every cell of the table is a list of directory names. A name holding a
    /// separator would be joined whole, and the path would then carry that
    /// separator wherever it was printed, stored or compared — which is how
    /// `Google/Chrome/User Data` ended up in the middle of a backslash path,
    /// and in `snapshot.json`. This runs on every platform because the table
    /// is the same on every platform; the bug only *showed* on one.
    #[test]
    fn no_column_hides_a_separator_inside_a_name() {
        for spec in &BROWSERS {
            let columns = [spec.macos, spec.linux, spec.windows]
                .into_iter()
                .chain(spec.linux_packaged.iter().copied());
            for name in columns.flatten() {
                assert!(!name.is_empty(), "{}: empty directory name", spec.id);
                assert!(
                    !name.contains(['/', '\\']),
                    "{}: {name:?} is a path, not a directory name",
                    spec.id
                );
                assert_eq!(
                    Path::new(name).components().count(),
                    1,
                    "{}: {name:?} is not one path component",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn macos_paths_are_under_application_support_and_ignore_the_linux_variables() {
        let xdg = [
            ("XDG_CONFIG_HOME", "elsewhere"),
            ("CHROME_CONFIG_HOME", "x"),
        ];
        for (id, relative) in [
            (CHROME, "Library/Application Support/Google/Chrome"),
            (
                CHROME_BETA,
                "Library/Application Support/Google/Chrome Beta",
            ),
            (
                CHROME_CANARY,
                "Library/Application Support/Google/Chrome Canary",
            ),
            (CHROMIUM, "Library/Application Support/Chromium"),
            (
                BRAVE,
                "Library/Application Support/BraveSoftware/Brave-Browser",
            ),
            (EDGE, "Library/Application Support/Microsoft Edge"),
            (VIVALDI, "Library/Application Support/Vivaldi"),
        ] {
            assert_eq!(dirs(Os::Mac, id, &[]), vec![home(relative)], "{id}");
            assert_eq!(dirs(Os::Mac, id, &xdg), vec![home(relative)], "{id}");
        }
        assert!(browser("arc").is_none());
    }

    #[test]
    fn linux_native_paths_hang_off_the_config_home() {
        for (id, relative) in [
            (CHROME, ".config/google-chrome"),
            (CHROME_BETA, ".config/google-chrome-beta"),
            (CHROME_CANARY, ".config/google-chrome-canary"),
            (CHROMIUM, ".config/chromium"),
            (BRAVE, ".config/BraveSoftware/Brave-Browser"),
            (EDGE, ".config/microsoft-edge"),
            (VIVALDI, ".config/vivaldi"),
        ] {
            assert_eq!(dirs(Os::Linux, id, &[])[0], home(relative), "{id}");
        }
    }

    #[test]
    fn linux_honours_xdg_and_the_two_chrome_variables() {
        let xdg = [("XDG_CONFIG_HOME", "cfg")];
        assert_eq!(dirs(Os::Linux, CHROME, &xdg)[0], home("cfg/google-chrome"));
        assert_eq!(
            dirs(Os::Linux, BRAVE, &xdg)[0],
            home("cfg/BraveSoftware/Brave-Browser")
        );

        // CHROME_CONFIG_HOME replaces XDG for the Chrome family only, and
        // channels keep their distinct suffixes under it.
        let both = [("XDG_CONFIG_HOME", "cfg"), ("CHROME_CONFIG_HOME", "chr")];
        assert_eq!(dirs(Os::Linux, CHROME, &both)[0], home("chr/google-chrome"));
        assert_eq!(
            dirs(Os::Linux, CHROME_BETA, &both)[0],
            home("chr/google-chrome-beta")
        );
        assert_eq!(dirs(Os::Linux, CHROMIUM, &both)[0], home("chr/chromium"));
        assert_eq!(dirs(Os::Linux, EDGE, &both)[0], home("cfg/microsoft-edge"));
        assert_eq!(dirs(Os::Linux, VIVALDI, &both)[0], home("cfg/vivaldi"));

        // CHROME_USER_DATA_DIR names a whole user-data directory, so it
        // replaces the native path rather than prefixing it — and a person
        // who set it for Chrome must not find every browser at that one path.
        let user_data = [
            ("XDG_CONFIG_HOME", "cfg"),
            ("CHROME_CONFIG_HOME", "chr"),
            ("CHROME_USER_DATA_DIR", "data/chrome"),
        ];
        assert_eq!(dirs(Os::Linux, CHROME, &user_data)[0], home("data/chrome"));
        assert_eq!(
            dirs(Os::Linux, CHROMIUM, &user_data)[0],
            home("data/chrome")
        );
        assert_eq!(
            dirs(Os::Linux, BRAVE, &user_data)[0],
            home("cfg/BraveSoftware/Brave-Browser")
        );

        // Empty is unset, and a trailing separator is not a new component.
        assert_eq!(
            dirs(Os::Linux, CHROME, &[("XDG_CONFIG_HOME", "")])[0],
            home(".config/google-chrome")
        );
        assert_eq!(
            dirs(Os::Linux, CHROME, &[("XDG_CONFIG_HOME", "cfg/")])[0],
            home("cfg/google-chrome")
        );
    }

    #[test]
    fn linux_snap_and_flatpak_are_extra_candidates_the_host_xdg_never_rewrites() {
        let xdg = [("XDG_CONFIG_HOME", "cfg"), ("CHROME_CONFIG_HOME", "chr")];
        let expected = [
            (
                CHROME,
                vec![".var/app/com.google.Chrome/config/google-chrome"],
            ),
            (CHROME_BETA, vec![]),
            (CHROME_CANARY, vec![]),
            (
                CHROMIUM,
                vec![
                    "snap/chromium/common/chromium",
                    "snap/chromium/current/.config/chromium",
                    ".var/app/org.chromium.Chromium/config/chromium",
                ],
            ),
            (
                BRAVE,
                vec![
                    "snap/brave/common/.config/BraveSoftware/Brave-Browser",
                    "snap/brave/current/.config/BraveSoftware/Brave-Browser",
                    ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
                ],
            ),
            (
                EDGE,
                vec![".var/app/com.microsoft.Edge/config/microsoft-edge"],
            ),
            (VIVALDI, vec![".var/app/com.vivaldi.Vivaldi/config/vivaldi"]),
        ];
        for (id, packaged) in expected {
            let want: Vec<PathBuf> = packaged.iter().map(|p| home(p)).collect();
            // Sandboxes hard-code their own config root: the host's variables
            // change the native path and leave these exactly where they are.
            for env in [&[][..], &xdg[..]] {
                assert_eq!(dirs(Os::Linux, id, env)[1..], want[..], "{id}");
            }
        }
        // Not a platform where a browser is packaged twice.
        for os in [Os::Mac, Os::Windows] {
            assert_eq!(dirs(os, CHROMIUM, &[]).len(), 1);
        }
    }

    #[test]
    fn windows_paths_hang_off_local_app_data_with_the_user_data_component() {
        let env = [("LOCALAPPDATA", "state/Local")];
        let local = |relative: &str| home(&format!("state/Local/{relative}"));
        for (id, relative) in [
            (CHROME, "Google/Chrome/User Data"),
            (CHROME_BETA, "Google/Chrome Beta/User Data"),
            // Chrome Canary's directory is `Chrome SxS`, not `Chrome Canary`.
            (CHROME_CANARY, "Google/Chrome SxS/User Data"),
            (CHROMIUM, "Chromium/User Data"),
            (BRAVE, "BraveSoftware/Brave-Browser/User Data"),
            (EDGE, "Microsoft/Edge/User Data"),
            (VIVALDI, "Vivaldi/User Data"),
        ] {
            assert_eq!(dirs(Os::Windows, id, &env), vec![local(relative)], "{id}");
        }
        // Unset falls back to the known folder, and to a path under the
        // profile only when even that is unavailable.
        let known = roots_with(Os::Windows, Some(home("known")), &[]);
        assert_eq!(
            browser(CHROME).unwrap().user_data_dirs(&known)[0],
            home("known/Google/Chrome/User Data")
        );
        // The variable wins over the known folder, which is what lets a
        // relocated profile, and these tests, name somewhere else.
        let both = roots_with(Os::Windows, Some(home("known")), &env);
        assert_eq!(
            browser(CHROME).unwrap().user_data_dirs(&both)[0],
            local("Google/Chrome/User Data")
        );
        assert_eq!(
            dirs(Os::Windows, CHROME, &[])[0],
            home("AppData/Local/Google/Chrome/User Data")
        );
    }

    #[test]
    fn the_default_archive_root_is_private_per_platform() {
        assert_eq!(default_root(&roots(Os::Mac, &[])), home(".knowmoretabs"));
        // `$XDG_CONFIG_HOME` is Chromium's, not ours: the archive is not
        // config and does not move when a browser's config root does.
        assert_eq!(
            default_root(&roots(Os::Linux, &[("XDG_CONFIG_HOME", "cfg")])),
            home(".knowmoretabs")
        );
        // Not `%USERPROFILE%\.knowmoretabs`: LocalAppData is the per-user
        // directory Windows defines as never roaming to a file server.
        assert_eq!(
            default_root(&roots(Os::Windows, &[("LOCALAPPDATA", "state/Local")])),
            home("state/Local/knowmoretabs")
        );
    }

    #[test]
    fn windows_rejects_root_names_the_other_platforms_accept() {
        let root = |name: &str| PathBuf::from(HOME).join(name).join("archive");
        for (name, expected) in [
            ("CON", RootProblem::Reserved("CON".to_owned())),
            ("nul", RootProblem::Reserved("nul".to_owned())),
            ("COM9", RootProblem::Reserved("COM9".to_owned())),
            ("LPT1.txt", RootProblem::Reserved("LPT1.txt".to_owned())),
            ("aux", RootProblem::Reserved("aux".to_owned())),
            ("tabs.", RootProblem::TrailingDotOrSpace("tabs.".to_owned())),
            ("tabs ", RootProblem::TrailingDotOrSpace("tabs ".to_owned())),
            (
                "a|b",
                RootProblem::Illegal {
                    component: "a|b".to_owned(),
                    character: '|',
                },
            ),
            (
                "alt:stream",
                RootProblem::Illegal {
                    component: "alt:stream".to_owned(),
                    character: ':',
                },
            ),
        ] {
            assert_eq!(windows_root_problem(&root(name)), Some(expected), "{name}");
        }
        // `HOME` carries the drive letter on Windows, so these also say that
        // the path's own syntax — `C:` and the separators — is not a name and
        // its colon is not one of the colons above.
        for fine in [
            "knowmoretabs",
            ".knowmoretabs",
            "CONtext",
            "COM0",
            "LPT0",
            "COM10",
            "my tabs",
            "tabs.d",
            "Ünïcøde",
        ] {
            assert_eq!(windows_root_problem(&root(fine)), None, "{fine}");
        }
    }

    #[test]
    fn a_root_is_too_long_only_when_what_we_write_under_it_would_not_fit() {
        let limit = WINDOWS_PATH_LIMIT - r"\\?\".len();
        let root = |len: usize| PathBuf::from("a".repeat(len));
        assert_eq!(
            windows_root_problem(&root(limit - DEEPEST_ARCHIVE_SUFFIX)),
            None
        );
        assert_eq!(
            windows_root_problem(&root(limit - DEEPEST_ARCHIVE_SUFFIX + 1)),
            Some(RootProblem::TooLong {
                length: limit + 1,
                limit
            })
        );
        // The classic 260 is not the limit: the standard library switches to
        // `\\?\` form past 248 characters, so a root of that order works.
        assert_eq!(windows_root_problem(&root(300)), None);
    }

    #[test]
    fn contains_path_folds_case_where_the_filesystem_does() {
        let root = home(".knowmoretabs");
        assert!(contains_path(&root, &root.join("export")));
        assert!(contains_path(&root, &root));
        assert!(!contains_path(&root, &home(".knowmoretab")));
        assert!(!contains_path(&root.join("export"), &root));

        let shouted = home(".KNOWMORETABS/snapshots");
        assert_eq!(
            contains_path(&root, &shouted),
            !(Os::HOST == Os::Linux),
            "macOS and Windows treat these as one directory; Linux does not"
        );
    }

    #[test]
    fn missing_last_used_uses_the_first_active_profile() {
        let state = r#"{"profile":{"last_active_profiles":["Profile 2"],"info_cache":{
            "Default":{"name":"Person 1"},"Profile 2":{"name":"Research"}}}}"#;
        let dir = setup(state);
        let sessions = dir.path().join("Profile 2").join(SESSIONS_DIR);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("Session_7"), b"synthetic").unwrap();
        let profile = profile_with_session(dir.path(), None).unwrap().0;
        assert_eq!(profile.dir_name, "Profile 2");
        assert_eq!(profile.display.as_deref(), Some("Research"));
    }

    #[test]
    fn missing_and_stale_preferences_fall_back_to_a_profile_with_a_session() {
        let state = r#"{"profile":{"last_used":"Profile 9",
            "info_cache":{"Default":{"name":"Person 1"},"Profile 2":{"name":"Only"}}}}"#;
        let dir = setup(state);
        let sessions = dir.path().join("Profile 2").join(SESSIONS_DIR);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("Session_8"), b"synthetic").unwrap();
        let profile = profile_with_session(dir.path(), None).unwrap().0;
        assert_eq!(profile.dir_name, "Profile 2");
    }
    #[test]
    fn display_names_resolve_before_lookalike_or_guarded_directory_names() {
        let state = r#"{"profile":{"info_cache":{
            "Profile 1":{"name":"Profile 2"},"Profile 3":{"name":"Work/Home"}}}}"#;
        let dir = setup(state);
        for d in ["Profile 1", "Profile 3"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        // Shaped like a directory Chromium would create, but no such
        // directory is listed: it is the display name of Profile 1.
        let p = resolve_profile(dir.path(), Some("Profile 2")).unwrap();
        assert_eq!(p.dir_name, "Profile 1");
        // A separator in a display name is text; the guard runs on the
        // directory it resolves to.
        let p = resolve_profile(dir.path(), Some("Work/Home")).unwrap();
        assert_eq!(p.dir_name, "Profile 3");
        assert_eq!(p.path, dir.path().join("Profile 3"));
        for bad in ["Work/Else", "../Profile 1", "Profile 1/", "a\\b", ".", ".."] {
            assert!(
                matches!(
                    resolve_profile(dir.path(), Some(bad)),
                    Err(ProfileError::InvalidName(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_broken_listed_profile_does_not_stop_the_search_for_one_with_a_session() {
        let state = r#"{"profile":{"last_used":"Default","info_cache":{
            "Default":{"name":"Person 1"},"Profile 0":{"name":"Broken"},"Profile 1":{"name":"Work"}}}}"#;
        let dir = setup(state);
        std::fs::create_dir_all(dir.path().join("Default").join(SESSIONS_DIR)).unwrap();
        std::fs::write(
            dir.path().join("Profile 0"),
            b"a file where a directory should be",
        )
        .unwrap();
        let sessions = dir.path().join("Profile 1").join(SESSIONS_DIR);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("Session_9"), b"synthetic").unwrap();
        let (profile, found) = profile_with_session(dir.path(), None).unwrap();
        assert_eq!(profile.dir_name, "Profile 1");
        assert_eq!(found.len(), 1);
        // Asked for by name, the broken one is still an error.
        assert!(matches!(
            profile_with_session(dir.path(), Some("Profile 0")),
            Err(DiscoveryError::Profile(ProfileError::NotDirectory(_)))
        ));
    }
}
