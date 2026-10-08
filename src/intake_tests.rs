//! The intake log's line and its fold into the `added` snapshot.
//!
//! slice: add
//! why: The fold is what makes an added page a library page everywhere, so
//!      its order, its titles and the line it reads are pinned beside it.

use std::fs;
use std::io::Write as _;

use super::*;
use crate::jsonl::Appender;

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const C: &str = "https://c.test/three";

fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}

fn append(root: &Path, lines: &[Line]) {
    let mut log = Appender::open(root, path(root)).unwrap();
    for line in lines {
        log.append(line).unwrap();
    }
}

#[test]
fn a_line_is_the_contract_shape_and_a_blank_title_is_left_out() {
    let line = Line::new(A, at("2026-10-08T09:00:00Z"), Some("One"));
    assert_eq!(
        serde_json::to_value(&line).unwrap(),
        serde_json::json!({"schema_version": 1, "url": A,
            "added_at": "2026-10-08T09:00:00Z", "title": "One"})
    );
    for blank in [None, Some(""), Some("  ")] {
        let line = Line::new(A, at("2026-10-08T09:00:00Z"), blank);
        assert_eq!(
            serde_json::to_value(&line).unwrap(),
            serde_json::json!({"schema_version": 1, "url": A,
                "added_at": "2026-10-08T09:00:00Z"})
        );
    }
}

#[test]
fn nothing_added_is_no_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    assert!(snapshot(dir.path()).unwrap().is_none());
    fs::create_dir_all(path(dir.path()).parent().unwrap()).unwrap();
    fs::write(path(dir.path()), "\n").unwrap();
    assert!(snapshot(dir.path()).unwrap().is_none());
}

#[test]
fn the_fold_is_one_window_in_add_order_dated_by_the_first_add() {
    let dir = tempfile::tempdir().unwrap();
    append(
        dir.path(),
        &[
            Line::new(B, at("2026-10-08T09:00:00Z"), None),
            Line::new(A, at("2026-10-08T09:05:00Z"), Some("One")),
            Line::new(C, at("2026-10-08T09:10:00Z"), Some("Three")),
        ],
    );
    let snapshot = snapshot(dir.path()).unwrap().unwrap();
    assert_eq!(snapshot.id, SNAPSHOT_ID);
    assert_eq!(snapshot.captured_at, at("2026-10-08T09:00:00Z"));
    assert_eq!(snapshot.schema_version, model::SCHEMA_VERSION);
    assert!(
        !snapshot.stats.is_degraded(),
        "a clean parse, not a partial one"
    );
    assert_eq!((snapshot.stats.windows, snapshot.stats.tabs), (1, 3));
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].tabs, 3);
    let tabs: Vec<_> = snapshot
        .tabs
        .iter()
        .map(|t| {
            (
                t.tab_id,
                t.window,
                t.position,
                t.url.as_str(),
                t.title.as_str(),
            )
        })
        .collect();
    assert_eq!(
        tabs,
        [(1, 1, 0, B, ""), (2, 1, 1, A, "One"), (3, 1, 2, C, "Three")]
    );
}

#[test]
fn a_title_line_names_the_page_and_keeps_its_place() {
    let dir = tempfile::tempdir().unwrap();
    append(
        dir.path(),
        &[
            Line::new(A, at("2026-10-08T09:00:00Z"), None),
            Line::new(B, at("2026-10-08T09:01:00Z"), None),
            Line::new(A, at("2026-10-08T09:02:00Z"), Some("One, titled later")),
        ],
    );
    let snapshot = snapshot(dir.path()).unwrap().unwrap();
    let tabs: Vec<_> = snapshot
        .tabs
        .iter()
        .map(|t| (t.url.as_str(), t.title.as_str()))
        .collect();
    assert_eq!(tabs, [(A, "One, titled later"), (B, "")]);
    assert_eq!(snapshot.captured_at, at("2026-10-08T09:00:00Z"));
}

#[test]
fn a_torn_tail_or_another_schema_costs_only_its_own_line() {
    let dir = tempfile::tempdir().unwrap();
    append(
        dir.path(),
        &[Line::new(A, at("2026-10-08T09:00:00Z"), None)],
    );
    let mut file = fs::File::options()
        .append(true)
        .open(path(dir.path()))
        .unwrap();
    writeln!(
        file,
        r#"{{"schema_version":2,"url":"{B}","added_at":"2026-10-08T09:01:00Z"}}"#
    )
    .unwrap();
    write!(file, r#"{{"schema_version":1,"url":"{C}","#).unwrap();
    drop(file);
    let snapshot = snapshot(dir.path()).unwrap().unwrap();
    let urls: Vec<_> = snapshot.tabs.iter().map(|t| t.url.as_str()).collect();
    assert_eq!(urls, [A]);
}

#[test]
fn a_captured_title_joins_only_an_untitled_page_dated_as_it_was_added() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let added = at("2026-10-08T09:00:00Z");
    append(
        root,
        &[Line::new(A, added, None), Line::new(B, added, Some("Two"))],
    );
    let not_held = || panic!("nothing else holds the lock");
    assert!(!retitle(root, C, "Three", not_held).unwrap(), "not added");
    assert!(
        !retitle(root, B, "Other", not_held).unwrap(),
        "already titled"
    );
    assert!(!retitle(root, A, "  ", not_held).unwrap(), "a blank title");
    let before = fs::read(path(root)).unwrap();
    assert!(retitle(root, A, "One", not_held).unwrap());
    assert!(!retitle(root, A, "Again", not_held).unwrap(), "once");
    let after = fs::read_to_string(path(root)).unwrap();
    let new: Vec<Line> = jsonl::records(&after[before.len()..]).0;
    assert_eq!(new, [Line::new(A, added, Some("One"))]);
    let tabs = snapshot(root).unwrap().unwrap().tabs;
    assert_eq!(
        tabs.iter()
            .map(|t| (t.url.as_str(), t.title.as_str()))
            .collect::<Vec<_>>(),
        [(A, "One"), (B, "Two")]
    );
}
