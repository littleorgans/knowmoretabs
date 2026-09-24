//! History signals in `save` through the real binary: every signal and its
//! rule, the copy that survives a live transaction and a WAL, the browser's
//! files left exactly as they were, the shapes of History that give up some
//! signals or all of them, and the runs that must not read it at all.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::history_builder::HistoryBuilder;
use common::{Fixture, SessionBuilder, assert_success, fingerprint, read_snapshot, stderr, stdout};
use rusqlite::Connection;
use serde_json::{Value, json};

/// One window holding these URLs in order, tab ids from 2.
fn session(urls: &[&str]) -> Vec<u8> {
    let mut builder = SessionBuilder::new();
    for (tab_id, url) in (2..).zip(urls) {
        builder = builder.simple_tab(1, tab_id, url, "Title");
    }
    builder.marker().build()
}

/// A fixture whose Default profile has these tabs open, and a History
/// ready to be filled.
fn profile_with(urls: &[&str]) -> (Fixture, HistoryBuilder) {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &session(urls));
    let history = HistoryBuilder::create(&fx.history_path("Default"));
    (fx, history)
}

fn saved(fx: &Fixture, args: &[&str]) -> (Value, String) {
    let output = fx.run(args);
    assert_success(&output);
    let dir = fx.snapshot_dirs().pop().expect("a snapshot was saved");
    (read_snapshot(&dir), stderr(&output))
}

fn tab_history<'a>(snapshot: &'a Value, url: &str) -> &'a Value {
    let tab = snapshot["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tab| tab["url"] == url)
        .unwrap_or_else(|| panic!("no tab for {url}"));
    &tab["history"]
}

const T: &str = "2026-08-01T12:00:00Z";

#[test]
fn every_signal_is_recorded_on_each_tab_whose_url_history_knows() {
    let article = "https://example.test/article";
    let unknown = "https://example.test/never-visited";
    let (fx, h) = profile_with(&[article, unknown, article]);
    let results = h.url(
        "https://search.example.test/?q=rust",
        1,
        0,
        "2026-07-01T09:00:00Z",
    );
    h.search(results, "rust sqlite");
    let searched = h.visit(results, "2026-07-01T09:00:00Z", 0, 0);
    let list = h.url("https://example.test/list", 2, 0, "2026-07-01T09:10:00Z");
    let listed = h.visit(list, "2026-07-01T09:10:00Z", searched, 0);
    let page = h.url(article, 12, 3, "2026-09-23T23:49:42Z");
    let first = h.visit(page, "2026-07-01T09:12:44.123456Z", listed, 0);
    // Typed later: no referring visit, still in progress.
    let latest = h.visit(page, "2026-09-23T23:49:42Z", 0, 0);
    h.foreground(first, 1_834_000_000);
    h.foreground(latest, -1_000_000);
    drop(h);

    let (snapshot, err) = saved(&fx, &["-v"]);
    let expected = json!({
        "visits": 12,
        "typed": 3,
        "first_visit": "2026-07-01T09:12:44.123456Z",
        "last_visit": "2026-09-23T23:49:42Z",
        "foreground_seconds": 1834,
        "search": {"term": "rust sqlite", "hops": 2},
        "referrer": "https://example.test/list"
    });
    assert_eq!(snapshot["tabs"][0]["history"], expected);
    assert_eq!(
        snapshot["tabs"][2]["history"], expected,
        "same URL, same signals"
    );
    assert!(
        snapshot["tabs"][1].get("history").is_none(),
        "a URL History does not know has no history key: {}",
        snapshot["tabs"][1]
    );

    let path = fx.history_path("Default");
    assert_eq!(
        snapshot["history"],
        json!({
            "path": path,
            "bytes": fs::metadata(&path).unwrap().len(),
            "schema_version": 70,
            "newest_visit": "2026-09-23T23:49:42Z",
            "tabs_found": 2,
            "unavailable": [],
            "error": null,
            "skipped_by_request": false
        })
    );
    assert!(
        err.contains(&format!(
            "History read from {}: 2 of 3 tabs found",
            path.display()
        )),
        "{err}"
    );
    assert!(!err.contains("History not read"), "{err}");

    // `--json` reports where the signals came from, never the tabs.
    let output = fx.run(&["--force", "--json"]);
    assert_success(&output);
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["saved"]["history"]["tabs_found"], 2);
    assert!(report["saved"].get("tabs").unwrap().is_number());
}

