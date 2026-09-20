//! End-to-end coverage for the slice 2 library commands.
//!
//! slice: library
//! why: The frontend contract is compact enough that a real binary test can
//!      pin its page indices, snapshot order, duplicate sightings and filters.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, assert_success, stderr, stdout};
use serde_json::{Value, json};

fn write_snapshot(root: &Path, id: &str, captured_at: &str, tabs: &Value, groups: &Value) {
    let dir = root.join("snapshots").join(id);
    fs::create_dir_all(&dir).unwrap();
    let snapshot = json!({
    "schema_version": 1,
    "id": id,
    "captured_at": captured_at,
    "source": {
        "browser": "chrome", "profile": "Default", "profile_display": "Test",
        "path": "/synthetic/Session_1", "file": "session.snss", "sha256": "",
        "bytes": 0, "saved_at": captured_at, "session_started_at": null
    },
    "stats": {
        "file_version": 1, "command_table": "synthetic", "commands": 0,
        "commands_by_id": {}, "unknown_commands": 0, "unknown_command_ids": [],
        "malformed_commands": 0, "truncated_bytes": 0, "marker_count": 1,
        "marker_ok": true, "windows": 1, "tabs": tabs.as_array().unwrap().len(),
        "groups": groups.as_array().unwrap().len(), "dropped_tabs": 0,
        "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0, "window_closed": 0},
        "navigation_fallbacks": 0, "groups_without_metadata": 0
    },
    "windows": [{"id": 1, "number": 1, "kind": "normal", "kind_id": 1,
                  "active_tab": 1, "tabs": tabs.as_array().unwrap().len()}],
    "groups": groups,
    "tabs": tabs
    });
    fs::write(
        dir.join("snapshot.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
}

fn tab(tab_id: i32, position: i32, url: &str, title: &str, group: Option<&str>) -> Value {
    json!({
        "tab_id": tab_id, "window": 1, "position": position, "url": url,
        "title": title, "pinned": position == 0, "active": position == 0,
        "group": group, "last_active": null, "window_id": 1
    })
}

fn group(id: &str, title: &str) -> Value {
    json!({"id": id, "title": title, "colour": "blue", "colour_id": 1,
           "collapsed": false, "saved_guid": null, "window": 1})
}

fn archive_fixture(fx: &Fixture) {
    let page_a = "https://example.test/a";
    let page_b = "https://example.test/b";
    let page_c = "https://other.test/c";
    let local = "file:///tmp/kept-in-snapshot";
    let forgotten = "https://forgotten.test/page";
    let g = group("group-token", "Reading");

    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z-9",
        "2026-01-01T00:00:00Z",
        &json!([
            tab(1, 0, page_a, "Old title", Some("group-token")),
            tab(2, 1, page_a, "", None),
            tab(3, 2, page_b, "Stable title", None),
            tab(4, 3, local, "Local", None),
            tab(5, 4, forgotten, "Hidden", None)
        ]),
        &json!([g.clone()]),
    );
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z-10",
        "2026-01-02T00:00:00Z",
        &json!([tab(6, 0, page_b, "", None)]),
        &json!([]),
    );
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z-11",
        "2026-01-03T00:00:00Z",
        &json!([
            tab(7, 0, page_a, "Newest title", None),
            tab(8, 1, page_b, "Newer stable title", None),
            tab(9, 2, page_c, "Third page", None)
        ]),
        &json!([]),
    );
    fs::create_dir_all(fx.root.join("snapshots/broken")).unwrap();
    fs::write(fx.root.join("snapshots/broken/snapshot.json"), b"{").unwrap();
    fs::write(
        fx.root.join("library.json"),
        br#"{"schema_version":1,"forgotten":["https://forgotten.test/page"]}"#,
    )
    .unwrap();
}

fn exported_library(fx: &Fixture) -> Value {
    let output = fx.run(&["export"]);
    assert_success(&output);
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html[start..].find("</script>").unwrap() + start;
    serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap()
}

fn assert_contract_shape(actual: &Value, exemplar: &Value, path: &str) {
    if path.ends_with("][5]") {
        assert!(
            actual.is_null() || actual.is_number(),
            "{path} is not a group index or null"
        );
        return;
    }
    match exemplar {
        Value::Object(fields) => {
            let actual_fields = actual
                .as_object()
                .unwrap_or_else(|| panic!("{path} is not an object"));
            for (name, expected) in fields {
                let value = actual_fields
                    .get(name)
                    .unwrap_or_else(|| panic!("{path}.{name} is missing"));
                assert_contract_shape(value, expected, &format!("{path}.{name}"));
            }
        }
        Value::Array(examples) => {
            let actual_values = actual
                .as_array()
                .unwrap_or_else(|| panic!("{path} is not an array"));
            if let Some(example) = examples.first() {
                for (index, value) in actual_values.iter().enumerate() {
                    assert_contract_shape(value, example, &format!("{path}[{index}]"));
                }
            }
        }
        Value::String(_) => assert!(actual.is_string(), "{path} is not a string"),
        Value::Number(_) => assert!(actual.is_number(), "{path} is not a number"),
        Value::Bool(_) => assert!(actual.is_boolean(), "{path} is not a boolean"),
        Value::Null => assert!(actual.is_null(), "{path} is not null"),
    }
}

