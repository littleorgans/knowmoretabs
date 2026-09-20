//! Synthetic browser trees for slice 4. No test reads a real browser profile.

mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

use common::{read_snapshot, stderr, stdout, two_tab_session};
use tempfile::TempDir;

fn relative_path(browser: &str) -> &'static str {
    if cfg!(target_os = "macos") {
        match browser {
            "chrome" => "Library/Application Support/Google/Chrome",
            "chrome-beta" => "Library/Application Support/Google/Chrome Beta",
            "chrome-canary" => "Library/Application Support/Google/Chrome Canary",
            "chromium" => "Library/Application Support/Chromium",
            "brave" => "Library/Application Support/BraveSoftware/Brave-Browser",
            "edge" => "Library/Application Support/Microsoft Edge",
            "vivaldi" => "Library/Application Support/Vivaldi",
            _ => panic!("unknown synthetic browser"),
        }
    } else if cfg!(windows) {
        match browser {
            "chrome" => "AppData/Local/Google/Chrome/User Data",
            "chrome-beta" => "AppData/Local/Google/Chrome Beta/User Data",
            "chrome-canary" => "AppData/Local/Google/Chrome SxS/User Data",
            "chromium" => "AppData/Local/Chromium/User Data",
            "brave" => "AppData/Local/BraveSoftware/Brave-Browser/User Data",
            "edge" => "AppData/Local/Microsoft/Edge/User Data",
            "vivaldi" => "AppData/Local/Vivaldi/User Data",
            _ => panic!("unknown synthetic browser"),
        }
    } else {
        match browser {
            "chrome" => ".config/google-chrome",
            "chrome-beta" => ".config/google-chrome-beta",
            "chrome-canary" => ".config/google-chrome-canary",
            "chromium" => ".config/chromium",
            "brave" => ".config/BraveSoftware/Brave-Browser",
            "edge" => ".config/microsoft-edge",
            "vivaldi" => ".config/vivaldi",
            _ => panic!("unknown synthetic browser"),
        }
    }
}

struct Tree {
    home: TempDir,
    root: PathBuf,
}

impl Tree {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        Self {
            root: home.path().join("archive"),
            home,
        }
    }

    fn user_data(&self, browser: &str) -> PathBuf {
        self.home.path().join(relative_path(browser))
    }

    fn browser(&self, browser: &str, profile: &str, display: &str, suffix: i64) {
        let user_data = self.user_data(browser);
        let profile_path = user_data.join(profile);
        let sessions = profile_path.join("Sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            user_data.join("Local State"),
            serde_json::json!({
                "profile": {
                    "last_used": profile,
                    "info_cache": {profile: {"name": display}}
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            sessions.join(format!("Session_{suffix}")),
            two_tab_session(),
        )
        .unwrap();
    }

    fn empty_browser(&self, browser: &str, with_profile: bool, malformed_state: bool) {
        let user_data = self.user_data(browser);
        std::fs::create_dir_all(&user_data).unwrap();
        if with_profile {
            std::fs::create_dir_all(user_data.join("Default").join("Sessions")).unwrap();
        }
        let state = if malformed_state {
            "{".to_owned()
        } else if with_profile {
            r#"{"profile":{"last_used":"Default","info_cache":{"Default":{"name":"Person 1"}}}}"#
                .to_owned()
        } else {
            r#"{"profile":{"info_cache":{}}}"#.to_owned()
        };
        std::fs::write(user_data.join("Local State"), state).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_knowmoretabs"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap())
            .arg("--root")
            .arg(&self.root)
            .args(args);
        command.output().unwrap()
    }

    fn snapshot_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<_> = std::fs::read_dir(self.root.join("snapshots"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs
    }
}

#[test]
fn zero_flags_choose_the_newest_supported_browser_and_report_the_rest() {
    let tree = Tree::new();
    tree.browser("chrome", "Default", "Person 1", 10);
    tree.browser("brave", "Default", "Work", 20);
    tree.browser("edge", "Default", "Person 1", 30);

    let output = tree.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let snapshot = read_snapshot(&tree.snapshot_dirs()[0]);
    assert_eq!(snapshot["source"]["browser"], "edge");
    let out = stdout(&output);
    assert!(out.contains("also found:"), "{out}");
    assert!(out.contains("--browser brave"), "{out}");
    assert!(out.contains("--browser chrome"), "{out}");
}

#[test]
fn zero_flag_scan_skips_empty_browsers_profiles_and_state_files() {
    let tree = Tree::new();
    tree.empty_browser("chrome-beta", false, false);
    tree.empty_browser("chromium", true, false);
    tree.empty_browser("chrome-canary", false, true);
    tree.browser("brave", "Default", "Work", 20);

    let output = tree.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
        "brave"
    );
    assert!(!stdout(&output).contains("also found:"));
}

#[test]
fn explicit_browser_isolated_and_unknown_names_explain_themselves() {
    let tree = Tree::new();
    tree.browser("brave", "Default", "Work", 20);

    let output = tree.run(&["--browser", "brave"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
        "brave"
    );

    let output = tree.run(&["--browser", "unknown"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("brave"), "{}", stderr(&output));

    let output = tree.run(&["--browser", "arc"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("StorableSidebar.json"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn display_names_are_resolved_without_becoming_paths() {
    let tree = Tree::new();
    let user_data = tree.user_data("vivaldi");
    let sessions = user_data.join("Profile 1").join("Sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        user_data.join("Local State"),
        r#"{"profile":{"info_cache":{"Profile 1":{"name":"研究 🐙"}}}}"#,
    )
    .unwrap();
    std::fs::write(sessions.join("Session_50"), two_tab_session()).unwrap();

    let output = tree.run(&["--browser", "vivaldi", "--profile", "研究 🐙"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let snapshot = read_snapshot(&tree.snapshot_dirs()[0]);
    assert_eq!(snapshot["source"]["profile"], "Profile 1");
    assert_eq!(snapshot["source"]["profile_display"], "研究 🐙");

    for bad in ["../Profile 1", "Guest", "System", "Default:secret"] {
        let output = tree.run(&["--browser", "vivaldi", "--profile", bad]);
        assert!(!output.status.success(), "{bad}");
    }
}

#[test]
fn alternating_browsers_do_not_mask_the_unchanged_session_skip() {
    let tree = Tree::new();
    tree.browser("chrome", "Default", "Person 1", 100);
    tree.browser("brave", "Default", "Work", 90);

    assert!(tree.run(&["--browser", "chrome"]).status.success());
    let brave = tree.run(&["--browser", "brave"]);
    assert!(brave.status.success(), "{}", stderr(&brave));
    assert_eq!(tree.snapshot_dirs().len(), 2);

    let skipped = tree.run(&["--browser", "brave"]);
    assert!(skipped.status.success(), "{}", stderr(&skipped));
    assert!(stdout(&skipped).contains("no change since"));
    assert_eq!(tree.snapshot_dirs().len(), 2);
}

#[test]
fn an_empty_profile_reports_no_session_without_creating_the_archive() {
    let tree = Tree::new();
    let user_data = tree.user_data("chromium");
    std::fs::create_dir_all(user_data.join("Default").join("Sessions")).unwrap();
    std::fs::write(
        user_data.join("Local State"),
        r#"{"profile":{"last_used":"Default","info_cache":{"Default":{"name":"Person 1"}}}}"#,
    )
    .unwrap();
    let output = tree.run(&["--browser", "chromium"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("Session_*"), "{}", stderr(&output));
    assert!(!tree.root.exists());
}
