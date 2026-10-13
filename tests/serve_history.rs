//! History signals in the served library and the export through the real
//! binary: the newest signals and the record that overrides them, the
//! privacy rules an export keeps, and a damaged record falling back to
//! snapshots.

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

use common::serve::{A, B, HIDDEN, Server, archive, page, without_generated_at};
use common::{Fixture, fingerprint, stderr, write_snapshot};
use serde_json::{Value, json};

const C: &str = "https://c.test/three";
const D: &str = "https://d.test/four";
const E: &str = "https://e.test/five";

/// `archive`, a third snapshot with C and D, and History signals on some
/// tabs: A's in both of its snapshots (the newer must win), a referrer that
/// is a page (B), one that is forgotten (from B), one on this machine (from
/// C), one that is the page itself (E, as saved before capture dropped
/// those), and none at all on D.
fn archive_with_history(fx: &Fixture) {
    archive(fx);
    write_snapshot(
        &fx.root,
        "2026-03-01-000000Z",
        "2026-03-01T00:00:00Z",
        &[(1, C, "C"), (2, D, "D"), (3, E, "E")],
    );
    let signals = |visits: u64, term: &str, referrer: &str| {
        json!({
            "visits": visits, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834,
            "search": {"term": term, "hops": 1},
            "referrer": referrer
        })
    };
    for (id, url, history) in [
        (
            "2026-01-01-000000Z",
            A,
            signals(2, "older search", "https://older.test/"),
        ),
        (
            "2026-01-01-000000Z",
            HIDDEN,
            signals(4, "hidden search", "https://ref.test/"),
        ),
        ("2026-02-01-000000Z", A, signals(12, "secret search", B)),
        ("2026-02-01-000000Z", B, signals(1, "b search", HIDDEN)),
        (
            "2026-03-01-000000Z",
            C,
            signals(1, "c search", "http://localhost:3000/"),
        ),
        ("2026-03-01-000000Z", E, signals(1, "e search", E)),
    ] {
        let path = fx.root.join("snapshots").join(id).join("snapshot.json");
        let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for tab in snapshot["tabs"].as_array_mut().unwrap() {
            if tab["url"] == url {
                tab["history"] = history.clone();
            }
        }
        snapshot["history"] = json!({
            "path": "/profile/History", "bytes": 4096, "schema_version": 70,
            "newest_visit": "2026-01-01T00:00:00Z", "tabs_found": 2,
            "unavailable": [], "error": null, "skipped_by_request": false
        });
        fs::write(&path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
    }
}

/// The export's embedded library and every byte of the export as text.
fn export_to(fx: &Fixture, dir: &str, args: &[&str]) -> (Value, String) {
    let dir = fx.home.path().join(dir);
    let mut all = vec!["export", dir.to_str().unwrap()];
    all.extend_from_slice(args);
    let output = fx.run(&all);
    assert!(output.status.success(), "{}", stderr(&output));
    let html = fs::read_to_string(dir.join("index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html[start..].find("</script>").unwrap() + start;
    let library = serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap();
    let text = fingerprint(&dir)
        .into_iter()
        .map(|(_, bytes, _)| String::from_utf8(bytes).unwrap())
        .collect();
    (library, text)
}

/// The library with every page's `history` taken out.
fn without_history(mut library: Value) -> Value {
    for page in library["pages"].as_array_mut().unwrap() {
        page.as_object_mut().unwrap().remove("history");
    }
    without_generated_at(library)
}

#[test]
fn serve_shows_each_pages_newest_history_signals() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    let library = Server::start(&fx).library();
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 12, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834,
            "search": {"term": "secret search", "hops": 1},
            "referrer": {"url": B, "title": "B"}
        })
    );
    // Serve lists forgotten pages flagged, so it names them as referrers too.
    assert_eq!(
        page(&library, B)["history"]["referrer"],
        json!({"url": HIDDEN, "title": "Hidden"})
    );
    assert_eq!(
        page(&library, HIDDEN)["history"]["search"]["term"],
        "hidden search"
    );
    let c = &page(&library, C)["history"];
    assert_eq!(c["visits"], 1);
    assert!(c.get("referrer").is_none(), "{c}");
    assert!(page(&library, D).get("history").is_none());
    let e = &page(&library, E)["history"];
    assert!(e.get("referrer").is_none(), "{e}");
    let text = library.to_string();
    assert!(!text.contains("older search") && !text.contains("localhost"));
}

#[test]
fn a_newer_snapshot_without_history_keeps_the_last_recorded_signals() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    write_snapshot(
        &fx.root,
        "2026-04-01-000000Z",
        "2026-04-01T00:00:00Z",
        &[(1, A, "A without new signals")],
    );
    let served = Server::start(&fx).library();
    let (exported, _) = export_to(&fx, "full", &["--with-history"]);
    for library in [served, exported] {
        let a = page(&library, A);
        assert_eq!(a["title"], "A without new signals");
        assert_eq!(a["history"]["visits"], 12);
        assert_eq!(a["history"]["search"]["term"], "secret search");
    }
}

