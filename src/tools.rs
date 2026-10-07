//! The external programs content capture may use: finding them, asking
//! their version, and running them within bounds.
//!
//! slice: content
//! why: Some documents are best read by a tool the owner already trusts:
//!      `gh` for GitHub, yt-dlp for videos, and later the installed browser
//!      for pages that need scripts. Every such run is the same promise: an
//!      argument vector and never a shell, no input, a fixed timeout after
//!      which the program is killed, a cap on what is kept of its output,
//!      and its complaints cut to one line of reason. A missing tool is not
//!      an error: the tier that needs it steps aside. What a command learns
//!      about the machine goes through [`Probe`], so `doctor`'s report can
//!      be tested from a table instead of the machine it runs on.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::platform::{self, Os};

/// How long a `--version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a readiness check such as `gh auth status` may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// What is kept of a program's complaints before cutting them to a line.
const STDERR_CAP: usize = 64 * 1024;
/// The longest reason kept from a program's complaints.
const REASON_CHARS: usize = 160;
/// How often a running program is checked against its deadline.
const POLL: Duration = Duration::from_millis(20);
/// How long output is waited for after a program exits, in case something
/// it started still holds the pipe.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// The bounds of one run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    /// The most of standard output kept; the rest is read and dropped.
    pub stdout_cap: usize,
}

/// What a run that finished in time produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Standard output was longer than the cap.
    pub overflowed: bool,
    /// Its standard error as one line of reason, or empty.
    pub complaint: String,
}

/// Why a run produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// It could not be started.
    Spawn(String),
    /// It was killed at its deadline.
    TimedOut(Duration),
}

impl Failure {
    pub fn reason(&self, name: &str) -> String {
        match self {
            Self::Spawn(why) => format!("{name} could not start: {why}"),
            Self::TimedOut(after) => format!("{name} timed out after {} s", after.as_secs()),
        }
    }
}

/// What a command asks of the machine about its tools. [`System`] asks the
/// machine; tests answer from a table.
pub trait Probe {
    /// `name` on `PATH`.
    fn find(&self, name: &str) -> Option<PathBuf>;
    /// The binary of browser `id` from the platform's browser table.
    fn browser(&self, id: &str) -> Option<PathBuf>;
    /// The first line `program --version` prints.
    fn version(&self, program: &Path) -> Option<String>;
    /// The exit code of a bounded run whose output goes nowhere, unread.
    fn exit_code(&self, program: &Path, args: &[&str], env: &[(&str, &str)]) -> Option<i32>;
}

/// The machine this runs on.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Probe for System {
    fn find(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        find_in(&path, name)
    }

    fn browser(&self, id: &str) -> Option<PathBuf> {
        let roots = platform::Roots::detect()?;
        let bases = Bases {
            home: roots.home.clone(),
            program_files: [
                std::env::var_os("ProgramFiles"),
                std::env::var_os("ProgramFiles(x86)"),
            ]
            .into_iter()
            .flatten()
            .map(PathBuf::from)
            .collect(),
            local_app_data: roots.local_app_data.clone(),
        };
        browser_candidates(id, Os::HOST, &bases)
            .into_iter()
            .find(|path| is_executable(path))
            .or_else(|| {
                browser_names(id, Os::HOST)
                    .iter()
                    .find_map(|name| self.find(name))
            })
    }

    fn version(&self, program: &Path) -> Option<String> {
        let output = run(
            program,
            &["--version"],
            &[],
            Limits {
                timeout: VERSION_TIMEOUT,
                stdout_cap: STDERR_CAP,
            },
        )
        .ok()?;
        first_line(&String::from_utf8_lossy(&output.stdout))
    }

    fn exit_code(&self, program: &Path, args: &[&str], env: &[(&str, &str)]) -> Option<i32> {
        let child = command(program, args, env)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        wait(child, CHECK_TIMEOUT).ok()?.code()
    }
}

/// `name` in one of `path`'s directories, as a file this user may run.
fn find_in(path: &OsStr, name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Runs `program` with `args` and `env` added to this process's
/// environment: no shell, no input, killed at `limits.timeout`.
pub fn run(
    program: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    limits: Limits,
) -> Result<Output, Failure> {
    let mut child = command(program, args, env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| Failure::Spawn(err.kind().to_string()))?;
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let _ = out_tx.send(read_capped(stdout, limits.stdout_cap));
        });
    }
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let _ = err_tx.send(read_capped(stderr, STDERR_CAP));
        });
    }
    let status = wait(child, limits.timeout)?;
    let (stdout, overflowed) = out_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    let (stderr, _) = err_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    Ok(Output {
        code: status.code(),
        stdout,
        overflowed,
        complaint: complaint(&String::from_utf8_lossy(&stderr)),
    })
}

