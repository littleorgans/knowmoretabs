//! What differs between macOS, Linux and Windows, through the real binary.
//!
//! The browser table itself is covered by unit tests, which can ask for any
//! platform's column from any machine. These cannot: they run the binary, so
//! they see the host's answer. Where a guarantee genuinely differs by
//! platform each arm asserts its own — none of them opts out of asserting.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{
    Fixture, assert_private_dir, assert_success, default_root_relative_path, point_home_at,
    read_snapshot, stderr, stdout, two_tab_session,
};
use tempfile::TempDir;

/// Windows' classic limit. Nothing here asserts a failure at it — the point
/// is that the standard library reaches past it — but a root chosen to sit
/// just under it is what puts the files the archive writes over it.
const CLASSIC_MAX_PATH: usize = 260;

#[test]
fn no_root_flag_puts_the_archive_where_the_platform_keeps_per_user_data() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());

    let output = fx
        .command_without_root()
        .arg("--json")
        .output()
        .expect("run knowmoretabs");
    assert_success(&output);

    let expected = default_root_relative_path()
        .split('/')
        .fold(fx.home.path().to_path_buf(), |path, part| path.join(part));
    assert!(
        expected.join("snapshots").is_dir(),
        "expected the archive at {}; home holds {:?}",
        expected.display(),
        std::fs::read_dir(fx.home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let saved = Path::new(value["saved"]["path"].as_str().unwrap());
    assert!(saved.starts_with(&expected), "{}", saved.display());
    // Where it goes and how private it is are the same decision: the Windows
    // default sits under `%LOCALAPPDATA%` precisely because inheritance is
    // the only protection available there.
    assert_private_dir(&expected);
    // Not the Unix dotfile on Windows, and not the Windows AppData tree
    // anywhere else: each platform gets one answer, not both.
    assert!(
        !fx.home.path().join(".knowmoretabs").exists() || !cfg!(windows),
        "a Unix dotfile archive was created on Windows"
    );
}

/// A root long enough that the files under it pass 260 characters. Rust's
/// standard library rewrites an absolute path past 248 characters into
/// `\\?\` form before it reaches the Windows API, so this is expected to
/// work rather than to fail — which is the finding, and a test is the only
/// honest way to hold it.
#[test]
fn a_root_near_the_classic_windows_limit_still_saves() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());

    let mut root = fx.home.path().join("deep");
    while root.as_os_str().len() < CLASSIC_MAX_PATH - 20 {
        root = root.join("p".repeat(40));
    }
    let snapshot_json_length =
        root.as_os_str().len() + r"\snapshots\2026-09-20-084415Z\snapshot.json".len();
    assert!(
        snapshot_json_length > CLASSIC_MAX_PATH,
        "the root was not long enough to be interesting: {snapshot_json_length}"
    );

    let output = fx
        .command_without_root()
        .arg("--root")
        .arg(&root)
        .output()
        .expect("run knowmoretabs");
    assert_success(&output);
    let published: Vec<PathBuf> = std::fs::read_dir(root.join("snapshots"))
        .expect("snapshots directory")
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(published.len(), 1);
    assert_eq!(read_snapshot(&published[0])["stats"]["tabs"], 2);
}

/// `CON`, `NUL` and a trailing dot are illegal on Windows and ordinary
/// directory names everywhere else, so the two platforms owe different
/// answers and both are asserted. The Windows one has to name the problem:
/// `CreateFile` on `CON` opens the console, and a run that fell through to
/// that would fail somewhere unrecognisable.
#[test]
fn a_root_windows_cannot_represent_is_refused_by_name_and_accepted_elsewhere() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());

    for (name, expected) in [
        ("CON", "reserved Windows device name"),
        ("NUL", "reserved Windows device name"),
        ("LPT1", "reserved Windows device name"),
        ("archive.", "ends with a dot or a space"),
    ] {
        let root = fx.home.path().join(name);
        let output = fx
            .command_without_root()
            .arg("--root")
            .arg(&root)
            .arg("--force")
            .output()
            .expect("run knowmoretabs");
        if cfg!(windows) {
            assert!(!output.status.success(), "{name} was accepted on Windows");
            let err = stderr(&output);
            assert!(err.contains("cannot use"), "{name}: {err}");
            assert!(err.contains(expected), "{name}: {err}");
            assert!(!root.exists(), "{name}: the root was created anyway");
        } else {
            assert_success(&output);
            assert!(
                root.join("snapshots").is_dir(),
                "{name} is an ordinary directory name here and must work"
            );
        }
    }
}