#[test]
fn export_leaves_out_searches_and_referrers_unless_asked() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    let (library, text) = export_to(&fx, "plain", &[]);
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 12, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834
        })
    );
    let json = library.to_string();
    assert!(
        !json.contains("\"search\"") && !json.contains("\"referrer\""),
        "{json}"
    );
    for secret in [
        "secret search",
        "b search",
        "c search",
        "hidden.test",
        "ref.test",
    ] {
        assert!(!text.contains(secret), "{secret} in the export");
    }

    let (library, text) = export_to(&fx, "full", &["--with-history"]);
    let a = &page(&library, A)["history"];
    assert_eq!(a["search"], json!({"term": "secret search", "hops": 1}));
    assert_eq!(a["referrer"], json!({"url": B, "title": "B"}));
    // An export never names a forgotten page, not even as where B came from.
    let b = &page(&library, B)["history"];
    assert_eq!(b["search"]["term"], "b search");
    assert!(b.get("referrer").is_none(), "{b}");
    assert!(!text.contains("hidden.test") && !text.contains("hidden search"));
}

/// The only thing History adds to the library or the export is each page's
/// `history`: the projection stays explicit, and the snapshot-level
/// provenance, older signals and every other field stay out.
#[test]
fn history_adds_only_the_page_history_to_the_library_and_the_export() {
    let before = Fixture::new();
    archive(&before);
    write_snapshot(
        &before.root,
        "2026-03-01-000000Z",
        "2026-03-01T00:00:00Z",
        &[(1, C, "C"), (2, D, "D"), (3, E, "E")],
    );
    let after = Fixture::new();
    archive_with_history(&after);

    let served = |fx: &Fixture| without_history(Server::start(fx).library());
    assert_eq!(served(&after), served(&before));
    for args in [&[][..], &["--with-history"][..]] {
        let (plain, _) = export_to(&before, "before", args);
        let (with, text) = export_to(&after, "after", args);
        assert_eq!(without_history(with), without_history(plain));
        assert!(!text.contains("/profile/History") && !text.contains("schema_version\":70"));
    }
    let library = Server::start(&after).library();
    assert!(!library.to_string().contains("/profile/History"));
}

/// `pages/history.json` as a refresh writes it, with these entries.
fn write_record(fx: &Fixture, pages: &Value) {
    fs::create_dir_all(fx.root.join("pages")).unwrap();
    let record = json!({
        "schema_version": 1,
        "updated_at": "2026-09-01T00:00:00Z",
        "source": {"path": "/profile/History", "bytes": 4096, "schema_version": 70,
                   "newest_visit": "2026-09-01T00:00:00Z", "tabs_found": 4,
                   "unavailable": [], "error": null, "skipped_by_request": false},
        "pages": pages
    });
    fs::write(
        fx.root.join("pages").join("history.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
}

/// `archive_with_history`, and a record that has newer signals for A, the
/// first for D (which names itself), a forgotten referrer on C, a referrer on
/// this machine on E, and nothing for B.
fn archive_with_record(fx: &Fixture) {
    archive_with_history(fx);
    let entry = |visits: u64, term: &str, referrer: &str| {
        json!({
            "visits": visits, "typed": 1,
            "first_visit": "2026-06-01T00:00:00Z", "last_visit": "2026-08-30T00:00:00Z",
            "search": {"term": term, "hops": 0},
            "referrer": referrer,
            "refreshed_at": "2026-09-01T00:00:00Z"
        })
    };
    write_record(
        fx,
        &json!({
            A: entry(40, "record search", D),
            D: entry(5, "d record search", D),
            C: entry(2, "c record search", HIDDEN),
            E: entry(3, "e record search", "http://127.0.0.1:8000/"),
            "https://not-in-the-library.test/": entry(1, "stray search", A),
        }),
    );
}

#[test]
fn the_library_takes_signals_from_its_record_and_falls_back_to_snapshots() {
    let fx = Fixture::new();
    archive_with_record(&fx);
    let library = Server::start(&fx).library();
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 40, "typed": 1,
            "first_visit": "2026-06-01T00:00:00Z", "last_visit": "2026-08-30T00:00:00Z",
            "search": {"term": "record search", "hops": 0},
            "referrer": {"url": D, "title": "D"}
        }),
        "the record wins over the newest snapshot, and has no refreshed_at in the page"
    );
    let d = &page(&library, D)["history"];
    assert_eq!(d["visits"], 5, "no snapshot had D's");
    assert!(d.get("referrer").is_none(), "not itself: {d}");
    // B is not in the record: the newest snapshot's signals, as before.
    assert_eq!(page(&library, B)["history"]["search"]["term"], "b search");
    // Serve names a forgotten referrer, never this machine.
    assert_eq!(page(&library, C)["history"]["referrer"]["url"], HIDDEN);
    assert!(page(&library, E)["history"].get("referrer").is_none());
    let text = library.to_string();
    assert!(!text.contains("stray search") && !text.contains("not-in-the-library"));
    assert!(!text.contains("refreshed_at") && !text.contains("/profile/History"));
}