/// `program` with `args`, `env` added to this process's environment, and
/// no input.
fn command(program: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null());
    command
}

/// How `child` exited, or killed at `timeout`.
fn wait(mut child: Child, timeout: Duration) -> Result<ExitStatus, Failure> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::TimedOut(timeout));
            }
        }
    }
}

/// Up to `cap` bytes of `reader`, then the rest read and dropped so the
/// writer never blocks; and whether there was more.
fn read_capped(mut reader: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return (kept, overflowed),
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..n.min(room)]);
                overflowed |= n > room;
            }
        }
    }
}

/// The first line of `text` that says something, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// A program's standard error as one line of reason: its first `ERROR:`
/// line, since a tool such as yt-dlp warns before it fails, else its first
/// line that says something; cut to [`REASON_CHARS`] characters.
pub fn complaint(stderr: &str) -> String {
    let error = stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("ERROR:"))
        .map(str::to_owned);
    let Some(line) = error.or_else(|| first_line(stderr)) else {
        return String::new();
    };
    let mut cut: String = line.chars().take(REASON_CHARS).collect();
    if cut.len() < line.len() {
        cut.push('…');
    }
    cut
}

/// The directories a browser's binary is found under.
#[derive(Debug, Clone, Default)]
struct Bases {
    home: PathBuf,
    /// `%ProgramFiles%` and `%ProgramFiles(x86)%`.
    program_files: Vec<PathBuf>,
    local_app_data: PathBuf,
}

/// Where browser `id`'s binary is installed on `os`, most likely first:
/// the application bundle on macOS, the install directories on Windows.
/// Linux installs put a launcher on `PATH`; see [`browser_names`].
fn browser_candidates(id: &str, os: Os, bases: &Bases) -> Vec<PathBuf> {
    match os {
        Os::Mac => {
            let Some(app) = mac_app(id) else {
                return Vec::new();
            };
            [
                PathBuf::from("/Applications"),
                bases.home.join("Applications"),
            ]
            .into_iter()
            .map(|dir| {
                dir.join(format!("{app}.app"))
                    .join("Contents")
                    .join("MacOS")
                    .join(app)
            })
            .collect()
        }
        Os::Windows => {
            let Some(names) = windows_install(id) else {
                return Vec::new();
            };
            bases
                .program_files
                .iter()
                .chain(std::iter::once(&bases.local_app_data))
                .map(|base| {
                    names
                        .iter()
                        .fold(base.clone(), |path, name| path.join(name))
                })
                .collect()
        }
        Os::Linux => Vec::new(),
    }
}

/// The application bundle's name, which is also its binary's.
fn mac_app(id: &str) -> Option<&'static str> {
    Some(match id {
        platform::CHROME => "Google Chrome",
        platform::CHROME_BETA => "Google Chrome Beta",
        platform::CHROME_CANARY => "Google Chrome Canary",
        platform::CHROMIUM => "Chromium",
        platform::BRAVE => "Brave Browser",
        platform::EDGE => "Microsoft Edge",
        platform::VIVALDI => "Vivaldi",
        _ => return None,
    })
}

/// The binary under an install base on Windows.
fn windows_install(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        platform::CHROME => &["Google", "Chrome", "Application", "chrome.exe"],
        platform::CHROME_BETA => &["Google", "Chrome Beta", "Application", "chrome.exe"],
        platform::CHROME_CANARY => &["Google", "Chrome SxS", "Application", "chrome.exe"],
        platform::CHROMIUM => &["Chromium", "Application", "chrome.exe"],
        platform::BRAVE => &["BraveSoftware", "Brave-Browser", "Application", "brave.exe"],
        platform::EDGE => &["Microsoft", "Edge", "Application", "msedge.exe"],
        platform::VIVALDI => &["Vivaldi", "Application", "vivaldi.exe"],
        _ => return None,
    })
}

