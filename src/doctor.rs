//! `knowmoretabs doctor`: which ways of reading pages this machine can use,
//! and what the archive holds of page text.
//!
//! slice: content
//! why: Content capture reads some pages with tools the owner may not have
//!      installed, and falls back quietly when one is missing, so the
//!      owner needs one place that says what is ready, what is missing and
//!      how to fix it, before a long run rather than after. It reads the
//!      machine and the archive offline by default. With `--live`, `gh`
//!      checks its own sign in with GitHub and the X post API is asked
//!      once for a fixed public post. It sends nothing of the owner's.
//!      Missing tools are warnings: only the generic web tier is required,
//!      and it is compiled in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::capture::Log;
use crate::content_store;
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::github_api::Readiness;
use crate::out;
use crate::platform;
use crate::tools::{Probe, System};
use crate::xpost;

/// The one tier content capture cannot do without.
const REQUIRED: &str = "generic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    /// Usable in part, or falling back to another tier.
    Degraded,
    Missing,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Missing => "missing",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub tier: &'static str,
    pub state: State,
    pub detail: String,
    /// What to do about it, when it is not ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn new(tier: &'static str, state: State, detail: impl Into<String>) -> Self {
        Self {
            tier,
            state,
            detail: detail.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// What the archive holds, as `doctor` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Archive {
    pub root: PathBuf,
    pub exists: bool,
    /// Whether only its owner may read it; `None` where that is not
    /// checked (Windows).
    pub private: Option<bool>,
    pub content_dir: bool,
    /// Pages by their latest status.
    pub statuses: BTreeMap<&'static str, usize>,
    pub unreadable_lines: usize,
}

impl Archive {
    pub fn read(root: &Path) -> Result<Self, Error> {
        let exists = root.is_dir();
        let log = content_store::read(root)?;
        let mut statuses = BTreeMap::new();
        for line in log.pages.values() {
            *statuses.entry(line.status.word()).or_default() += 1;
        }
        Ok(Self {
            root: root.to_path_buf(),
            exists,
            private: exists.then(|| is_private(root)).flatten(),
            content_dir: content_store::dir(root).is_dir(),
            statuses,
            unreadable_lines: log.unreadable,
        })
    }

    fn check(&self) -> Check {
        if !self.exists {
            return Check::new(
                "archive",
                State::Missing,
                format!("no archive at {}", self.root.display()),
            )
            .hint("run knowmoretabs save first");
        }
        let mut detail = self.root.display().to_string();
        match self.private {
            Some(true) => detail.push_str(", private"),
            Some(false) => detail.push_str(", readable by other accounts"),
            None => {}
        }
        detail.push_str(if self.content_dir {
            "; pages/content present"
        } else {
            "; no pages/content yet"
        });
        if self.statuses.is_empty() {
            detail.push_str("; no page text captured");
        } else {
            let counts = self
                .statuses
                .iter()
                .map(|(status, n)| format!("{n} {status}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(detail, "; {counts}");
        }
        if self.unreadable_lines > 0 {
            let _ = write!(detail, "; {} unreadable log lines", self.unreadable_lines);
        }
        if self.private == Some(false) {
            return Check::new("archive", State::Degraded, detail)
                .hint(format!("chmod 700 {}", self.root.display()));
        }
        Check::new("archive", State::Ready, detail)
    }
}

#[cfg(unix)]
fn is_private(root: &Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(root).ok()?.permissions().mode();
    // No bit for the group or for others.
    Some(mode.trailing_zeros() >= 6)
}

#[cfg(not(unix))]
fn is_private(_: &Path) -> Option<bool> {
    None
}

/// The whole report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub ready: bool,
    pub checks: Vec<Check>,
    pub archive: Archive,
}

impl Report {
    /// What `probe` finds, for headless reading with browser `browser`;
    /// `x` is what the X post API answered in live mode; `None` keeps
    /// every check offline, including GitHub sign in.
    pub fn gather(
        probe: &impl Probe,
        browser: &str,
        x: Option<Result<u16, String>>,
        archive: Archive,
    ) -> Self {
        let checks = vec![
            Check::new(REQUIRED, State::Ready, "compiled in"),
            github(probe, x.is_some()),
            x_check(x),
            youtube(probe),
            headless(probe, browser),
            archive.check(),
        ];
        let ready = checks
            .iter()
            .any(|check| check.tier == REQUIRED && check.state == State::Ready);
        Self {
            ready,
            checks,
            archive,
        }
    }