#[test]
fn export_round_trip_has_contract_counts_order_and_deduplication() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);

    assert_eq!(library["schema_version"], 1);
    assert!(library["generated_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        library["stats"],
        json!({
            "pages": 3, "snapshots": 3, "domains": 2, "sightings": 7, "forgotten": 1
        })
    );
    let snapshots = library["snapshots"].as_array().unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "2026-01-01-000000Z-9",
            "2026-01-01-000000Z-10",
            "2026-01-01-000000Z-11"
        ]
    );
    assert_eq!(snapshots[0]["tabs_total"], 3);
    assert_eq!(
        snapshots[0]["groups"][0],
        json!({"id":0,"title":"Reading","colour":"blue"})
    );
    assert_eq!(snapshots[0]["tabs"][0], json!([0, 1, 0, 1, 1, 0]));
    assert_eq!(snapshots[0]["tabs"][1], json!([0, 1, 1, 2, 0, null]));
    for snapshot in snapshots {
        for tab in snapshot["tabs"].as_array().unwrap() {
            assert!(tab[0].as_u64().unwrap() < library["pages"].as_array().unwrap().len() as u64);
            assert!(tab[4] == 0 || tab[4] == 1);
        }
    }
}

#[test]
fn contract_matches_the_committed_fixture_shape() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);
    let fixture: Value =
        serde_json::from_str(include_str!("../design-b/fixtures/library.json")).unwrap();
    assert_contract_shape(&library, &fixture, "library");
}

#[test]
fn page_seen_in_snapshots_one_and_three_is_absent_from_two() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);
    let page_index = library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|page| page["url"] == "https://example.test/a")
        .unwrap();
    let snapshots = library["snapshots"].as_array().unwrap();
    assert!(
        snapshots[0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab[0] == page_index)
    );
    assert!(
        !snapshots[1]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab[0] == page_index)
    );
    assert!(
        snapshots[2]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab[0] == page_index)
    );
}

#[test]
fn duplicate_url_in_one_snapshot_keeps_two_sightings() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);
    let page_index = library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|page| page["url"] == "https://example.test/a")
        .unwrap();
    let sightings = library["snapshots"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tab| tab[0] == page_index)
        .count();
    assert_eq!(sightings, 2);
}

#[test]
fn newest_non_empty_title_wins() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);
    let pages = library["pages"].as_array().unwrap();
    assert_eq!(
        pages
            .iter()
            .find(|page| page["url"] == "https://example.test/a")
            .unwrap()["title"],
        "Newest title"
    );
    assert_eq!(
        pages
            .iter()
            .find(|page| page["url"] == "https://example.test/b")
            .unwrap()["title"],
        "Newer stable title"
    );
}

#[test]
fn forgotten_urls_are_absent_from_export() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let library = exported_library(&fx);
    assert!(
        !library["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| { page["url"] == "https://forgotten.test/page" })
    );
    assert_eq!(library["stats"]["forgotten"], 1);
}

#[test]
fn local_url_stays_in_snapshot_but_not_library() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let snapshot: Value = serde_json::from_slice(
        &fs::read(fx.root.join("snapshots/2026-01-01-000000Z-9/snapshot.json")).unwrap(),
    )
    .unwrap();
    assert!(
        snapshot["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| { tab["url"] == "file:///tmp/kept-in-snapshot" })
    );
    let library = exported_library(&fx);
    assert!(
        !library["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| { page["url"] == "file:///tmp/kept-in-snapshot" })
    );
}

#[test]
fn corrupt_snapshot_is_skipped_and_export_succeeds() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let output = fx.run(&["export", "--json"]);
    assert_success(&output);
    assert!(stderr(&output).contains("skipped 1"), "{}", stderr(&output));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["exported"]["skipped_snapshots"], 1);
    assert_eq!(report["exported"]["stats"]["snapshots"], 3);
    assert!(fx.root.join("export/index.html").is_file());
}

#[test]
fn empty_archive_exports_valid_zero_page_library() {
    let fx = Fixture::new();
    let library = exported_library(&fx);
    assert_eq!(
        library["stats"],
        json!({
            "pages": 0, "snapshots": 0, "domains": 0, "sightings": 0, "forgotten": 0
        })
    );
    assert!(library["snapshots"].as_array().unwrap().is_empty());
    assert!(library["pages"].as_array().unwrap().is_empty());
}

#[test]
fn list_is_newest_first_in_human_and_json_forms() {
    let fx = Fixture::new();
    archive_fixture(&fx);
    let human = fx.run(&["list"]);
    assert_success(&human);
    let human_text = stdout(&human);
    let lines: Vec<_> = human_text.lines().collect();
    assert!(lines[0].contains("-11"), "{human_text}");
    assert!(lines[1].contains("-10"), "{human_text}");
    assert!(lines[2].contains("-9"), "{human_text}");
    assert!(stderr(&human).contains("skipped 1"), "{}", stderr(&human));

    let machine = fx.run(&["list", "--json"]);
    assert_success(&machine);
    let value: Value = serde_json::from_str(&stdout(&machine)).unwrap();
    assert_eq!(value["skipped_snapshots"], 1);
    assert_eq!(value["snapshots"][0]["id"], "2026-01-01-000000Z-11");
    assert_eq!(value["snapshots"][2]["id"], "2026-01-01-000000Z-9");
}
