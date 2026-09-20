//! The archive's promises through the real binary: unchanged sessions are
//! skipped, interrupted runs leave nothing behind, concurrent runs both
//! succeed, same-second ids get a suffix, and the staleness check refuses.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    Fixture, SessionBuilder, assert_success, read_snapshot, stderr, stdout, two_tab_session,
};

#[test]
fn unchanged_layout_is_skipped_unless_forced() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    assert_success(&fx.run(&[]));
    let first = fx.snapshot_dirs()[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // A rewritten file with the same layout but different bytes and a new
    // title: Chrome does this constantly.
    let rewritten = SessionBuilder::new()
        .simple_tab(1, 2, "https://example.test/one", "One (reloaded)")
        .simple_tab(1, 3, "https://example.test/two", "Two")
        .selected_tab(1, 0)
        .raw_command(21, &[2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0])
        .marker()
        .build();
    fx.write_session("Default", 21, &rewritten);
    let output = fx.run(&[]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.starts_with(&format!(
            "no change since {first}: 2 tabs across 1 window. Nothing saved"
        )),
        "{out}"
    );
    assert!(out.contains("--force"), "{out}");
    assert_eq!(fx.snapshot_dirs().len(), 1);
    assert!(fx.staging_dirs().is_empty());

    let output = fx.run(&["--json"]);
    assert_success(&output);
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["skipped"]["previous"], first);

    assert_success(&fx.run(&["save", "--force"]));
    assert_eq!(fx.snapshot_dirs().len(), 2);

    // A real change: a new tab.
    let changed = SessionBuilder::new()
        .simple_tab(1, 2, "https://example.test/one", "One")
        .simple_tab(1, 3, "https://example.test/two", "Two")
        .simple_tab(1, 4, "https://example.test/three", "Three")
        .marker()
        .build();
    fx.write_session("Default", 22, &changed);
    assert_success(&fx.run(&[]));
    assert_eq!(fx.snapshot_dirs().len(), 3);
}

#[test]
fn skip_compares_within_the_same_profile_only() {
    let fx = Fixture::new();
    fx.write_local_state(
        r#"{"profile": {"last_used": "Default", "info_cache": {"Default": {}, "Profile 1": {}}}}"#,
    );
    fx.write_session("Default", 20, &two_tab_session());
    fx.write_session("Profile 1", 20, &two_tab_session());
    assert_success(&fx.run(&["--profile", "Default"]));
    assert_success(&fx.run(&["--profile", "Profile 1"]));
    assert_eq!(
        fx.snapshot_dirs().len(),
        2,
        "same layout, different profile, both saved"
    );
}

/// The durability promise, exercised rather than reasoned about: a run is
/// killed outright after it has staged a complete snapshot and before it
/// renames one into place, and the archive has to be exactly what it was.
/// This runs everywhere. It used to be Unix-only, which left the platform
/// whose `rename` differs as the one platform never checked.
#[test]
#[cfg(debug_assertions)]
fn a_kill_between_staging_and_rename_leaves_the_archive_untouched() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    assert_success(&fx.run(&[]));
    let before = fx.snapshot_dirs();
    let original = std::fs::read(before[0].join("snapshot.json")).unwrap();
    let original_session = std::fs::read(before[0].join("session.snss")).unwrap();
    let ready = fx.home.path().join("staged");

    let mut child = fx
        .command()
        .args(["--force"])
        .env("KNOWMORETABS_PAUSE_BEFORE_PUBLISH", &ready)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child never reached the staging rendezvous");
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before staging"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "the run was stopped, not finished");
    // Unix kills with SIGKILL, which leaves no exit code at all; Windows
    // uses TerminateProcess, which sets one. Either way nothing ran on the
    // way out — no unwinding, no destructor, no chance to tidy up.
    assert_eq!(
        status.code().is_none(),
        cfg!(unix),
        "unexpected exit: {status:?}"
    );
    assert_eq!(fx.snapshot_dirs(), before, "nothing new was published");
    assert_eq!(
        std::fs::read(before[0].join("snapshot.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(before[0].join("session.snss")).unwrap(),
        original_session
    );
    assert_eq!(
        fx.staging_dirs().len(),
        1,
        "the half-written snapshot is still staged"
    );

    // The next run cleans up and succeeds.
    let output = fx.run(&["--force", "-v"]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("removed 1 stale staging directory"),
        "{}",
        stderr(&output)
    );
    assert!(fx.staging_dirs().is_empty());
    assert_eq!(fx.snapshot_dirs().len(), 2);
}

#[test]
fn two_concurrent_saves_both_succeed() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    let children: Vec<_> = (0..2)
        .map(|_| fx.command().arg("--force").spawn().expect("spawn"))
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert_success(&output);
    }
    assert_eq!(fx.snapshot_dirs().len(), 2);
    assert!(fx.staging_dirs().is_empty());
    for dir in fx.snapshot_dirs() {
        assert_eq!(read_snapshot(&dir)["stats"]["tabs"], 2);
    }
}