/// The launcher names a browser puts on `PATH` on Linux.
fn browser_names(id: &str, os: Os) -> &'static [&'static str] {
    if os != Os::Linux {
        return &[];
    }
    match id {
        platform::CHROME => &["google-chrome", "google-chrome-stable"],
        platform::CHROME_BETA => &["google-chrome-beta"],
        platform::CHROME_CANARY => &["google-chrome-canary", "google-chrome-unstable"],
        platform::CHROMIUM => &["chromium", "chromium-browser"],
        platform::BRAVE => &["brave-browser", "brave"],
        platform::EDGE => &["microsoft-edge", "microsoft-edge-stable"],
        platform::VIVALDI => &["vivaldi", "vivaldi-stable"],
        _ => &[],
    }
}

/// Tools answered from a table, for tests: what is found, the version each
/// prints and the exit code each returns; every run is remembered as its
/// arguments and environment, joined.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Table {
    pub found: std::collections::HashMap<&'static str, PathBuf>,
    pub browsers: std::collections::HashMap<&'static str, PathBuf>,
    pub versions: std::collections::HashMap<PathBuf, String>,
    pub codes: std::collections::HashMap<PathBuf, i32>,
    pub runs: std::cell::RefCell<Vec<(String, String)>>,
}

#[cfg(test)]
impl Probe for Table {
    fn find(&self, name: &str) -> Option<PathBuf> {
        self.found.get(name).cloned()
    }

    fn browser(&self, id: &str) -> Option<PathBuf> {
        self.browsers.get(id).cloned()
    }

    fn version(&self, program: &Path) -> Option<String> {
        self.versions.get(program).cloned()
    }

    fn exit_code(&self, program: &Path, args: &[&str], env: &[(&str, &str)]) -> Option<i32> {
        let env = env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>();
        self.runs.borrow_mut().push((args.join(" "), env.join(" ")));
        self.codes.get(program).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_browser_has_a_binary_on_every_platform() {
        let bases = Bases {
            home: PathBuf::from("/home/someone"),
            program_files: vec![PathBuf::from("C:\\Program Files")],
            local_app_data: PathBuf::from("C:\\Users\\someone\\AppData\\Local"),
        };
        for spec in &platform::BROWSERS {
            assert_eq!(
                browser_candidates(spec.id, Os::Mac, &bases).len(),
                2,
                "{}",
                spec.id
            );
            assert_eq!(
                browser_candidates(spec.id, Os::Windows, &bases).len(),
                2,
                "{}",
                spec.id
            );
            assert!(!browser_names(spec.id, Os::Linux).is_empty(), "{}", spec.id);
        }
        assert_eq!(
            browser_candidates(platform::CHROME, Os::Mac, &bases)[0],
            Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        );
        assert_eq!(
            browser_candidates("arc", Os::Mac, &bases),
            Vec::<PathBuf>::new()
        );
        assert_eq!(browser_names(platform::CHROME, Os::Mac), [] as [&str; 0]);
    }

    #[test]
    fn a_complaint_is_its_first_line_cut_short() {
        assert_eq!(complaint(""), "");
        assert_eq!(
            complaint("\n  gh: Not Found (HTTP 404)  \nmore detail\n"),
            "gh: Not Found (HTTP 404)"
        );
        assert_eq!(
            complaint("WARNING: a hint\nERROR: the failure\nERROR: a second\n"),
            "ERROR: the failure"
        );
        assert_eq!(complaint("WARNING: only a hint\n"), "WARNING: only a hint");
        let long = "é".repeat(REASON_CHARS + 5);
        let cut = complaint(&long);
        assert_eq!(cut.chars().count(), REASON_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn output_past_the_cap_is_read_and_dropped() {
        assert_eq!(read_capped(&b"abcdef"[..], 4), (b"abcd".to_vec(), true));
        assert_eq!(read_capped(&b"abc"[..], 4), (b"abc".to_vec(), false));
        assert_eq!(read_capped(&b"abc"[..], 0), (Vec::new(), true));
    }

    #[cfg(unix)]
    #[test]
    fn only_files_this_user_may_run_are_found_on_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        std::fs::write(&plain, b"").unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let runnable = dir.path().join("runnable");
        std::fs::write(&runnable, b"").unwrap();
        std::fs::set_permissions(&runnable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([Path::new("relative"), dir.path()]).unwrap();
        assert_eq!(find_in(&path, "runnable"), Some(runnable));
        assert_eq!(find_in(&path, "plain"), None);
        assert_eq!(find_in(&path, "folder"), None);
        assert_eq!(find_in(&path, "missing"), None);
    }
}