#[test]
fn history_lookup_strips_credentials_but_keeps_path_query_and_fragment() {
    let tab = "https://user:p%40ss@example.test/Case?q=one#section";
    let stored = "https://example.test/Case?q=one#section";
    let distinct = "https://example.test/Case?q=two#section";
    let (fx, h) = profile_with(&[tab, stored, distinct]);
    let id = h.url(stored, 7, 2, T);
    h.visit(id, T, 0, 0);
    h.url("https://example.test/Case?q=one", 99, 0, T);
    drop(h);
    let (snapshot, _) = saved(&fx, &[]);
    assert_eq!(tab_history(&snapshot, tab)["visits"], 7);
    assert_eq!(tab_history(&snapshot, tab), tab_history(&snapshot, stored));
    assert!(tab_history(&snapshot, distinct).is_null());
    assert_eq!(snapshot["tabs"][0]["url"], tab);
    assert_eq!(snapshot["history"]["tabs_found"], 2);
}

#[test]
fn search_is_the_nearest_results_page_within_three_hops() {
    let zero = "https://search.example.test/?q=zero";
    let one = "https://example.test/one-hop";
    let three = "https://example.test/three-hops";
    let four = "https://example.test/four-hops";
    let nearest = "https://example.test/nearest";
    let (fx, h) = profile_with(&[zero, one, three, four, nearest]);

    let results_at = |term: &str, at: &str| {
        let id = h.url(&format!("https://search.example.test/?q={term}"), 1, 0, at);
        h.search(id, term);
        h.visit(id, at, 0, 0)
    };
    let results = |term: &str| results_at(term, T);
    let page = |url: &str| h.url(url, 1, 0, T);

    let results_page = h.url(zero, 1, 0, T);
    h.search(results_page, "zero");
    h.visit(results_page, T, 0, 0);

    let from = results("one");
    h.visit(page(one), T, from, 0);

    // results -> a -(opened a tab)-> b -> page: three hops, through both links.
    let from = results("three");
    let a = h.visit(page("https://example.test/a"), T, from, 0);
    let b = h.visit(page("https://example.test/b"), T, 0, a);
    h.visit(page(three), T, b, 0);

    let mut from = results("four");
    for step in ["c", "d", "e"] {
        from = h.visit(page(&format!("https://example.test/{step}")), T, from, 0);
    }
    h.visit(page(four), T, from, 0);

    // Two visits to one page: the newer came two hops from a newer search,
    // the older one hop from an older one. The nearer wins, however old.
    let near = results_at("near", "2026-08-01T10:30:00Z");
    let far = results_at("far", "2026-08-01T12:30:00Z");
    let middle = h.visit(page("https://example.test/f"), T, far, 0);
    let target = page(nearest);
    h.visit(target, "2026-08-01T11:00:00Z", near, 0);
    h.visit(target, "2026-08-01T13:00:00Z", middle, 0);
    drop(h);

    let (snapshot, _) = saved(&fx, &[]);
    let search = |url| tab_history(&snapshot, url).get("search").cloned();
    assert_eq!(search(zero), Some(json!({"term": "zero", "hops": 0})));
    assert_eq!(search(one), Some(json!({"term": "one", "hops": 1})));
    assert_eq!(search(three), Some(json!({"term": "three", "hops": 3})));
    assert_eq!(search(four), None, "four hops back is too far");
    assert_eq!(search(nearest), Some(json!({"term": "near", "hops": 1})));
}

