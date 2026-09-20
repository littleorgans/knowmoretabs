//! Shared helpers for the integration tests: a fake home directory with a
//! Chrome-shaped user-data tree, and a way to run the real binary against it.

#![allow(dead_code)]

pub mod session_builder;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub use session_builder::SessionBuilder;

/// Where `platform::BrowserSpec` for Chrome looks, relative to home, on the
/// platform the tests are running on. Kept in step by `default_discovery`.
pub fn chrome_relative_path() -> &'static str {
    if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else if cfg!(windows) {
        "AppData/Local/Google/Chrome/User Data"
    } else {
        ".config/google-chrome"
    }
}

pub struct Fixture {
    pub home: TempDir,
    pub user_data: PathBuf,
    pub root: PathBuf,
}

impl Fixture {
    /// A home with Chrome's user-data dir, a `Local State` naming `Default`,
    /// and an archive root beside it. No session file yet.
    pub fn new() -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let user_data = home.path().join(chrome_relative_path());
        std::fs::create_dir_all(&user_data).expect("user data dir");
        let fixture = Self {
            root: home.path().join("archive"),
            home,
            user_data,
        };
        fixture.write_local_state(r#"{"profile": {"last_used": "Default", "info_cache": {"Default": {"name": "Person 1"}}}}"#);
        fixture
    }

    pub fn write_local_state(&self, json: &str) {
        std::fs::write(self.user_data.join("Local State"), json).expect("Local State");
    }

    pub fn profile_dir(&self, profile: &str) -> PathBuf {
        self.user_data.join(profile)
    }

    pub fn sessions_dir(&self, profile: &str) -> PathBuf {
        self.profile_dir(profile).join("Sessions")
    }

    /// Writes `Sessions/Session_<suffix>` for the profile and returns its path.
    pub fn write_session(&self, profile: &str, suffix: i64, bytes: &[u8]) -> PathBuf {
        let dir = self.sessions_dir(profile);
        std::fs::create_dir_all(&dir).expect("sessions dir");
        let path = dir.join(format!("Session_{suffix}"));
        std::fs::write(&path, bytes).expect("session file");
        path
    }

    /// The binary with HOME pointed at the fixture and `--root` set. The
    /// user-data dir is *not* passed, so default discovery is what runs
    /// unless the caller adds `--user-data-dir` or `--session`.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_knowmoretabs"));
        cmd.env_clear();
        cmd.env("HOME", self.home.path());
        cmd.env("USERPROFILE", self.home.path());
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.arg("--root").arg(&self.root);
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .output()
            .expect("run knowmoretabs")
    }

    /// Published snapshot directories, oldest first.
    pub fn snapshot_dirs(&self) -> Vec<PathBuf> {
        let dir = self.root.join("snapshots");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .map(|e| e.expect("entry").path())
            .filter(|p| p.is_dir() && !p.file_name().unwrap().to_string_lossy().starts_with('.'))
            .collect();
        dirs.sort();
        dirs
    }

    pub fn staging_dirs(&self) -> Vec<PathBuf> {
        let dir = self.root.join("snapshots");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        entries
            .map(|e| e.expect("entry").path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".staging-")
            })
            .collect()
    }
}

pub fn read_snapshot(dir: &Path) -> serde_json::Value {
    let bytes = std::fs::read(dir.join("snapshot.json")).expect("snapshot.json");
    serde_json::from_slice(&bytes).expect("valid json")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success\nstdout: {}\nstderr: {}",
        stdout(output),
        stderr(output)
    );
}

/// One window, two tabs: the smallest session worth saving.
pub fn two_tab_session() -> Vec<u8> {
    SessionBuilder::new()
        .simple_tab(1, 2, "https://example.test/one", "One")
        .simple_tab(1, 3, "https://example.test/two", "Two")
        .selected_tab(1, 1)
        .marker()
        .build()
}