#[test]
fn same_second_collision_gets_a_numeric_suffix() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    // Two saves usually land in the same second; when they straddle one,
    // try again rather than flake.
    for _ in 0..5 {
        let _ = std::fs::remove_dir_all(&fx.root);
        assert_success(&fx.run(&["--force"]));
        assert_success(&fx.run(&["--force"]));
        let names: Vec<String> = fx
            .snapshot_dirs()
            .iter()
            .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2);
        if names[1] == format!("{}-2", names[0]) {
            assert_eq!(read_snapshot(&fx.snapshot_dirs()[1])["id"], names[1]);
            return;
        }
    }
    panic!("five attempts never landed two saves in the same second");
}

fn set_mtime(path: &std::path::Path, secs_after_epoch: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs_after_epoch))
        .unwrap();
}

#[test]
fn staleness_check_refuses_when_encrypted_files_are_newer() {
    let fx = Fixture::new();
    let clear = fx.write_session("Default", 20, &two_tab_session());
    let encrypted_dir = fx.profile_dir("Default").join("Sessions_Encrypted");
    std::fs::create_dir_all(&encrypted_dir).unwrap();
    let encrypted = encrypted_dir.join("Session_21");
    std::fs::write(&encrypted, b"SNSS\x05\0\0\0").unwrap();
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 3600;

    // Encrypted newer by under five minutes: proceed.
    set_mtime(&clear, base);
    set_mtime(&encrypted, base + 299);
    assert_success(&fx.run(&[]));
    assert_eq!(fx.snapshot_dirs().len(), 1);

    // Newer by five minutes or more: refuse, write nothing, even with --force.
    set_mtime(&encrypted, base + 300);
    let output = fx.run(&["--force"]);
    assert_eq!(output.status.code(), Some(3));
    let err = stderr(&output);
    assert!(
        err.contains("Chrome's encrypted session files are newer"),
        "{err}"
    );
    assert!(err.contains("no snapshot was saved"), "{err}");
    assert!(!err.contains("fallback"), "{err}");
    assert_eq!(fx.snapshot_dirs().len(), 1);

    let output = fx.run(&["--json"]);
    let value: serde_json::Value = serde_json::from_str(&stderr(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "stale");
    assert!(
        value["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("300s newer")
    );

    // Cleartext caught up: proceed again.
    set_mtime(&clear, base + 300);
    assert_success(&fx.run(&["--force"]));
    assert_eq!(fx.snapshot_dirs().len(), 2);
}

#[test]
fn staleness_check_refuses_when_cleartext_has_vanished() {
    let fx = Fixture::new();
    let encrypted_dir = fx.profile_dir("Default").join("Sessions_Encrypted");
    std::fs::create_dir_all(&encrypted_dir).unwrap();
    std::fs::write(encrypted_dir.join("Session_21"), b"SNSS\x05\0\0\0").unwrap();
    std::fs::create_dir_all(fx.sessions_dir("Default")).unwrap();
    let output = fx.run(&[]);
    assert_eq!(output.status.code(), Some(3));
    assert!(!fx.root.exists(), "refused before the archive was created");
}

#[test]
fn staleness_check_applies_to_an_explicit_session_inside_chromes_directory() {
    let fx = Fixture::new();
    let clear = fx.write_session("Default", 20, &two_tab_session());
    let encrypted_dir = fx.profile_dir("Default").join("Sessions_Encrypted");
    std::fs::create_dir_all(&encrypted_dir).unwrap();
    let encrypted = encrypted_dir.join("Tabs_21");
    std::fs::write(&encrypted, b"SNSS\x05\0\0\0").unwrap();
    let base = 1_700_000_000;
    set_mtime(&clear, base);
    set_mtime(&encrypted, base + 1000);

    let output = fx.run(&["--session", clear.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));

    // The same bytes copied elsewhere carry no such context.
    let copy = fx.home.path().join("Session_20");
    std::fs::copy(&clear, &copy).unwrap();
    assert_success(&fx.run(&["--session", copy.to_str().unwrap()]));
}