    fn text(&self) -> String {
        let width = self.checks.iter().map(|c| c.tier.len()).max().unwrap_or(0);
        let mut text = String::new();
        for check in &self.checks {
            let _ = writeln!(
                text,
                "{:width$}  {:8}  {}",
                check.tier,
                check.state.word(),
                check.detail
            );
            if let Some(hint) = &check.hint {
                let _ = writeln!(text, "{:width$}  {:8}  {hint}", "", "");
            }
        }
        text
    }
}

/// `name` and its version, or where it is when it printed none.
fn named(name: &str, version: Option<String>, path: &Path) -> String {
    match version {
        Some(version) if version.to_ascii_lowercase().contains(name) => version,
        Some(version) => format!("{name} {version}"),
        None => format!("{name} at {}", path.display()),
    }
}

fn found(probe: &impl Probe, name: &str, path: &Path) -> String {
    named(name, probe.version(path), path)
}

fn github(probe: &impl Probe, live: bool) -> Check {
    let readiness = if live {
        Readiness::check(probe)
    } else {
        Readiness::assumed(probe)
    };
    let read_from_the_web = "GitHub pages are read from the web meanwhile";
    match readiness {
        Readiness::Ready(gh) => Check::new(
            "github",
            State::Ready,
            format!(
                "{}, {}",
                named("gh", gh.version, &gh.path),
                if live {
                    "signed in"
                } else {
                    "sign in not checked (run doctor --live)"
                }
            ),
        ),
        Readiness::Missing => Check::new("github", State::Missing, "gh not found").hint(format!(
            "install gh (https://cli.github.com) and run gh auth login; {read_from_the_web}"
        )),
        Readiness::NotSignedIn(gh, code) => {
            let code = code.map_or_else(|| "no exit code".to_owned(), |c| format!("exit {c}"));
            Check::new(
                "github",
                State::Degraded,
                format!(
                    "{}, not signed in (gh auth status: {code})",
                    named("gh", gh.version, &gh.path)
                ),
            )
            .hint(format!("run gh auth login; {read_from_the_web}"))
        }
    }
}

fn x_check(x: Option<Result<u16, String>>) -> Check {
    let host = xpost::API_HOST;
    match x {
        None => Check::new(
            "x",
            State::Ready,
            format!("compiled in; {host} not asked (doctor --live asks once)"),
        ),
        Some(Ok(status @ 200..=299)) => Check::new(
            "x",
            State::Ready,
            format!("compiled in; {host} answered HTTP {status}"),
        ),
        Some(Ok(status)) => Check::new(
            "x",
            State::Degraded,
            format!("compiled in; {host} answered HTTP {status}"),
        )
        .hint("X posts fail until the X post API answers again"),
        Some(Err(reason)) => Check::new(
            "x",
            State::Degraded,
            format!("compiled in; {host} not reached: {reason}"),
        )
        .hint("X posts fail until the X post API can be reached"),
    }
}

fn youtube(probe: &impl Probe) -> Check {
    let later = "the YouTube route is not built yet, so videos are read as web pages";
    let Some(ytdlp) = probe.find("yt-dlp") else {
        return Check::new(
            "youtube",
            State::Missing,
            format!("yt-dlp not found; {later}"),
        )
        .hint("install yt-dlp and deno (or node)");
    };
    let ytdlp = found(probe, "yt-dlp", &ytdlp);
    let runtimes: Vec<String> = ["deno", "node"]
        .into_iter()
        .filter_map(|name| Some(found(probe, name, &probe.find(name)?)))
        .collect();
    if runtimes.is_empty() {
        return Check::new(
            "youtube",
            State::Degraded,
            format!("{ytdlp}; no deno or node for it; {later}"),
        )
        .hint("install deno (or node): yt-dlp needs a JavaScript runtime for YouTube");
    }
    Check::new(
        "youtube",
        State::Ready,
        format!("{ytdlp}; {}; {later}", runtimes.join(", ")),
    )
}

fn headless(probe: &impl Probe, browser: &str) -> Check {
    let later = "the headless tier is not built yet";
    if platform::browser(browser).is_none() {
        return Check::new(
            "headless",
            State::Missing,
            format!("unknown browser {browser:?}; {later}"),
        )
        .hint("pass --browser with chrome, chrome-beta, chrome-canary, chromium, brave, edge or vivaldi");
    }
    match probe.browser(browser) {
        Some(path) => Check::new(
            "headless",
            State::Ready,
            format!("{}; {later}", found(probe, browser, &path)),
        ),
        None => Check::new(
            "headless",
            State::Missing,
            format!("no {browser} binary found; {later}"),
        )
        .hint(format!(
            "install {browser}, or pass --browser with one you have"
        )),
    }
}

pub fn command(
    root: &Path,
    browser: Option<&str>,
    live: bool,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let x = live.then(|| xpost::reachable(&Fetcher::new(&BTreeSet::new())));
    let report = Report::gather(
        &System,
        browser.unwrap_or(platform::CHROME),
        x,
        Archive::read(root)?,
    );
    if json {
        out::json(&serde_json::to_value(&report).unwrap_or_default());
    } else if !log.quiet {
        out::block(&report.text());
    }
    if report.ready {
        Ok(())
    } else {
        Err(Error::NotReady)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Table;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(format!("/opt/bin/{name}"))
    }

    /// Every tool found and answering.
    fn everything() -> Table {
        let mut table = Table::default();
        for (name, version) in [
            ("gh", "gh version 2.102.0 (2026-09-30)"),
            ("yt-dlp", "2026.09.01"),
            ("deno", "deno 2.5.0 (stable)"),
            ("node", "v25.0.0"),
        ] {
            table.found.insert(name, path(name));
            table.versions.insert(path(name), version.to_owned());
        }
        table.codes.insert(path("gh"), 0);
        table.browsers.insert(platform::CHROME, path("chrome"));
        table
            .versions
            .insert(path("chrome"), "Google Chrome 141.0.0.0".to_owned());
        table
    }

    fn archive() -> Archive {
        Archive {
            root: PathBuf::from("/home/someone/.knowmoretabs"),
            exists: true,
            private: Some(true),
            content_dir: true,
            statuses: BTreeMap::from([("ok", 70), ("behind_login", 1)]),
            unreadable_lines: 0,
        }
    }

    fn states(report: &Report) -> Vec<(&str, State)> {
        report.checks.iter().map(|c| (c.tier, c.state)).collect()
    }

    #[test]
    fn offline_checks_never_ask_gh_about_sign_in() {
        let mut table = everything();
        table.codes.insert(path("gh"), 1);
        let report = Report::gather(&table, platform::CHROME, None, archive());
        assert!(
            table.runs.borrow().is_empty(),
            "offline doctor must not ask gh"
        );
        assert_eq!(report.checks[1].state, State::Ready);
        assert_eq!(
            report.checks[1].detail,
            "gh version 2.102.0 (2026-09-30), sign in not checked (run doctor --live)"
        );
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["checks"][1]["detail"], report.checks[1].detail);
        assert!(
            report
                .text()
                .contains("sign in not checked (run doctor --live)")
        );
    }