#[test]
fn referrer_is_another_page_else_the_external_referrer_and_never_this_machine() {
    let reloaded = "https://example.test/reloaded";
    let external = "https://example.test/from-mail";
    let local_link = "https://example.test/from-dev-server";
    let local_external = "https://example.test/local-external";
    let only_local = "https://example.test/only-local";
    let opened = "https://example.test/opened";
    let both = "https://example.test/both";
    let itself = "https://example.test/itself";
    let (fx, h) = profile_with(&[
        reloaded,
        external,
        local_link,
        local_external,
        only_local,
        opened,
        both,
        itself,
    ]);
    let visit_to = |url: &str| {
        let id = h.url(url, 1, 0, T);
        h.visit(id, T, 0, 0)
    };
    let (at1, at2, at3) = (
        "2026-08-01T10:00:00Z",
        "2026-08-01T11:00:00Z",
        "2026-08-01T12:00:00Z",
    );

    // Linked from a list, reloaded, then typed: the list is still the answer.
    let list = visit_to("https://example.test/list");
    let id = h.url(reloaded, 3, 1, at3);
    let linked = h.visit(id, at1, list, 0);
    h.visit(id, at2, linked, 0);
    h.visit(id, at3, 0, 0);

    let id = h.url(external, 1, 0, T);
    let v = h.visit(id, T, 0, 0);
    h.external_referrer(v, "https://mail.example.test/inbox?id=7");

    // The newest link came from a dev server: passed over for the older one.
    let dev = visit_to("http://localhost:5173/");
    let real = visit_to("https://example.test/real");
    let id = h.url(local_link, 2, 0, at2);
    h.visit(id, at1, real, 0);
    h.visit(id, at2, dev, 0);

    let id = h.url(local_external, 2, 0, at2);
    let older = h.visit(id, at1, 0, 0);
    h.external_referrer(older, "https://news.example.test/");
    let newer = h.visit(id, at2, 0, 0);
    h.external_referrer(newer, "http://127.0.0.1:8000/admin");

    let id = h.url(only_local, 1, 0, T);
    let from_dev = h.visit(id, T, dev, 0);
    h.external_referrer(from_dev, "http://[::1]:3000/");

    let opening_tab = visit_to("https://example.test/opener");
    h.visit(h.url(opened, 1, 0, T), T, 0, opening_tab);

    let link = visit_to("https://example.test/link");
    h.visit(h.url(both, 1, 0, T), T, link, opening_tab);

    let id = h.url(itself, 2, 0, at2);
    let first = h.visit(id, at1, 0, 0);
    h.visit(id, at2, first, 0);
    drop(h);

    let (snapshot, _) = saved(&fx, &[]);
    let referrer = |url| tab_history(&snapshot, url).get("referrer").cloned();
    assert_eq!(referrer(reloaded), Some(json!("https://example.test/list")));
    assert_eq!(
        referrer(external),
        Some(json!("https://mail.example.test/inbox?id=7"))
    );
    assert_eq!(
        referrer(local_link),
        Some(json!("https://example.test/real"))
    );
    assert_eq!(
        referrer(local_external),
        Some(json!("https://news.example.test/"))
    );
    assert_eq!(referrer(only_local), None);
    assert_eq!(referrer(opened), Some(json!("https://example.test/opener")));
    assert_eq!(referrer(both), Some(json!("https://example.test/link")));
    assert_eq!(referrer(itself), None, "a page does not refer to itself");
    assert!(tab_history(&snapshot, itself).is_object());
}

#[test]
fn foreground_counts_only_positive_durations() {
    let summed = "https://example.test/read";
    let unknown = "https://example.test/unknown-duration";
    let (fx, h) = profile_with(&[summed, unknown]);
    let id = h.url(summed, 3, 0, T);
    for micros in [5_000_000, -1_000_000, 2_500_000] {
        let v = h.visit(id, T, 0, 0);
        h.foreground(v, micros);
    }
    let id = h.url(unknown, 2, 0, T);
    for _ in 0..2 {
        let v = h.visit(id, T, 0, 0);
        h.foreground(v, -1_000_000);
    }
    drop(h);

    let (snapshot, _) = saved(&fx, &[]);
    assert_eq!(tab_history(&snapshot, summed)["foreground_seconds"], 7);
    let history = tab_history(&snapshot, unknown);
    assert_eq!(history["visits"], 2);
    assert!(history.get("foreground_seconds").is_none(), "{history}");
}

/// `History`, and each companion that exists, with its bytes and mtime.
fn browser_files(history: &Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
    let dir = history.parent().unwrap();
    fingerprint(dir)
        .into_iter()
        .filter(|(path, _, _)| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("History")
        })
        .collect()
}