/// A Flatpak or Snap install has its own user-data directory, and the host's
/// `$XDG_CONFIG_HOME` does not reach inside the sandbox. This is the Linux
/// answer; on macOS and Windows a `~/.var/app` tree is somebody else's data
/// and must be left alone, which is the other arm.
#[test]
fn flatpak_and_snap_trees_are_found_on_linux_and_ignored_elsewhere() {
    let tree = Tree::new();
    tree.install(
        ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
        30,
    );
    tree.install("snap/chromium/common/chromium", 20);

    let output = tree.run(&[], &[]);
    if cfg!(target_os = "linux") {
        assert_success(&output);
        let snapshot = read_snapshot(&tree.snapshot_dirs()[0]);
        assert_eq!(snapshot["source"]["browser"], "brave", "newest wins");
        let out = stdout(&output);
        assert!(out.contains("--browser chromium"), "{out}");

        // The legacy snap layout, and a native install newer than both.
        tree.install("snap/chromium/current/.config/chromium", 40);
        assert_success(&tree.run(&[], &[]));
        assert_eq!(
            read_snapshot(tree.snapshot_dirs().last().unwrap())["source"]["browser"],
            "chromium"
        );
    } else {
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("no Session_* file"),
            "{}",
            stderr(&output)
        );
        assert!(!tree.root.exists());
    }
}

/// The three Linux variables from `browsers.md` §7, end to end. Elsewhere
/// they are somebody else's convention and Chromium ignores them, so the
/// other arm asserts that setting them changes nothing.
#[test]
fn the_linux_environment_overrides_move_only_the_browsers_they_belong_to() {
    let linux = cfg!(target_os = "linux");

    // XDG_CONFIG_HOME moves every browser's native path.
    let tree = Tree::new();
    tree.install("elsewhere/microsoft-edge", 10);
    let env = [("XDG_CONFIG_HOME", tree.home.path().join("elsewhere"))];
    let output = tree.run(&[], &env);
    assert_eq!(output.status.success(), linux, "{}", stderr(&output));
    if linux {
        assert_eq!(
            read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
            "edge"
        );
    }

    // CHROME_CONFIG_HOME wins over it, for the Chrome family only.
    let tree = Tree::new();
    tree.install("chr/google-chrome", 10);
    tree.install("xdg/vivaldi", 20);
    let env = [
        ("XDG_CONFIG_HOME", tree.home.path().join("xdg")),
        ("CHROME_CONFIG_HOME", tree.home.path().join("chr")),
    ];
    let output = tree.run(&["--browser", "chrome"], &env);
    assert_eq!(output.status.success(), linux, "{}", stderr(&output));
    if linux {
        assert_eq!(
            read_snapshot(&tree.snapshot_dirs()[0])["source"]["browser"],
            "chrome"
        );
        assert_success(&tree.run(&["--browser", "vivaldi"], &env));
    }

    // CHROME_USER_DATA_DIR names a whole user-data directory, and only
    // Chrome's and Chromium's: Brave stays where it was.
    let tree = Tree::new();
    tree.install("somewhere/else", 10);
    tree.install(".config/BraveSoftware/Brave-Browser", 20);
    let env = [(
        "CHROME_USER_DATA_DIR",
        tree.home.path().join("somewhere").join("else"),
    )];
    let output = tree.run(&["--browser", "chrome"], &env);
    assert_eq!(output.status.success(), linux, "{}", stderr(&output));
    let brave = tree.run(&["--browser", "brave"], &env);
    assert_eq!(brave.status.success(), linux, "{}", stderr(&brave));
    if linux {
        let browsers: Vec<String> = tree
            .snapshot_dirs()
            .iter()
            .map(|dir| {
                read_snapshot(dir)["source"]["browser"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert!(browsers.contains(&"chrome".to_owned()), "{browsers:?}");
        assert!(browsers.contains(&"brave".to_owned()), "{browsers:?}");
    }
}

/// `--open` hands the URL to `open`, `xdg-open` or `start` depending on the
/// platform, and none of them may take the command down with them: a
/// headless Linux box has no `xdg-open` and `serve` is still doing its job.
/// `PATH` is emptied so the lookup fails where a lookup is what happens.
#[test]
fn open_failing_never_fails_serve() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let fx = Fixture::new();
    let mut child = fx
        .command()
        .args(["serve", "--port", "0", "--open"])
        .env("PATH", "")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut banner = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut banner)
        .unwrap();
    assert!(
        banner.starts_with("Your library is at http://127.0.0.1:"),
        "{banner:?}"
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

/// A home directory holding user-data trees at paths chosen by the test
/// rather than by the browser table, which is how the Flatpak, Snap and
/// environment-override layouts are built.
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

    /// A one-profile user-data directory at a home-relative path.
    fn install(&self, relative: &str, suffix: i64) {
        let user_data = relative
            .split('/')
            .fold(self.home.path().to_path_buf(), |path, part| path.join(part));
        let sessions = user_data.join("Default").join("Sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            user_data.join("Local State"),
            r#"{"profile":{"last_used":"Default","info_cache":{"Default":{"name":"Person 1"}}}}"#,
        )
        .unwrap();
        std::fs::write(
            sessions.join(format!("Session_{suffix}")),
            two_tab_session(),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str], env: &[(&str, PathBuf)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_knowmoretabs"));
        point_home_at(&mut command, self.home.path());
        for (name, value) in env {
            command.env(name, value);
        }
        command.arg("--root").arg(&self.root).args(args);
        command.output().unwrap()
    }

    fn snapshot_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.root.join("snapshots")) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs
    }
}
