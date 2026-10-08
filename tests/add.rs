//! `knowmoretabs add` through the real binary: the lines `--json` streams,
//! the exit status a caller branches on, and the snapshots it never touches.
//! Pages are read from a local server that stands in for every site: a
//! debug build resolves every name to it, so nothing reaches the internet.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::add::{add, article, intake, lines};
use common::site::{Reply, Site};
use common::{Fixture, assert_success, fingerprint, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const A: &str = "http://a.test/one";
const SAVED: &str = "https://saved.test/page";
const TOKEN: &str = "http://a.test/reset?token=abc123";

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
fn signed_in_needs_content() {
    let fx = Fixture::new();
    let output = fx.run(&["add", A, "--no-content", "--signed-in"]);
    assert_eq!(output.status.code(), Some(2), "a usage error");
    assert_eq!(stdout(&output), "");
    assert!(!fx.root.exists());
}

#[test]
fn json_follows_the_text_and_the_image_and_titles_the_page() {
    let fx = Fixture::new();
    let site = Site::start(|_, path, _| match path {
        "/one" => Reply::html(article("One note")),
        _ => Reply::status(404),
    });
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    assert_eq!(
        lines(&output),
        [
            json!({"stage": "library", "state": "running"}),
            json!({"stage": "library", "state": "done", "value": "added"}),
            json!({"stage": "content", "state": "running", "tier": "web"}),
            json!({"stage": "content", "state": "done", "status": "ok", "tier": "web",
                "http_status": 200, "title": "One note"}),
            json!({"stage": "image", "state": "running"}),
            json!({"stage": "image", "state": "done", "status": "none"}),
            json!({"stage": "done", "state": "done", "url": A, "value": "added",
                "content": "ok", "image": "none"}),
        ]
    );
    let intake = intake(&fx);
    assert_eq!(intake.len(), 2, "the page's line, then its title");
    assert_eq!(intake[0].get("title"), None);
    assert_eq!(intake[1]["title"], "One note");
    assert_eq!(intake[1]["added_at"], intake[0]["added_at"]);

    // Run again: nothing is fetched or written, and how it stands is said.
    let asked = site.seen().len();
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    assert_eq!(
        lines(&output)[1..],
        [
            json!({"stage": "library", "state": "done", "value": "known"}),
            json!({"stage": "content", "state": "done", "status": "ok", "tier": "web",
                "http_status": 200, "title": "One note"}),
            json!({"stage": "image", "state": "running"}),
            json!({"stage": "image", "state": "done", "status": "none"}),
            json!({"stage": "done", "state": "done", "url": A, "value": "known",
                "content": "ok", "image": "none"}),
        ]
    );
    assert_eq!(site.seen().len(), asked, "no request");
    assert_eq!(self::intake(&fx).len(), 2);
}

#[test]
fn a_page_known_from_a_snapshot_gets_text_and_never_a_line() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, "http://saved.test/page", "Saved")],
    );
    let site = Site::start(|_, _, _| Reply::html(article("Saved page")));
    let output = add(&fx, &site, &["http://saved.test/page", "--json"]);
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(lines[1]["value"], "known");
    assert_eq!(lines.last().unwrap()["content"], "ok");
    assert_eq!(intake(&fx), Vec::<Value>::new());
}

#[test]
fn a_failed_read_exits_zero_and_only_an_error_is_read_again() {
    let fx = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let site = Site::start({
        let calls = Arc::clone(&calls);
        move |_, path, _| match path {
            "/one" if calls.fetch_add(1, Ordering::SeqCst) == 0 => Reply::status(500),
            "/one" => Reply::html(article("One note")),
            "/blocked" => Reply::status(403),
            _ => Reply::status(404),
        }
    });
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    let first = lines(&output);
    assert_eq!(
        first[3],
        json!({"stage": "content", "state": "done", "status": "error", "tier": "web",
            "http_status": 500, "reason": "HTTP 500"})
    );
    assert_eq!(
        first.last().unwrap(),
        &json!({"stage": "done", "state": "done", "url": A, "value": "added",
            "content": "error", "image": "unknown"}),
        "a failed text waits for its text before an image"
    );
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    let second = lines(&output);
    assert_eq!(second[2]["state"], "running", "read again");
    assert_eq!(second[3]["status"], "ok");

    for (url, status, http) in [
        ("http://a.test/blocked", "blocked", 403),
        ("http://a.test/gone", "not_found", 404),
    ] {
        let output = add(&fx, &site, &[url, "--json"]);
        assert_success(&output);
        let lines = lines(&output);
        assert_eq!(
            (&lines[3]["status"], &lines[3]["http_status"]),
            (&status.into(), &http.into()),
            "{url}"
        );
        assert_eq!(lines.last().unwrap()["content"], status);
    }

    // A blocked page opened signed in, with remote debugging off in the
    // fixture's Chrome: said as the content stage's outcome, exit 0.
    let output = add(
        &fx,
        &site,
        &["http://a.test/blocked", "--signed-in", "--json"],
    );
    assert_success(&output);
    assert_eq!(
        lines(&output)[2..],
        [
            json!({"stage": "content", "state": "running", "tier": "signed_in"}),
            json!({"stage": "content", "state": "done", "status": "off", "tier": "signed_in"}),
            json!({"stage": "image", "state": "running"}),
            json!({"stage": "image", "state": "done", "status": "none"}),
            json!({"stage": "done", "state": "done", "url": "http://a.test/blocked",
                "value": "known", "content": "off", "image": "none"}),
        ]
    );
}

