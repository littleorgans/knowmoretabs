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
        common::point_home_at(&mut command, self.home.path());
        command.arg("--root").arg(&self.root).args(args);
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

fn set_modified(path: &std::path::Path, at: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

impl Tree {
    /// Writes `Sessions_Encrypted/Session_<suffix>` dated an hour after
    /// everything in `Sessions/`, which is what a profile Chrome has migrated
    /// looks like: the cleartext files stopped moving.
    fn encrypted(&self, browser: &str, profile: &str, suffix: i64) {
        let profile_path = self.user_data(browser).join(profile);
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for entry in std::fs::read_dir(profile_path.join("Sessions")).unwrap() {
            set_modified(&entry.unwrap().path(), old);
        }
        let encrypted = profile_path.join("Sessions_Encrypted");
        std::fs::create_dir_all(&encrypted).unwrap();
        let path = encrypted.join(format!("Session_{suffix}"));
        std::fs::write(&path, b"SNSS").unwrap();
        set_modified(&path, old + std::time::Duration::from_secs(3_600));
    }
}

#[test]
fn a_stale_browser_is_ranked_by_its_encrypted_sessions_and_never_saved() {
    let tree = Tree::new();
    tree.browser("chrome", "Default", "Person 1", 10);
    tree.encrypted("chrome", "Default", 11);
    tree.browser("brave", "Default", "Work", 20);

    // Brave was used after Chrome's encrypted log moved on: Brave is saved,
    // and Chrome is reported as found and stale rather than merely "older".
    let output = tree.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
        "brave"
    );
    let out = stdout(&output);
    assert!(out.contains("--browser chrome"), "{out}");
    assert!(out.contains("would refuse"), "{out}");
    let output = tree.run(&["--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["also_found"][0]["browser"], "chrome");
    assert_eq!(value["also_found"][0]["stale"], true);

    // Chrome's encrypted log is now the newest thing on the machine: Chrome
    // is the browser in use, and the scan refuses instead of saving Brave.
    tree.encrypted("chrome", "Default", 30);
    let output = tree.run(&["--force"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(tree.snapshot_dirs().len(), 1);
    assert_eq!(tree.run(&["--browser", "chrome"]).status.code(), Some(3));
    let output = tree.run(&["--browser", "brave", "--force"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn a_migrated_profile_with_no_cleartext_left_does_not_stop_the_scan_unless_newest() {
    let tree = Tree::new();
    tree.browser("chrome", "Default", "Person 1", 10);
    std::fs::remove_file(
        tree.user_data("chrome")
            .join("Default")
            .join("Sessions")
            .join("Session_10"),
    )
    .unwrap();
    tree.encrypted("chrome", "Default", 11);
    tree.browser("brave", "Default", "Work", 20);

    let output = tree.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
        "brave"
    );
    assert!(stdout(&output).contains("would refuse"));

    tree.encrypted("chrome", "Default", 30);
    assert_eq!(tree.run(&["--force"]).status.code(), Some(3));
    assert_eq!(tree.run(&["--browser", "chrome"]).status.code(), Some(3));
}

#[test]
fn a_broken_profile_entry_does_not_hide_the_profile_with_the_session() {
    let tree = Tree::new();
    tree.browser("brave", "Default", "Work", 20);
    let user_data = tree.user_data("chrome");
    std::fs::create_dir_all(user_data.join("Default").join("Sessions")).unwrap();
    std::fs::write(user_data.join("Profile 0"), b"not a directory").unwrap();
    let good = user_data.join("Profile 1").join("Sessions");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::write(good.join("Session_99"), two_tab_session()).unwrap();
    std::fs::write(
        user_data.join("Local State"),
        r#"{"profile":{"last_used":"Default","info_cache":{
            "Default":{"name":"Person 1"},"Profile 0":{"name":"Broken"},"Profile 1":{"name":"Work"}}}}"#,
    )
    .unwrap();

    for args in [&[][..], &["--browser", "chrome", "--force"][..]] {
        let output = tree.run(args);
        assert!(output.status.success(), "{:?}: {}", args, stderr(&output));
        let snapshot = read_snapshot(tree.snapshot_dirs().last().unwrap());
        assert_eq!(snapshot["source"]["browser"], "chrome");
        assert_eq!(snapshot["source"]["profile"], "Profile 1");
    }
}

#[test]
fn display_names_are_matched_as_text_before_the_directory_guard() {
    let tree = Tree::new();
    let user_data = tree.user_data("edge");
    for (dir, suffix) in [("Profile 1", 10), ("Profile 3", 20)] {
        let sessions = user_data.join(dir).join("Sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("Session_{suffix}")),
            two_tab_session(),
        )
        .unwrap();
    }
    std::fs::write(
        user_data.join("Local State"),
        r#"{"profile":{"info_cache":{"Profile 1":{"name":"Profile 2"},"Profile 3":{"name":"Work/Home"}}}}"#,
    )
    .unwrap();

    // Shaped like a directory Chromium creates, but it is Profile 1's display name.
    let output = tree.run(&["--browser", "edge", "--profile", "Profile 2"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["profile"],
        "Profile 1"
    );
    // A separator in a display name is text, never a path component.
    let output = tree.run(&["--browser", "edge", "--profile", "Work/Home"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[1])["source"]["profile"],
        "Profile 3"
    );
    // The same characters with no display name behind them are refused, in both modes.
    for args in [
        &["--browser", "edge", "--profile", "../Profile 1"][..],
        &["--profile", "Work/Elsewhere"][..],
    ] {
        let output = tree.run(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(&output).contains("not a Chrome profile directory name"),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(tree.snapshot_dirs().len(), 2);
}

#[test]
fn a_display_name_ambiguous_in_one_browser_is_warned_about_not_fatal() {
    let tree = Tree::new();
    tree.browser("chrome", "Profile 1", "Work", 40);
    let brave = tree.user_data("brave");
    let sessions = brave.join("Profile 1").join("Sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("Session_10"), two_tab_session()).unwrap();
    std::fs::write(
        brave.join("Local State"),
        r#"{"profile":{"info_cache":{"Profile 1":{"name":"Work"},"Profile 2":{"name":"Work"}}}}"#,
    )
    .unwrap();

    let output = tree.run(&["--profile", "Work"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
        "chrome"
    );
    let err = stderr(&output);
    assert!(
        err.contains("brave") && err.contains("--browser brave"),
        "{err}"
    );

    let output = tree.run(&["--profile", "Guest"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not a browsing profile"));
}

#[test]
fn an_absent_browser_asked_for_by_name_says_so_and_a_bare_machine_names_every_directory() {
    let tree = Tree::new();
    let output = tree.run(&[]);
    assert!(!output.status.success());
    let err = stderr(&output);
    for browser in [
        "chrome",
        "chrome-beta",
        "chrome-canary",
        "chromium",
        "brave",
        "edge",
        "vivaldi",
    ] {
        assert!(err.contains(&format!("{browser} (")), "{err}");
    }
    assert!(err.contains("no user-data directory"), "{err}");

    tree.browser("edge", "Default", "Person 1", 5);
    let output = tree.run(&["--browser", "brave"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("brave has no user-data directory at"), "{err}");
    assert!(err.contains("installed supported browsers: edge"), "{err}");
    assert!(!tree.root.exists());
}

#[test]
fn also_found_columns_line_up() {
    let tree = Tree::new();
    tree.browser("chrome", "Default", "Person 1", 40_000_000);
    tree.browser("brave", "Profile 12", "W", 39_000_000);
    tree.browser("vivaldi", "Default", "A much longer display name", 10);

    let output = tree.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    let rows: Vec<&str> = out.lines().filter(|l| l.contains("--browser")).collect();
    assert_eq!(rows.len(), 2, "{out}");
    let column = |row: &str, marker: &str| {
        row.find(marker)
            .unwrap_or_else(|| panic!("{marker:?} missing from {row:?}"))
    };
    for marker in [" / ", " older", "— --browser"] {
        assert_eq!(column(rows[0], marker), column(rows[1], marker), "{out}");
    }
}
