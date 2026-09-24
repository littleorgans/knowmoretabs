//! `pages/history.json` through the real binary: a written save and
//! `history --refresh` look up every page the library knows, from one private
//! copy of History, skip the pages they must not look up, and never drop an
//! entry History has forgotten; the runs that must not read History leave the
//! file alone; and the browser's files stay exactly as they were.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::history_builder::HistoryBuilder;
use common::{
    Fixture, SessionBuilder, assert_success, fingerprint, read_snapshot, stderr, stdout,
    write_snapshot,
};
use serde_json::{Value, json};

const OPEN: &str = "https://example.test/open";
const OLDER: &str = "https://example.test/older";
const OLDEST: &str = "https://example.test/oldest";
const UNKNOWN: &str = "https://example.test/never-visited";
const T: &str = "2026-08-01T12:00:00Z";

/// One window holding these URLs in order, tab ids from 2.
fn session(urls: &[&str]) -> Vec<u8> {
    let mut builder = SessionBuilder::new();
    for (tab_id, url) in (2..).zip(urls) {
        builder = builder.simple_tab(1, tab_id, url, "Title");
    }
    builder.marker().build()
}

/// A library of two earlier snapshots that `OPEN` is not in, the browser
/// with `OPEN` open now, and a History that knows every page but `UNKNOWN`.
fn library_and_browser() -> (Fixture, HistoryBuilder) {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-07-01-000000Z",
        "2026-07-01T00:00:00Z",
        &[(1, OLDEST, "Oldest"), (2, UNKNOWN, "Unknown")],
    );
    write_snapshot(
        &fx.root,
        "2026-08-01-000000Z",
        "2026-08-01T00:00:00Z",
        &[(1, OLDER, "Older")],
    );
    fx.write_session("Default", 20, &session(&[OPEN]));
    let h = HistoryBuilder::create(&fx.history_path("Default"));
    for (url, visits) in [(OPEN, 1), (OLDER, 2), (OLDEST, 3)] {
        let id = h.url(url, visits, 0, T);
        h.visit(id, T, 0, 0);
    }
    (fx, h)
}

fn record_path(fx: &Fixture) -> PathBuf {
    fx.root.join("pages").join("history.json")
}

fn record(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(record_path(fx)).expect("pages/history.json")).unwrap()
}

