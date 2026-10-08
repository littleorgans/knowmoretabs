//! `knowmoretabs add` through the real binary: the lines `--json` streams,
//! the exit status a caller branches on, and the snapshots it never touches.

mod common;

use common::{Fixture, assert_success, fingerprint, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const A: &str = "https://a.test/one";
const SAVED: &str = "https://saved.test/page";

fn lines(output: &std::process::Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn json_streams_the_library_stage_and_ends_with_done() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, SAVED, "Saved")],
    );
    let before = fingerprint(&fx.root.join("snapshots"));

    let output = fx.run(&["add", A, "--title", "One", "--no-content", "--json"]);
    assert_success(&output);
    assert_eq!(
        lines(&output),
        [
            json!({"stage": "library", "state": "running"}),
            json!({"stage": "library", "state": "done", "value": "added"}),
            json!({"stage": "done", "state": "done", "url": A, "value": "added",
                "content": null, "image": null}),
        ]
    );

    let output = fx.run(&["--json", "add", A, "--no-content"]);
    assert_success(&output);
    assert_eq!(lines(&output)[1]["value"], "known");
    let output = fx.run(&["--json", "add", SAVED, "--no-content"]);
    assert_success(&output);
    assert_eq!(lines(&output)[1]["value"], "known");

    let output = fx.run(&["--json", "content", "--url", A, "--dry-run", "--no-browser"]);
    assert_success(&output);
    assert_eq!(fingerprint(&fx.root.join("snapshots")), before);
}

#[test]
fn refusals_exit_non_zero_with_their_reason() {
    let fx = Fixture::new();
    let output = fx.run(&["add", "http://localhost:3000/", "--no-content", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let lines = lines(&output);
    assert_eq!(
        lines[1],
        json!({"stage": "library", "state": "done", "value": "refused", "reason": "private"})
    );
    assert_eq!(lines.last().unwrap()["stage"], "done");
    assert!(!fx.root.exists(), "nothing written");

    let output = fx.run(&["add", "ftp://a.test/file", "--no-content"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("not a web page address"));
}

#[test]
fn content_capture_is_not_implied_yet() {
    let fx = Fixture::new();
    let output = fx.run(&["add", A]);
    assert_eq!(output.status.code(), Some(2), "a usage error");
    assert!(stderr(&output).contains("--no-content"));
    assert!(!fx.root.exists());
}