/// What a copy of `History` alone, without its companions, says the page's
/// visit count is. An error is an answer too: a torn copy may not open.
fn visits_in_a_bare_copy(history: &Path, url: &str) -> Result<Option<i64>, rusqlite::Error> {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("History");
    fs::copy(history, &copy).unwrap();
    let conn = Connection::open(&copy)?;
    let count = conn
        .query_row(
            "select visit_count from urls where url = ?1",
            [url],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            err => Err(err),
        });
    drop(conn);
    count
}

/// Chrome keeps a transaction open and commits every ten seconds; a big
/// enough change spills uncommitted pages into `History` itself, with their
/// originals in `History-journal`. This test is that browser.
#[test]
fn a_copy_taken_during_an_uncommitted_transaction_reads_the_committed_state() {
    let page = "https://example.test/journal";
    let (fx, h) = profile_with(&[page]);
    let id = h.url(page, 1, 0, T);
    h.visit(id, T, 0, 0);
    h.conn.execute_batch("begin").unwrap();
    for n in 0..20_000 {
        h.url(&format!("https://filler.example.test/{n}"), 1, 0, T);
    }
    h.conn.execute_batch("commit").unwrap();
    drop(h);

    let path = fx.history_path("Default");
    let browser = Connection::open(&path).unwrap();
    browser
        .execute_batch(
            "pragma locking_mode = exclusive;
             pragma cache_size = 10;
             begin;
             update urls set visit_count = 999, title = hex(randomblob(40));",
        )
        .unwrap();
    let journal = path.with_file_name("History-journal");
    assert!(
        fs::metadata(&journal).unwrap().len() > 0,
        "the transaction spilled"
    );
    assert_ne!(
        visits_in_a_bare_copy(&path, page).ok(),
        Some(Some(1)),
        "without its journal the copy reads uncommitted or torn data"
    );
    let before = browser_files(&path);

    let (snapshot, err) = saved(&fx, &[]);
    assert!(snapshot["history"]["error"].is_null(), "{err}");
    assert_eq!(tab_history(&snapshot, page)["visits"], 1);
    assert_eq!(browser_files(&path), before, "the browser's files moved");

    browser.execute_batch("rollback").unwrap();
}

#[test]
fn readonly_browser_files_are_recovered_and_copies_are_removed() {
    let page = "https://example.test/readonly";
    let (fx, h) = profile_with(&[page]);
    let id = h.url(page, 1, 0, T);
    h.visit(id, T, 0, 0);
    h.conn
        .execute_batch("pragma cache_size = 1; begin; update urls set visit_count = 999;")
        .unwrap();
    for n in 0..100 {
        h.url(&format!("https://filler.example.test/{n}"), 1, 0, T);
    }
    let paths = [
        fx.history_path("Default"),
        fx.history_path("Default").with_file_name("History-journal"),
    ];
    let original: Vec<_> = paths
        .iter()
        .map(|path| fs::metadata(path).unwrap().permissions())
        .collect();
    for (path, permissions) in paths.iter().zip(&original) {
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(path, readonly).unwrap();
    }
    let output = fx.run(&[]);
    let leftovers = fx.staging_dirs();
    for (path, permissions) in paths.iter().zip(original) {
        fs::set_permissions(path, permissions).unwrap();
    }
    assert_success(&output);
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert_eq!(
        tab_history(&snapshot, page)["visits"],
        1,
        "{}",
        stderr(&output)
    );
    assert!(leftovers.is_empty(), "read-only copies survived cleanup");
    h.conn.execute_batch("rollback").unwrap();
}