fn urls(record: &Value) -> Vec<&str> {
    record["pages"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn json_run(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let output = fx.run(&all);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn a_written_save_looks_up_every_library_page_in_the_copy_it_read() {
    let (fx, h) = library_and_browser();
    let results = h.url("https://search.example.test/?q=old", 1, 0, T);
    h.search(results, "old things");
    let searched = h.visit(results, T, 0, 0);
    let id: i64 = h
        .conn
        .query_row("select id from urls where url = ?1", [OLDEST], |row| {
            row.get(0)
        })
        .unwrap();
    h.visit(id, "2026-07-15T00:00:00Z", searched, 0);
    drop(h);

    let output = fx.run(&["-v"]);
    assert_success(&output);
    let err = stderr(&output);
    assert!(
        err.contains("History signals for 3 of 4 library pages written to"),
        "{err}"
    );
    let snapshot = read_snapshot(fx.snapshot_dirs().last().unwrap());
    // The snapshot is what it always was: its own tabs only.
    assert_eq!(snapshot["history"]["tabs_found"], 1);
    assert_eq!(snapshot["tabs"].as_array().unwrap().len(), 1);

    let record = record(&fx);
    assert_eq!(record["schema_version"], 1);
    assert_eq!(urls(&record), [OLDER, OLDEST, OPEN]);
    let captured_at = &snapshot["captured_at"];
    assert_eq!(&record["updated_at"], captured_at);
    assert_eq!(
        record["pages"][OLDEST],
        json!({
            "visits": 3, "typed": 0,
            "first_visit": "2026-07-15T00:00:00Z", "last_visit": T,
            "search": {"term": "old things", "hops": 1},
            "referrer": "https://search.example.test/?q=old",
            "refreshed_at": captured_at
        })
    );
    assert_eq!(record["pages"][OPEN]["visits"], 1);
    assert_eq!(
        record["pages"][OPEN]["last_visit"], snapshot["tabs"][0]["history"]["last_visit"],
        "the same copy served both"
    );
    // The snapshot's provenance, counting library pages.
    let mut source = snapshot["history"].clone();
    source["tabs_found"] = json!(3);
    assert_eq!(record["source"], source);

    // `--force` refreshes it too, with what History says now.
    let h = HistoryBuilder {
        conn: rusqlite::Connection::open(fx.history_path("Default")).unwrap(),
    };
    h.conn
        .execute("update urls set visit_count = 9 where url = ?1", [OLDER])
        .unwrap();
    drop(h);
    assert_success(&fx.run(&["--force"]));
    let forced = self::record(&fx);
    assert_eq!(forced["pages"][OLDER]["visits"], 9);
    assert_ne!(forced["updated_at"], record["updated_at"]);
}

#[test]
fn forgotten_local_and_non_web_pages_are_never_looked_up() {
    let fx = Fixture::new();
    let forgotten = "https://example.test/forgotten";
    let skipped = [
        "http://localhost:3000/dev",
        "http://127.0.0.1:8000/admin",
        "http://[::1]:5173/",
        "http://app.localhost/",
        "chrome://settings/",
        "about:blank",
        "file:///tmp/report.pdf",
        "ftp://files.example.test/a",
    ];
    let mut tabs = vec![(1, OLDER, "Older"), (2, forgotten, "Forgotten")];
    tabs.extend((3..).zip(skipped).map(|(id, url)| (id, url, "Skipped")));
    write_snapshot(
        &fx.root,
        "2026-08-01-000000Z",
        "2026-08-01T00:00:00Z",
        &tabs,
    );
    let http = "http://plain.example.test/";
    write_snapshot(
        &fx.root,
        "2026-08-02-000000Z",
        "2026-08-02T00:00:00Z",
        &[(1, http, "Plain http")],
    );
    assert_success(&fx.run(&["forget", forgotten]));
    // What an earlier refresh recorded, before the page was forgotten.
    fs::create_dir_all(fx.root.join("pages")).unwrap();
    let earlier = json!({
        "schema_version": 1,
        "updated_at": "2026-08-01T00:00:00Z",
        "source": {"path": null, "bytes": null, "schema_version": null, "newest_visit": null,
                   "tabs_found": 1, "unavailable": [], "error": null, "skipped_by_request": false},
        "pages": {forgotten: {"visits": 1, "typed": 0, "last_visit": "2026-07-01T00:00:00Z",
                              "search": {"term": "before forgetting", "hops": 0},
                              "refreshed_at": "2026-08-01T00:00:00Z"}}
    });
    fs::write(record_path(&fx), earlier.to_string()).unwrap();

    fx.write_session("Default", 20, &session(&[OPEN]));
    let h = HistoryBuilder::create(&fx.history_path("Default"));
    for url in [OPEN, OLDER, forgotten, http].into_iter().chain(skipped) {
        let id = h.url(url, 5, 0, T);
        h.visit(id, T, 0, 0);
    }
    drop(h);

    let report = json_run(&fx, &["history", "--refresh"]);
    assert_eq!(report["refreshed"]["pages"], 2, "{report}");
    assert_eq!(report["refreshed"]["found"], 2);
    let record = record(&fx);
    assert_eq!(urls(&record), [http, forgotten, OLDER]);
    assert_eq!(
        record["pages"][forgotten], earlier["pages"][forgotten],
        "a forgotten page is not looked up, and not dropped either"
    );
    assert_eq!(record["pages"][OLDER]["visits"], 5);
    // `OPEN` is not in the library until a save puts it there.
    assert!(record["pages"].get(OPEN).is_none());
}

#[test]
fn a_page_history_has_forgotten_keeps_its_entry_and_when_it_was_last_seen() {
    let (fx, h) = library_and_browser();
    drop(h);
    let first = json_run(&fx, &["history", "--refresh"]);
    assert_eq!(first["refreshed"]["kept"], 0);
    let before = record(&fx);
    assert_eq!(urls(&before), [OLDER, OLDEST]);

    // Ninety days on: OLDEST has expired from History, OLDER was visited again.
    let h = HistoryBuilder {
        conn: rusqlite::Connection::open(fx.history_path("Default")).unwrap(),
    };
    h.conn
        .execute_batch(&format!(
            "delete from visits where url = (select id from urls where url = '{OLDEST}');
             delete from urls where url = '{OLDEST}';
             update urls set visit_count = 7 where url = '{OLDER}';"
        ))
        .unwrap();
    drop(h);
    let second = json_run(&fx, &["history", "--refresh"]);
    assert_eq!(
        second["refreshed"],
        json!({"path": record_path(&fx), "pages": 3, "found": 1, "kept": 1, "recorded": 2})
    );
    let after = record(&fx);
    assert_eq!(urls(&after), [OLDER, OLDEST]);
    assert_eq!(
        after["pages"][OLDEST], before["pages"][OLDEST],
        "kept whole"
    );
    assert_eq!(after["pages"][OLDER]["visits"], 7);
    assert_eq!(after["pages"][OLDER]["refreshed_at"], after["updated_at"]);
    assert_ne!(after["updated_at"], before["updated_at"]);
    assert_eq!(after["pages"][OLDEST]["refreshed_at"], before["updated_at"]);

    // The status says how many History no longer has.
    let status = json_run(&fx, &["history"]);
    assert_eq!(status["history"]["pages"], 2);
    assert_eq!(status["history"]["not_in_history"], 1);
    let output = fx.run(&["history"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("2 pages with History signals in")
            && stdout(&output).contains("1 of them no longer in History"),
        "{}",
        stdout(&output)
    );
    let output = fx.run(&["history", "--refresh"]);
    assert!(
        stdout(&output).contains("kept the earlier signals of 1 page History no longer has"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn history_refresh_backfills_without_saving_and_fails_when_history_cannot_be_read() {
    let (fx, h) = library_and_browser();
    drop(h);
    let output = fx.run(&["history"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("No History signals recorded for the library yet"),
        "{}",
        stdout(&output)
    );
    assert_eq!(json_run(&fx, &["history"]), json!({"history": null}));

    let output = fx.run(&["history", "--refresh"]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with("History signals for 2 of 3 library pages written to "),
        "{}",
        stdout(&output)
    );
    assert_eq!(fx.snapshot_dirs().len(), 2, "no snapshot was saved");
    assert_eq!(urls(&record(&fx)), [OLDER, OLDEST]);
    let written = fs::read(record_path(&fx)).unwrap();

    // Reading History is the whole command, so failing to is an error, and
    // the record stays exactly as it was.
    fs::write(fx.history_path("Default"), b"not a database".repeat(200)).unwrap();
    let output = fx.run(&["history", "--refresh"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output)
            .contains("History not read, so nothing was refreshed: cannot read the copy of"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read(record_path(&fx)).unwrap(), written);
    assert!(fx.staging_dirs().is_empty());
}

#[test]
fn no_history_touches_neither_the_snapshot_nor_the_record() {
    let (fx, h) = library_and_browser();
    drop(h);
    let output = fx.run(&["save", "--no-history"]);
    assert_success(&output);
    assert_eq!(fx.snapshot_dirs().len(), 3);
    assert!(!record_path(&fx).exists(), "no record is started");

    assert_success(&fx.run(&["history", "--refresh"]));
    let before = fingerprint(&fx.root.join("pages"));
    fx.write_session("Default", 21, &session(&[OPEN, OLDER]));
    let output = fx.run(&["--no-history", "-v"]);
    assert_success(&output);
    assert_eq!(fx.snapshot_dirs().len(), 4);
    assert_eq!(fingerprint(&fx.root.join("pages")), before);
    assert!(
        !stderr(&output).contains("library pages"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_skipped_save_reads_nothing_and_leaves_the_record_alone() {
    let (fx, h) = library_and_browser();
    drop(h);
    assert_success(&fx.run(&[]));
    let before = fingerprint(&fx.root.join("pages"));
    assert_eq!(before.len(), 1);

    // History moved on; the tabs did not.
    let h = HistoryBuilder {
        conn: rusqlite::Connection::open(fx.history_path("Default")).unwrap(),
    };
    h.conn
        .execute("update urls set visit_count = 50", [])
        .unwrap();
    drop(h);
    let output = fx.run(&["-v"]);
    assert_success(&output);
    assert!(stdout(&output).starts_with("no change since "));
    assert!(!stderr(&output).contains("History"), "{}", stderr(&output));
    assert_eq!(fingerprint(&fx.root.join("pages")), before);

    // A History that would fail to read is not even looked at.
    fs::write(fx.history_path("Default"), b"not a database").unwrap();
    let output = fx.run(&[]);
    assert_success(&output);
    assert_eq!(stderr(&output), "");
    assert_eq!(fingerprint(&fx.root.join("pages")), before);
}

#[test]
fn a_save_whose_history_cannot_be_read_leaves_the_record_alone() {
    let (fx, h) = library_and_browser();
    drop(h);
    assert_success(&fx.run(&["history", "--refresh"]));
    let before = fingerprint(&fx.root.join("pages"));
    fs::write(fx.history_path("Default"), b"not a database".repeat(200)).unwrap();
    let output = fx.run(&[]);
    assert_success(&output);
    let err = stderr(&output);
    assert_eq!(
        err.matches("knowmoretabs: ").count(),
        1,
        "one warning: {err}"
    );
    assert!(err.contains("History not read, so this snapshot"), "{err}");
    assert_eq!(fx.snapshot_dirs().len(), 3);
    assert_eq!(fingerprint(&fx.root.join("pages")), before);
}

#[test]
fn a_damaged_record_is_never_overwritten_and_costs_a_save_nothing() {
    let (fx, h) = library_and_browser();
    drop(h);
    fs::create_dir_all(fx.root.join("pages")).unwrap();
    for damaged in [
        &b"{\"schema_version\": 1, \"pages\": {"[..],
        br#"{"schema_version": 2}"#,
    ] {
        fs::write(record_path(&fx), damaged).unwrap();
        let output = fx.run(&["history", "--refresh"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("cannot read the History record"),
            "{}",
            stderr(&output)
        );
        let output = fx.run(&["history"]);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(fs::read(record_path(&fx)).unwrap(), damaged);
    }
    let output = fx.run(&["-v"]);
    assert_success(&output);
    let err = stderr(&output);
    assert!(
        err.contains("History signals for the library not updated: cannot read the History record"),
        "{err}"
    );
    let snapshot = read_snapshot(fx.snapshot_dirs().last().unwrap());
    assert_eq!(snapshot["tabs"][0]["history"]["visits"], 1, "{err}");
    assert_eq!(
        fs::read(record_path(&fx)).unwrap(),
        br#"{"schema_version": 2}"#
    );
}

#[test]
fn a_refresh_waits_for_the_lock_and_reads_the_library_after_it() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;

    let (fx, h) = library_and_browser();
    drop(h);
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(fx.root.join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut child = fx
        .command()
        .args(["history", "--refresh"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pipe = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(pipe).read_line(&mut line).unwrap();
        tx.send(line).unwrap();
    });
    let waiting = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(waiting.contains("holds the archive; waiting"), "{waiting}");
    assert!(child.try_wait().unwrap().is_none());
    assert!(!record_path(&fx).exists());
    // Another run forgets a page while we still hold the lock.
    fs::write(
        fx.root.join("library.json"),
        format!(r#"{{"schema_version":1,"forgotten":["{OLDEST}"]}}"#),
    )
    .unwrap();
    drop(lock);
    assert_success(&child.wait_with_output().unwrap());
    reader.join().unwrap();
    assert_eq!(urls(&record(&fx)), [OLDER], "read after the lock");
    // Replaced in one rename, in a private directory, leaving nothing beside.
    let pages: Vec<_> = fs::read_dir(fx.root.join("pages"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(pages, ["history.json"]);
    common::assert_private_dir(&fx.root.join("pages"));
}

/// Every file under `dir` whose name starts with `History`.
fn history_files_under(dir: &Path) -> Vec<PathBuf> {
    fingerprint(dir)
        .into_iter()
        .map(|(path, _, _)| path)
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("History")
        })
        .collect()
}

#[test]
fn the_browsers_files_are_untouched_and_no_copy_is_left_anywhere() {
    let (fx, h) = library_and_browser();
    drop(h);
    let path = fx.history_path("Default");
    fs::write(path.with_file_name("History-journal"), b"").unwrap();
    let past = SystemTime::now() - Duration::from_secs(86_400);
    for name in ["History", "History-journal"] {
        fs::File::options()
            .write(true)
            .open(path.with_file_name(name))
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let browser = || {
        fingerprint(path.parent().unwrap())
            .into_iter()
            .filter(|(p, _, _)| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("History")
            })
            .collect::<Vec<_>>()
    };
    let before = browser();
    assert_eq!(before.len(), 2);
    // What a run killed while reading History leaves behind.
    let killed = fx.root.join("snapshots").join(".staging-killed");
    fs::create_dir_all(&killed).unwrap();
    fs::copy(&path, killed.join("History")).unwrap();

    let temp = fx.home.path().join("temp");
    fs::create_dir(&temp).unwrap();
    for args in [&["history", "--refresh"][..], &["save"]] {
        let output = fx
            .command()
            .args(args)
            .env("TMPDIR", &temp)
            .env("TMP", &temp)
            .env("TEMP", &temp)
            .output()
            .unwrap();
        assert_success(&output);
        assert_eq!(browser(), before, "{args:?} moved the browser's files");
        assert!(!killed.exists(), "the killed run's copy is still there");
        assert!(fx.staging_dirs().is_empty());
        assert_eq!(history_files_under(&fx.root), Vec::<PathBuf>::new());
        assert_eq!(history_files_under(&temp), Vec::<PathBuf>::new());
    }
    assert_eq!(urls(&record(&fx)), [OLDER, OLDEST, OPEN]);
}

#[test]
fn refresh_reads_history_even_when_the_selected_sessions_are_encrypted() {
    let (fx, h) = library_and_browser();
    drop(h);
    let clear = fx.sessions_dir("Default").join("Session_20");
    let encrypted_dir = fx.profile_dir("Default").join("Sessions_Encrypted");
    fs::create_dir_all(&encrypted_dir).unwrap();
    let encrypted = encrypted_dir.join("Session_21");
    fs::write(&encrypted, b"SNSS\x05\0\0\0").unwrap();
    let now = SystemTime::now();
    fs::File::options()
        .write(true)
        .open(&clear)
        .unwrap()
        .set_modified(now - Duration::from_secs(600))
        .unwrap();
    fs::File::options()
        .write(true)
        .open(&encrypted)
        .unwrap()
        .set_modified(now)
        .unwrap();
    for remove_clear in [false, true] {
        if remove_clear {
            fs::remove_file(&clear).unwrap();
        }
        assert_eq!(fx.run(&["save", "--force"]).status.code(), Some(3));
        for selector in [&[][..], &["--browser", "chrome"][..]] {
            let mut args = vec!["history", "--refresh"];
            args.extend_from_slice(selector);
            let report = json_run(&fx, &args);
            assert_eq!(report["refreshed"]["found"], 2);
            assert_eq!(urls(&record(&fx)), [OLDER, OLDEST]);
            assert_eq!(fx.snapshot_dirs().len(), 2);
        }
    }
}
