//! Typed errors for the whole binary and their one-line rendering.
//!
//! slice: capture
//! why: A failed `save` has to tell the user what to do next in one line,
//!      keep the categories apart so tests and scripts can tell "no Chrome
//!      here" from "archive unwritable", and never show a backtrace. Each
//!      variant carries the path it was about; the chain of causes is shown
//!      only under `-v`. Data-quality outcomes (a torn tail, an unknown
//!      command) are not errors and do not appear here: they are counters in
//!      the snapshot.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::platform::ProfileError;
use crate::snss::HeaderError;
use crate::staleness::StaleReason;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot determine your home directory; pass --root DIR and --session FILE")]
    NoHome,
    #[error("cannot {what} {}: {source}", path.display())]
    Io {
        what: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(
        "no Session_* file under {}; open Chrome once so it writes one, or pass --session FILE",
        .0.display()
    )]
    NoSession(PathBuf),
    #[error(
        "{} is a Tabs_* file, Chrome's recently-closed list, which uses a different command table; pass the Session_* file beside it",
        .0.display()
    )]
    TabsFile(PathBuf),
    #[error("cannot parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: HeaderError,
    },
    #[error("{}", crate::staleness::MESSAGE)]
    Stale {
        profile: PathBuf,
        reason: StaleReason,
    },
    #[error("Chrome changed {} while it was being read; run the command again", .0.display())]
    Unstable(PathBuf),
    #[error("cannot encode {}: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot read library state {}: {reason}; repair the file first", path.display())]
    LibraryState { path: PathBuf, reason: String },
    #[error("not in your library: {}; nothing changed", .0.join(", "))]
    NotInLibrary(Vec<String>),
    #[error("cannot listen on 127.0.0.1:{port}: {source}; pass --port N to use another port")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "cannot embed library data: missing or out-of-order library-data markers in index.html"
    )]
    AssetMarkers,
    #[error("cannot export into {}; choose a directory outside the archive's source data", .0.display())]
    ExportDestination(PathBuf),
}

impl Error {
    /// A stable machine-readable name for `--json` output.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoHome => "no_home",
            Self::Io { .. } => "io",
            Self::Profile(_) => "profile",
            Self::NoSession(_) => "no_session",
            Self::TabsFile(_) => "tabs_file",
            Self::Parse { .. } => "parse",
            Self::Stale { .. } => "stale",
            Self::Unstable(_) => "unstable",
            Self::Json { .. } => "json",
            Self::LibraryState { .. } => "library_state",
            Self::NotInLibrary(_) => "not_in_library",
            Self::Bind { .. } => "bind",
            Self::AssetMarkers => "asset_markers",
            Self::ExportDestination(_) => "export_destination",
        }
    }

    /// 1 for everything except the staleness refusal, which gets its own
    /// code so a cron job can tell "Chrome moved on" from "disk full".
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Stale { .. } => 3,
            _ => 1,
        }
    }

    /// Adapter for `map_err`: `read`, `create`, `rename` and so on.
    pub fn io(what: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { what, path, source }
    }

    /// Extra detail worth showing under `-v` or in JSON.
    fn detail(&self) -> Option<String> {
        match self {
            Self::Stale { profile, reason } => {
                Some(format!("{reason} (profile {})", profile.display()))
            }
            _ => None,
        }
    }
}

/// The text written to stderr. Human form is one line, plus the cause chain
/// under `-v`; JSON form is one object.
pub fn render(error: &Error, verbose: bool, json: bool) -> String {
    if json {
        let value = serde_json::json!({
            "error": {
                "kind": error.kind(),
                "message": error.to_string(),
                "detail": error.detail(),
            }
        });
        return value.to_string();
    }
    let mut out = format!("knowmoretabs: {error}");
    if verbose {
        if let Some(detail) = error.detail() {
            let _ = write!(out, "\n  detail: {detail}");
        }
        let mut cause = std::error::Error::source(error);
        while let Some(c) = cause {
            let _ = write!(out, "\n  caused by: {c}");
            cause = c.source();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_rendering_is_one_line_unless_verbose() {
        let err = Error::Io {
            what: "read",
            path: PathBuf::from("/x/y"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        let plain = render(&err, false, false);
        assert_eq!(plain.lines().count(), 1);
        assert!(plain.starts_with("knowmoretabs: cannot read /x/y"));
        let verbose = render(&err, true, false);
        assert!(verbose.contains("caused by: denied"));
    }

    #[test]
    fn json_rendering_carries_kind_and_detail() {
        let err = Error::Stale {
            profile: PathBuf::from("/p"),
            reason: StaleReason::NoCleartext,
        };
        let value: serde_json::Value = serde_json::from_str(&render(&err, false, true)).unwrap();
        assert_eq!(value["error"]["kind"], "stale");
        assert!(value["error"]["detail"].as_str().unwrap().contains("/p"));
        assert_eq!(err.exit_code(), 3);
        assert_eq!(Error::NoHome.exit_code(), 1);
    }
}