#[test]
fn a_timeout_is_said_as_a_wait_before_the_retry() {
    let fx = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let site = Site::start({
        let calls = Arc::clone(&calls);
        move |_, path, _| {
            let mut reply = Reply::html(article("Slow"));
            if path == "/one" && calls.fetch_add(1, Ordering::SeqCst) == 0 {
                reply.stall = Some(Duration::from_secs(2));
            } else if path != "/one" {
                reply = Reply::status(404);
            }
            reply
        }
    });
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    let lines = lines(&output);
    let states: Vec<(&str, &str)> = lines
        .iter()
        .map(|l| (l["stage"].as_str().unwrap(), l["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states[2..5],
        [
            ("content", "running"),
            ("content", "retrying"),
            ("content", "done")
        ]
    );
    let after = lines[3]["after_s"].as_f64().unwrap();
    assert!((2.0..=3.0).contains(&after), "{after}");
    assert_eq!(lines[4]["status"], "ok");
}

#[test]
fn without_json_each_stage_is_one_line() {
    let fx = Fixture::new();
    let site = Site::start(|_, path, _| match path {
        "/one" => Reply::html(article("One note")),
        _ => Reply::status(404),
    });
    let output = add(&fx, &site, &[A]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        format!("added {A} to your library\ncontent ok by web\nimage none (no_candidate)\n")
    );
    let output = add(&fx, &site, &[A, "--no-content"]);
    assert_eq!(
        stdout(&output),
        format!("already in your library: {A}; nothing changed\n")
    );
}

#[test]
fn batch_content_json_is_one_end_document_as_before() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, A, "One")],
    );
    let site = Site::start(|_, path, _| match path {
        "/one" => Reply::html(article("One note")),
        _ => Reply::status(404),
    });
    let output = fx
        .command()
        .args(["--json", "content", "--no-browser"])
        .env("KNOWMORETABS_TEST_RESOLVE", site.address.to_string())
        .output()
        .unwrap();
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(lines.len(), 1, "one document, no stage lines");
    let mut keys: Vec<&str> = lines[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "deferred",
            "dir",
            "fetch_seconds",
            "fetched",
            "headless",
            "images",
            "log",
            "more",
            "not_fetched",
            "notes",
            "reasons",
            "recorded",
            "seconds",
            "statuses",
        ]
    );
    assert_eq!(lines[0]["statuses"], json!({"ok": 1}));
}

#[test]
fn a_rule_refused_url_reports_skipped_without_a_request() {
    let fx = Fixture::new();
    let site = Site::start(|_, _, _| Reply::status(500));
    let output = add(&fx, &site, &["--json", "--", TOKEN]);
    assert_success(&output);
    let events = lines(&output);
    let content = events
        .iter()
        .find(|event| event["stage"] == "content" && event["state"] == "done")
        .unwrap();
    assert_eq!(content["status"], "skipped");
    assert_eq!(content.get("tier"), Some(&Value::Null));
    assert!(site.seen().is_empty(), "no request for a rule refused URL");
}

#[test]
fn unrecorded_stages_still_complete_once_in_order() {
    let site = Site::start(|_, _, _| Reply::status(500));
    for args in [
        vec![A, "--json"],
        vec![TOKEN, "--json"],
        vec![A, "--signed-in", "--json"],
    ] {
        let fx = Fixture::new();
        let output = add(&fx, &site, &args);
        assert_success(&output);
        let events = lines(&output);
        let completed: Vec<&str> = events
            .iter()
            .filter(|event| event["state"] == "done")
            .map(|event| event["stage"].as_str().unwrap())
            .collect();
        assert_eq!(completed, ["library", "content", "image", "done"]);
        assert_eq!(events.last().unwrap()["stage"], "done");
        assert!(
            events
                .iter()
                .any(|event| { event["stage"] == "image" && event["state"] == "running" })
        );
        for stage in ["content", "image"] {
            let settled = events
                .iter()
                .find(|event| event["stage"] == stage && event["state"] == "done")
                .unwrap();
            assert_eq!(settled["status"], events.last().unwrap()[stage]);
        }
    }
}

#[test]
fn a_title_write_failure_keeps_the_library_success_and_final_event() {
    let fx = Fixture::new();
    let intake_path = fx.root.join("pages/added.jsonl");
    let site = Site::start(move |_, path, _| match path {
        "/one" => Reply::html(article("Captured title").replace(
            "</head>",
            "<meta property=\"og:image\" content=\"http://cdn.test/image\"></head>",
        )),
        "/image" => {
            // During the image request no archive writer holds the lock.
            // Make the later title append fail without disturbing the
            // already committed intake line or the content and image stores.
            std::fs::rename(&intake_path, intake_path.with_extension("saved")).unwrap();
            std::fs::create_dir(&intake_path).unwrap();
            Reply::status(404)
        }
        _ => Reply::status(404),
    });
    let output = add(&fx, &site, &[A, "--json"]);
    assert_success(&output);
    assert!(stderr(&output).contains("read the intake log"));
    let events = lines(&output);
    assert_eq!(events.last().unwrap()["stage"], "done");
    assert_eq!(events.last().unwrap()["value"], "added");
    assert!(!events.iter().any(|event| event.get("error").is_some()));
}