#[test]
fn a_wal_history_is_read_with_the_rows_only_its_wal_holds() {
    let page = "https://example.test/wal";
    let (fx, h) = profile_with(&[page]);
    h.conn
        .execute_batch("pragma journal_mode = wal; pragma wal_autocheckpoint = 0;")
        .unwrap();
    let id = h.url(page, 4, 1, T);
    h.visit(id, T, 0, 0);
    let path = fx.history_path("Default");
    assert!(
        fs::metadata(path.with_file_name("History-wal"))
            .unwrap()
            .len()
            > 0,
        "the rows are in the WAL"
    );
    assert_eq!(
        visits_in_a_bare_copy(&path, page).unwrap(),
        None,
        "and only there"
    );
    let before = browser_files(&path);

    // `h` stays open: closing the last connection would checkpoint the WAL.
    let (snapshot, _) = saved(&fx, &[]);
    assert_eq!(tab_history(&snapshot, page)["visits"], 4);
    assert_eq!(browser_files(&path), before, "the browser's files moved");
    drop(h);
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
    let page = "https://example.test/page";
    let (fx, h) = profile_with(&[page]);
    let id = h.url(page, 1, 0, T);
    h.visit(id, T, 0, 0);
    drop(h);
    let path = fx.history_path("Default");
    // A cold journal and an empty WAL, as a browser leaves them; old mtimes,
    // so that any write would show.
    fs::write(path.with_file_name("History-journal"), b"").unwrap();
    fs::write(path.with_file_name("History-wal"), b"").unwrap();
    let past = SystemTime::now() - Duration::from_secs(86_400);
    for name in ["History", "History-journal", "History-wal"] {
        fs::File::options()
            .write(true)
            .open(path.with_file_name(name))
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let before = browser_files(&path);
    assert_eq!(before.len(), 3);

    // What a run killed while reading History leaves behind.
    let killed = fx.root.join("snapshots").join(".staging-killed");
    fs::create_dir_all(&killed).unwrap();
    fs::copy(&path, killed.join("History")).unwrap();
    fs::write(killed.join("History-journal"), b"").unwrap();

    let temp = fx.home.path().join("temp");
    fs::create_dir(&temp).unwrap();
    let output = fx
        .command()
        .arg("-v")
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(
        stderr(&output).contains("removed 1 stale staging directory"),
        "{}",
        stderr(&output)
    );
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert_eq!(tab_history(&snapshot, page)["visits"], 1);

    assert_eq!(browser_files(&path), before, "the browser's files moved");
    assert!(!killed.exists(), "the killed run's copy is still there");
    assert!(fx.staging_dirs().is_empty());
    assert_eq!(history_files_under(&fx.root), Vec::<PathBuf>::new());
    assert_eq!(history_files_under(&temp), Vec::<PathBuf>::new());
}

#[test]
fn an_older_history_gives_the_signals_it_has_and_names_the_rest() {
    let page = "https://example.test/old";
    let fx = Fixture::new();
    fx.write_session("Default", 20, &session(&[page]));
    let h = HistoryBuilder::create(&fx.history_path("Default")).without_newer_columns();
    let results = h.url("https://search.example.test/?q=old", 1, 0, T);
    h.search(results, "old");
    let searched = h.old_visit(results, T, 0);
    let id = h.url(page, 5, 2, T);
    h.old_visit(id, "2026-07-01T00:00:00Z", searched);
    drop(h);

    let (snapshot, err) = saved(&fx, &[]);
    assert!(!err.contains("History not read"), "{err}");
    assert_eq!(
        *tab_history(&snapshot, page),
        json!({
            "visits": 5,
            "typed": 2,
            "first_visit": "2026-07-01T00:00:00Z",
            "last_visit": T,
            "search": {"term": "old", "hops": 1},
            "referrer": "https://search.example.test/?q=old"
        })
    );
    let source = &snapshot["history"];
    assert_eq!(source["schema_version"], 48);
    assert!(source["error"].is_null());
    assert_eq!(
        source["unavailable"],
        json!([
            "visits.opener_visit",
            "visits.external_referrer_url",
            "context_annotations.visit_id",
            "context_annotations.total_foreground_duration"
        ])
    );
}

/// Puts something, or nothing, where the profile's History goes.
type MakeHistory = fn(&Path);

#[test]
fn a_history_that_cannot_be_read_costs_the_signals_not_the_snapshot() {
    let page = "https://example.test/page";
    let cases: [(&str, MakeHistory); 4] = [
        ("has no urls table", |path| {
            HistoryBuilder::create(path)
                .conn
                .execute_batch("drop table urls")
                .unwrap();
        }),
        ("has no urls.typed_count column", |path| {
            HistoryBuilder::create(path)
                .conn
                .execute_batch("alter table urls drop column typed_count")
                .unwrap();
        }),
        ("no History file at", |_| {}),
        ("cannot read the copy of", |path| {
            fs::write(path, b"this is not a database".repeat(200)).unwrap();
        }),
    ];
    for (reason, make) in cases {
        let fx = Fixture::new();
        fx.write_session("Default", 20, &session(&[page]));
        make(&fx.history_path("Default"));

        let (snapshot, err) = saved(&fx, &[]);
        let warning = "History not read, so this snapshot has no History signals: ";
        assert_eq!(err.matches(warning).count(), 1, "{reason}: {err}");
        assert!(err.contains(reason), "{reason}: {err}");
        assert!(snapshot["tabs"][0].get("history").is_none(), "{reason}");
        let source = &snapshot["history"];
        assert!(
            source["error"].as_str().unwrap().contains(reason),
            "{reason}: {source}"
        );
        assert_eq!(source["tabs_found"], 0);
        assert_eq!(source["path"], json!(fx.history_path("Default")));
        assert!(fx.staging_dirs().is_empty(), "{reason}");
    }
}

#[test]
fn a_session_file_outside_a_profile_has_no_history_and_says_so() {
    let fx = Fixture::new();
    let copy = fx.home.path().join("backup").join("Session_1");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::write(&copy, session(&["https://example.test/page"])).unwrap();

    let (snapshot, err) = saved(&fx, &["--session", copy.to_str().unwrap()]);
    assert!(err.contains("not inside a browser profile"), "{err}");
    assert!(snapshot["history"]["path"].is_null());
    assert!(
        snapshot["history"]["error"]
            .as_str()
            .unwrap()
            .contains("not inside a browser profile")
    );
}

#[test]
fn an_unchanged_layout_is_skipped_without_reading_history() {
    let page = "https://example.test/page";
    let (fx, h) = profile_with(&[page]);
    let id = h.url(page, 1, 0, T);
    h.visit(id, T, 0, 0);
    let (_, err) = saved(&fx, &["-v"]);
    assert!(err.contains("History read from"), "{err}");

    // History moved on; the tabs did not.
    h.conn
        .execute("update urls set visit_count = 2 where id = ?1", [id])
        .unwrap();
    h.visit(id, "2026-08-02T00:00:00Z", 0, 0);
    drop(h);
    let output = fx.run(&["-v"]);
    assert_success(&output);
    assert!(stdout(&output).starts_with("no change since "));
    assert!(!stderr(&output).contains("History"), "{}", stderr(&output));

    // A History that would fail to read is not even looked at.
    fs::write(fx.history_path("Default"), b"not a database").unwrap();
    let output = fx.run(&[]);
    assert_success(&output);
    assert!(stdout(&output).starts_with("no change since "));
    assert_eq!(stderr(&output), "");
    assert_eq!(fx.snapshot_dirs().len(), 1);

    // `--force` always reads it.
    let output = fx.run(&["--force", "-v"]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("History not read"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fx.snapshot_dirs().len(), 2);
}

#[test]
fn no_history_reads_nothing_and_records_that_it_was_asked_not_to() {
    let page = "https://example.test/page";
    let (fx, h) = profile_with(&[page]);
    let id = h.url(page, 1, 0, T);
    let v = h.visit(id, T, 0, 0);
    h.search(id, "private");
    h.external_referrer(v, "https://mail.example.test/");
    drop(h);

    let (snapshot, err) = saved(&fx, &["save", "--no-history", "-v"]);
    assert!(snapshot["tabs"][0].get("history").is_none());
    assert_eq!(
        snapshot["history"],
        json!({
            "path": fx.history_path("Default"),
            "bytes": null,
            "schema_version": null,
            "newest_visit": null,
            "tabs_found": 0,
            "unavailable": [],
            "error": null,
            "skipped_by_request": true
        })
    );
    assert!(err.contains("History not read (--no-history)"), "{err}");
    let text = fs::read_to_string(fx.snapshot_dirs()[0].join("snapshot.json")).unwrap();
    assert!(!text.contains("private") && !text.contains("mail.example.test"));

    // Nothing is copied or opened: a History that could not be read draws
    // no warning.
    fs::write(fx.history_path("Default"), b"not a database").unwrap();
    let output = fx.run(&["--no-history", "--force"]);
    assert_success(&output);
    assert_eq!(stderr(&output), "");
}
