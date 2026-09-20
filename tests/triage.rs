//! `knowmoretabs forget` and `restore` through the real binary: the state
//! file they write, the errors they refuse with, and the snapshots they
//! must never touch.

mod common;

use std::fs;

use common::{Fixture, assert_success, fingerprint, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const LOCAL: &str = "http://localhost:3000/dev";

fn archive(fx: &Fixture) {
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, A, "A"), (2, B, "B"), (3, LOCAL, "Dev")],
    );
}

fn state(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fx.root.join("library.json")).unwrap()).unwrap()
}

#[test]
fn forget_writes_the_state_file_and_restore_empties_it() {
    let fx = Fixture::new();
    archive(&fx);
    let before = fingerprint(&fx.root.join("snapshots"));

    let output = fx.run(&["forget", B, A]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "forgot 2 pages; hidden from the library, the snapshots are untouched\n"
    );
    assert_eq!(
        state(&fx),
        json!({"schema_version": 1, "forgotten": [A, B]})
    );
    let text = fs::read_to_string(fx.root.join("library.json")).unwrap();
    assert!(
        text.contains('\n'),
        "state is pretty-printed for a human editor"
    );

    let output = fx.run(&["--json", "restore", A]);
    assert_success(&output);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        value,
        json!({"action": "restore", "changed": [A], "unchanged": [], "forgotten": 1})
    );
    assert_eq!(state(&fx), json!({"schema_version": 1, "forgotten": [B]}));

    let output = fx.run(&["restore", B]);
    assert_success(&output);
    assert_eq!(stdout(&output), "restored 1 page; back in the library\n");
    assert_eq!(state(&fx), json!({"schema_version": 1, "forgotten": []}));

    assert_eq!(
        fingerprint(&fx.root.join("snapshots")),
        before,
        "forgetting never touches a snapshot"
    );
    assert!(
        fx.staging_dirs().is_empty(),
        "no staging left in the archive"
    );
    let leftovers: Vec<_> = fs::read_dir(&fx.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_url_not_in_the_library_is_refused_and_nothing_is_written() {
    let fx = Fixture::new();
    archive(&fx);
    for (args, missing) in [
        (
            vec!["forget", A, "https://nowhere.test/"],
            "https://nowhere.test/",
        ),
        // Local pages are in the snapshot but not in the library.
        (vec!["forget", LOCAL], LOCAL),
        (
            vec!["restore", "https://nowhere.test/"],
            "https://nowhere.test/",
        ),
    ] {
        let output = fx.run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            stderr(&output),
            format!("knowmoretabs: not in your library: {missing}; nothing changed\n")
        );
        assert!(stdout(&output).is_empty());
        assert!(!fx.root.join("library.json").exists(), "{args:?}");
    }
    let output = fx.run(&["--json", "forget", "https://nowhere.test/"]);
    let value: Value = serde_json::from_str(&stderr(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "not_in_library");
}

#[test]
fn repeating_a_forget_is_idempotent_and_says_so() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["forget", A]));
    let output = fx.run(&["forget", A, A, B]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "forgot 1 page (1 was already forgotten); hidden from the library, the snapshots are untouched\n"
    );
    let output = fx.run(&["forget", A]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "already forgotten: 1 page; nothing changed\n"
    );
    assert_eq!(
        state(&fx),
        json!({"schema_version": 1, "forgotten": [A, B]})
    );

    let output = fx.run(&["restore", A, B, A]);
    assert_success(&output);
    assert_eq!(stdout(&output), "restored 2 pages; back in the library\n");
    let output = fx.run(&["restore", A]);
    assert_success(&output);
    assert_eq!(stdout(&output), "not forgotten: 1 page; nothing changed\n");
    assert_success(&fx.run(&["-q", "forget", A]));
    assert_eq!(state(&fx), json!({"schema_version": 1, "forgotten": [A]}));
}

#[test]
fn a_forgotten_url_whose_snapshot_is_gone_can_still_be_restored() {
    let fx = Fixture::new();
    archive(&fx);
    fs::create_dir_all(&fx.root).unwrap();
    fs::write(
        fx.root.join("library.json"),
        br#"{"schema_version":1,"forgotten":["https://gone.test/"],"note":"kept"}"#,
    )
    .unwrap();
    let output = fx.run(&["restore", "https://gone.test/"]);
    assert_success(&output);
    // A field this build does not know survives the rewrite.
    assert_eq!(
        state(&fx),
        json!({"schema_version": 1, "forgotten": [], "note": "kept"})
    );
}

#[test]
fn damaged_state_is_refused_and_left_alone() {
    let fx = Fixture::new();
    archive(&fx);
    fs::create_dir_all(&fx.root).unwrap();
    for body in [&b"{"[..], b"[]", br#"{"schema_version":2,"forgotten":[]}"#] {
        fs::write(fx.root.join("library.json"), body).unwrap();
        let output = fx.run(&["forget", A]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("cannot read library state"),
            "{}",
            stderr(&output)
        );
        assert_eq!(fs::read(fx.root.join("library.json")).unwrap(), body);
    }
}

#[test]
fn two_concurrent_forgets_both_survive() {
    let fx = Fixture::new();
    archive(&fx);
    let children: Vec<_> = [A, B]
        .iter()
        .map(|url| fx.command().args(["forget", url]).spawn().unwrap())
        .collect();
    for child in children {
        assert_success(&child.wait_with_output().unwrap());
    }
    assert_eq!(
        state(&fx),
        json!({"schema_version": 1, "forgotten": [A, B]})
    );
}

#[test]
fn export_hides_what_forget_hid() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["forget", A]));
    let output = fx.run(&["export"]);
    assert_success(&output);
    assert!(stdout(&output).contains("1 pages across 1 snapshots (1 sightings, 1 forgotten)"));
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    assert!(!html.contains(A));
    assert!(html.contains(B));
}