#[test]
fn export_keeps_its_privacy_rules_for_signals_from_the_record() {
    let fx = Fixture::new();
    archive_with_record(&fx);
    let (library, text) = export_to(&fx, "plain", &[]);
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 40, "typed": 1,
            "first_visit": "2026-06-01T00:00:00Z", "last_visit": "2026-08-30T00:00:00Z"
        })
    );
    for secret in ["record search", "hidden.test", "refreshed_at"] {
        assert!(!text.contains(secret), "{secret} in the export");
    }

    let (library, text) = export_to(&fx, "full", &["--with-history"]);
    assert_eq!(
        page(&library, A)["history"]["referrer"],
        json!({"url": D, "title": "D"})
    );
    let d = &page(&library, D)["history"];
    assert_eq!(d["search"]["term"], "d record search");
    assert!(d.get("referrer").is_none(), "not itself: {d}");
    let c = &page(&library, C)["history"];
    assert_eq!(c["search"]["term"], "c record search");
    assert!(c.get("referrer").is_none(), "a forgotten referrer: {c}");
    assert!(page(&library, E)["history"].get("referrer").is_none());
    assert!(!text.contains("hidden.test") && !text.contains("127.0.0.1:8000"));
    assert!(!text.contains("stray search"));
}

#[test]
fn a_damaged_record_is_reported_and_the_library_falls_back_to_snapshots() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    fs::create_dir_all(fx.root.join("pages")).unwrap();
    fs::write(fx.root.join("pages").join("history.json"), b"{\"pages\": ").unwrap();
    let mut child = fx
        .command()
        .args(["serve", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut warning = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut warning)
        .unwrap();
    assert!(
        warning.contains("cannot read the History record")
            && warning.contains("pages show the History signals their snapshots recorded"),
        "{warning}"
    );
    let server = Server::from_child(child);
    let library = server.library();
    assert_eq!(
        page(&library, A)["history"]["search"]["term"],
        "secret search"
    );

    let dir = fx.home.path().join("out");
    let output = fx.run(&["export", dir.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(stderr(&output).contains("cannot read the History record"));
}

#[test]
fn history_referrer_privacy_uses_chromiums_credential_free_identity() {
    let fx = Fixture::new();
    let own = "https://user:pass@own.test/Case?q=one#section";
    let own_stored = "https://own.test/Case?q=one#section";
    let hidden = "https://user:pass@hidden.test/Case?q=one#section";
    let hidden_stored = "https://hidden.test/Case?q=one#section";
    let distinct = "https://hidden.test/Case?q=two#section";
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[
            (1, own, "Own"),
            (2, hidden, "Hidden"),
            (3, A, "A"),
            (4, B, "B"),
        ],
    );
    assert!(fx.run(&["forget", hidden]).status.success());
    let entry = |referrer: &str| {
        json!({
            "visits": 1, "typed": 0, "last_visit": "2026-01-01T00:00:00Z",
            "referrer": referrer, "refreshed_at": "2026-01-01T00:00:00Z"
        })
    };
    write_record(
        &fx,
        &json!({own: entry(own_stored), A: entry(hidden_stored), B: entry(distinct)}),
    );
    let (exported, _) = export_to(&fx, "export", &["--with-history"]);
    assert!(
        page(&exported, own)["history"].get("referrer").is_none(),
        "self referrer"
    );
    assert!(
        page(&exported, A)["history"].get("referrer").is_none(),
        "forgotten referrer"
    );
    assert_eq!(page(&exported, B)["history"]["referrer"]["url"], distinct);
    let served = Server::start(&fx).library();
    assert!(page(&served, own)["history"].get("referrer").is_none());
    common::assert_success(&fx.run(&["tags", "--create", "Research"]));
    let prompt = fx.home.path().join("prompt");
    assert!(
        fx.run(&[
            "tag",
            "--prompt",
            prompt.to_str().unwrap(),
            "--with-history"
        ])
        .status
        .success()
    );
    let text = fs::read_to_string(prompt.join("pages.jsonl")).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for url in [own, A] {
        assert!(
            lines
                .iter()
                .find(|p| p["url"] == url)
                .unwrap()
                .get("referrer")
                .is_none()
        );
    }
    assert!(!text.contains(hidden_stored));
}