    #[test]
    fn a_fully_equipped_machine_is_ready_on_every_line() {
        let report = Report::gather(&everything(), platform::CHROME, Some(Ok(200)), archive());
        assert!(report.ready);
        assert!(
            report
                .checks
                .iter()
                .all(|c| c.state == State::Ready && c.hint.is_none())
        );
        let detail = |tier: &str| {
            report
                .checks
                .iter()
                .find(|c| c.tier == tier)
                .unwrap()
                .detail
                .clone()
        };
        assert_eq!(
            detail("github"),
            "gh version 2.102.0 (2026-09-30), signed in"
        );
        assert_eq!(
            detail("x"),
            "compiled in; api.fxtwitter.com answered HTTP 200"
        );
        assert!(
            detail("youtube").starts_with("yt-dlp 2026.09.01; deno 2.5.0 (stable), node v25.0.0;")
        );
        assert!(detail("headless").starts_with("Google Chrome 141.0.0.0;"));
        assert_eq!(
            detail("archive"),
            "/home/someone/.knowmoretabs, private; pages/content present; 1 behind_login, 70 ok"
        );
    }

    #[test]
    fn missing_tools_are_warnings_with_hints_and_still_exit_zero() {
        let bare = Table::default();
        let mut no_archive = archive();
        no_archive.exists = false;
        let report = Report::gather(&bare, platform::CHROME, None, no_archive);
        assert!(report.ready, "only the generic tier is required");
        assert_eq!(
            states(&report),
            [
                ("generic", State::Ready),
                ("github", State::Missing),
                ("x", State::Ready),
                ("youtube", State::Missing),
                ("headless", State::Missing),
                ("archive", State::Missing),
            ]
        );
        assert!(
            report
                .checks
                .iter()
                .filter(|c| c.state != State::Ready)
                .all(|c| c.hint.is_some())
        );
        let text = report.text();
        assert!(
            text.contains("github    missing   gh not found\n"),
            "{text}"
        );
        assert!(text.contains("x         ready     compiled in; api.fxtwitter.com not asked"));
    }

    #[test]
    fn degraded_lines_say_what_is_wrong_and_never_show_gh_output() {
        let mut table = everything();
        table.codes.insert(path("gh"), 1);
        table.found.remove("deno");
        table.found.remove("node");
        let mut open = archive();
        open.private = Some(false);
        let report = Report::gather(&table, platform::CHROME, Some(Ok(503)), open);
        assert!(report.ready);
        assert_eq!(
            states(&report),
            [
                ("generic", State::Ready),
                ("github", State::Degraded),
                ("x", State::Degraded),
                ("youtube", State::Degraded),
                ("headless", State::Ready),
                ("archive", State::Degraded),
            ]
        );
        assert_eq!(
            report.checks[1].detail,
            "gh version 2.102.0 (2026-09-30), not signed in (gh auth status: exit 1)"
        );
        assert_eq!(
            table.runs.borrow().as_slice(),
            [(
                "auth status --hostname github.com".to_owned(),
                "GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 NO_COLOR=1".to_owned()
            )],
            "gh is asked for its exit code once, and only that"
        );
        let unreachable = Report::gather(
            &table,
            platform::CHROME,
            Some(Err("timeout".into())),
            archive(),
        );
        assert_eq!(
            unreachable.checks[2].detail,
            "compiled in; api.fxtwitter.com not reached: timeout"
        );
    }

    #[test]
    fn headless_follows_the_chosen_browser() {
        let report = Report::gather(&everything(), platform::BRAVE, None, archive());
        assert_eq!(report.checks[4].state, State::Missing);
        assert!(report.checks[4].detail.starts_with("no brave binary found"));
        let unknown = Report::gather(&everything(), "arc", None, archive());
        assert!(
            unknown.checks[4]
                .detail
                .starts_with("unknown browser \"arc\"")
        );
    }

    #[test]
    fn the_json_report_has_a_line_per_tier_and_the_archive() {
        let report = Report::gather(&Table::default(), platform::CHROME, None, archive());
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["ready"], true);
        assert_eq!(value["checks"][1]["tier"], "github");
        assert_eq!(value["checks"][1]["state"], "missing");
        assert!(value["checks"][0].get("hint").is_none());
        assert_eq!(value["archive"]["statuses"]["ok"], 70);
        assert_eq!(value["archive"]["private"], true);
    }

    #[test]
    fn an_archive_is_read_from_its_content_log() {
        let root = tempfile::tempdir().unwrap();
        let missing = Archive::read(&root.path().join("absent")).unwrap();
        assert!(!missing.exists && !missing.content_dir && missing.statuses.is_empty());
        let mut store = content_store::Store::open(root.path()).unwrap();
        store
            .record(
                content_store::Line::new("https://a.test/", content_store::Status::NotFound),
                None,
            )
            .unwrap();
        let archive = Archive::read(root.path()).unwrap();
        assert!(archive.exists && archive.content_dir);
        assert_eq!(archive.statuses, BTreeMap::from([("not_found", 1)]));
    }
}
