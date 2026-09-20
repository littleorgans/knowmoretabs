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
    write_snapshot_in(root, id, captured_at, tabs, groups, 1);
}

fn write_snapshot_in(
    root: &Path,
    id: &str,
    captured_at: &str,
    tabs: &Value,
    groups: &Value,
    windows: u32,
) {
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
    "windows": (1..=windows).map(|number| json!({"id": number, "number": number,
                  "kind": "normal", "kind_id": 1, "active_tab": null,
                  "tabs": 0})).collect::<Vec<_>>(),
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

fn tab_in(tab_id: i32, window: u32, position: i32, url: &str, title: &str) -> Value {
    json!({
        "tab_id": tab_id, "window": window, "position": position, "url": url,
        "title": title, "pinned": false, "active": false,
        "group": Value::Null, "last_active": null, "window_id": window
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

#[test]
fn a_blank_newer_title_does_not_replace_a_known_one() {
    let fx = Fixture::new();
    let page = "https://titles.test/page";
    write_snapshot(
        &fx.root,
        "2026-02-01-000000Z",
        "2026-02-01T00:00:00Z",
        &json!([tab(1, 0, page, "The title it had", None)]),
        &json!([]),
    );
    write_snapshot(
        &fx.root,
        "2026-02-02-000000Z",
        "2026-02-02T00:00:00Z",
        &json!([tab(2, 0, page, "", None), tab(3, 1, page, "   ", None)]),
        &json!([]),
    );
    let library = exported_library(&fx);
    assert_eq!(library["pages"][0]["title"], "The title it had");
}

#[test]
fn snapshots_ascend_by_capture_time_not_by_directory_name() {
    let fx = Fixture::new();
    // The directory names and the capture times disagree, which is what a
    // restored or hand-copied snapshot directory looks like.
    write_snapshot(
        &fx.root,
        "2026-03-01-000000Z",
        "2026-06-01T00:00:00Z",
        &json!([tab(1, 0, "https://late.test/", "Late", None)]),
        &json!([]),
    );
    write_snapshot(
        &fx.root,
        "2026-04-01-000000Z",
        "2026-05-01T00:00:00Z",
        &json!([tab(2, 0, "https://early.test/", "Early", None)]),
        &json!([]),
    );
    let library = exported_library(&fx);
    let ids: Vec<_> = library["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["2026-04-01-000000Z", "2026-03-01-000000Z"]);

    // `list` is the same order, reversed: newest capture first.
    let human = fx.run(&["list"]);
    assert_success(&human);
    let text = stdout(&human);
    let lines: Vec<_> = text.lines().collect();
    assert!(lines[0].contains("2026-03-01-000000Z"), "{text}");
    assert!(lines[1].contains("2026-04-01-000000Z"), "{text}");
}

#[test]
fn snapshot_id_is_the_directory_name_not_the_json_field() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-05-01-000000Z",
        "2026-05-01T00:00:00Z",
        &json!([tab(1, 0, "https://example.test/", "A", None)]),
        &json!([]),
    );
    let path = fx.root.join("snapshots/2026-05-01-000000Z/snapshot.json");
    let body = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        body.replace(
            "\"id\":\"2026-05-01-000000Z\"",
            "\"id\":\"copied-from-elsewhere\"",
        ),
    )
    .unwrap();
    let library = exported_library(&fx);
    assert_eq!(library["snapshots"][0]["id"], "2026-05-01-000000Z");
}

#[test]
fn positions_are_zero_based_per_window_and_unplaced_tabs_go_last() {
    let fx = Fixture::new();
    write_snapshot_in(
        &fx.root,
        "2026-06-01-000000Z",
        "2026-06-01T00:00:00Z",
        &json!([
            tab_in(10, 1, 0, "file:///tmp/local", "Local"),
            tab_in(11, 1, 1, "https://p.test/1", "One"),
            // Capture writes -1 when the log never placed the tab.
            tab_in(12, 1, -1, "https://p.test/2", "Two"),
            tab_in(13, 2, 0, "https://p.test/3", "Three"),
        ]),
        &json!([]),
        3,
    );
    let snapshot = &exported_library(&fx)["snapshots"][0];
    // Window 1 keeps slot 0 for the filtered local tab, so the tab that really
    // sat at position 1 still reports 1, and the unplaced tab is appended.
    assert_eq!(
        snapshot["tabs"],
        json!([
            [0, 1, 1, 11, 0, null],
            [1, 1, 2, 12, 0, null],
            [2, 2, 0, 13, 0, null]
        ])
    );
    assert_eq!(snapshot["tabs_total"], 3);
    // The window count is the snapshot's, including windows whose every tab
    // was filtered out of the library.
    assert_eq!(snapshot["windows"], 3);
}

#[test]
fn local_pages_are_excluded_under_every_spelling() {
    let fx = Fixture::new();
    let local = [
        "file:///tmp/notes.html",
        "http://localhost:8080/app",
        "http://127.0.0.1/app",
        "http://127.0.0.2/app",
        "http://[::1]:5173/app",
        "http://0.0.0.0:3000/app",
        "http://app.localhost/",
    ];
    let mut tabs: Vec<Value> = local
        .iter()
        .enumerate()
        .map(|(i, url)| {
            tab_in(
                i32::try_from(i).unwrap() + 1,
                1,
                i32::try_from(i).unwrap(),
                url,
                "Local",
            )
        })
        .collect();
    tabs.push(tab_in(
        90,
        1,
        90,
        "http://localhost.example.test/",
        "Not local",
    ));
    write_snapshot(
        &fx.root,
        "2026-07-01-000000Z",
        "2026-07-01T00:00:00Z",
        &json!(tabs),
        &json!([]),
    );
    let library = exported_library(&fx);
    let urls: Vec<_> = library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["url"].as_str().unwrap())
        .collect();
    assert_eq!(urls, ["http://localhost.example.test/"]);
    assert_eq!(library["stats"]["sightings"], 1);
}

#[test]
fn unusable_snapshots_are_skipped_and_counted_but_an_empty_one_is_kept() {
    let fx = Fixture::new();
    // Valid, readable, and holds no tabs: a real snapshot, not a damaged one.
    write_snapshot(
        &fx.root,
        "2026-08-01-000000Z",
        "2026-08-01T00:00:00Z",
        &json!([]),
        &json!([]),
    );
    let damaged: [(&str, &[u8]); 3] = [
        // Truncated.
        ("2026-08-02-000000Z", br#"{"schema_version":1,"id":"x","#),
        // Valid JSON, wrong shape.
        ("2026-08-03-000000Z", br#"{"hello":"world"}"#),
        // A schema this build cannot claim to understand.
        (
            "2026-08-04-000000Z",
            br#"{"schema_version":99,"id":"x","captured_at":"2026-08-04T00:00:00Z",
            "source":{"browser":null,"profile":null,"profile_display":null,"path":"/x",
            "file":"s","sha256":"","bytes":0,"saved_at":null,"session_started_at":null},
            "stats":{"file_version":1,"command_table":"x","commands":0,"commands_by_id":{},
            "unknown_commands":0,"unknown_command_ids":[],"malformed_commands":0,
            "truncated_bytes":0,"marker_count":1,"marker_ok":true,"windows":0,"tabs":0,
            "groups":0,"dropped_tabs":0,"dropped_tab_reasons":{"no_navigations":0,
            "window_missing":0,"window_closed":0},"navigation_fallbacks":0,
            "groups_without_metadata":0},"windows":[],"groups":[],"tabs":[]}"#,
        ),
    ];
    for (id, body) in damaged {
        let dir = fx.root.join("snapshots").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("snapshot.json"), body).unwrap();
    }
    // A directory where the file should be.
    fs::create_dir_all(fx.root.join("snapshots/2026-08-05-000000Z/snapshot.json")).unwrap();
    // Two tabs claiming one tab_id: the frontend's deep-link target is not unique.
    write_snapshot(
        &fx.root,
        "2026-08-06-000000Z",
        "2026-08-06T00:00:00Z",
        &json!([
            tab_in(7, 1, 0, "https://dup.test/1", "One"),
            tab_in(7, 1, 1, "https://dup.test/2", "Two")
        ]),
        &json!([]),
    );

    let output = fx.run(&["export", "--json"]);
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["exported"]["skipped_snapshots"], 5);
    assert_eq!(report["exported"]["stats"]["snapshots"], 1);
    assert_eq!(report["exported"]["stats"]["pages"], 0);
    assert!(stderr(&output).contains("skipped 5"), "{}", stderr(&output));

    let library = exported_library(&fx);
    assert_eq!(library["snapshots"][0]["id"], "2026-08-01-000000Z");
    assert_eq!(library["snapshots"][0]["tabs_total"], 0);
}

#[test]
fn stats_forgotten_counts_pages_the_archive_holds() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-09-01-000000Z",
        "2026-09-01T00:00:00Z",
        &json!([
            tab(1, 0, "https://kept.test/", "Kept", None),
            tab(2, 1, "https://gone.test/", "Gone", None)
        ]),
        &json!([]),
    );
    // One of these is in the archive; the other names a page no surviving
    // snapshot mentions, and is therefore not a page.
    fs::write(
        fx.root.join("library.json"),
        br#"{"schema_version":1,"forgotten":["https://gone.test/","https://never-seen.test/"]}"#,
    )
    .unwrap();
    let library = exported_library(&fx);
    assert_eq!(library["stats"]["forgotten"], 1);
    assert_eq!(library["stats"]["pages"], 1);
    assert_eq!(library["pages"][0]["url"], "https://kept.test/");
}

#[test]
fn damaged_library_state_refuses_to_export_rather_than_reveal() {
    for body in [
        &br#"{"schema_version":1,"forgotten":["#[..],
        &br"[]"[..],
        &b""[..],
        // A version this build cannot read: its forgotten list may not be
        // the whole list.
        &br#"{"schema_version":2,"forgotten":[]}"#[..],
    ] {
        let fx = Fixture::new();
        write_snapshot(
            &fx.root,
            "2026-10-01-000000Z",
            "2026-10-01T00:00:00Z",
            &json!([tab(1, 0, "https://secret.test/", "Secret", None)]),
            &json!([]),
        );
        fs::create_dir_all(&fx.root).unwrap();
        fs::write(fx.root.join("library.json"), body).unwrap();
        let output = fx.run(&["export"]);
        assert!(!output.status.success(), "{}", stdout(&output));
        assert!(
            stderr(&output).contains("cannot read library state"),
            "{}",
            stderr(&output)
        );
        assert!(
            !fx.root.join("export/index.html").exists(),
            "a refused export must publish nothing"
        );
    }
}

#[test]
fn export_refuses_a_destination_holding_the_archive_or_its_snapshots() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-11-01-000000Z",
        "2026-11-01T00:00:00Z",
        &json!([tab(1, 0, "https://example.test/", "A", None)]),
        &json!([]),
    );
    for relative in ["", "snapshots", "snapshots/2026-11-01-000000Z"] {
        let destination = fx.root.join(relative);
        let output = fx.run(&["export", destination.to_str().unwrap()]);
        assert!(!output.status.success(), "{relative}: {}", stdout(&output));
        assert!(
            stderr(&output).contains("cannot export into"),
            "{relative}: {}",
            stderr(&output)
        );
    }
    assert!(
        fs::read_to_string(fx.root.join("snapshots/2026-11-01-000000Z/snapshot.json"))
            .unwrap()
            .contains("https://example.test/")
    );
    let output = fx.run(&["export"]);
    assert_success(&output);
}

#[test]
fn embedded_data_cannot_close_its_own_script_element() {
    let fx = Fixture::new();
    write_snapshot(
        &fx.root,
        "2026-12-01-000000Z",
        "2026-12-01T00:00:00Z",
        &json!([tab(
            1,
            0,
            "https://example.test/</script><script>x=1</script>",
            "</script> in a title too",
            None
        )]),
        &json!([]),
    );
    assert_success(&fx.run(&["export"]));
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let data = &html[start..start + html[start..].find("</script>").unwrap()];
    // Nothing in the data can end the element, so the first `</script>` after
    // the marker is the element's own closing tag.
    assert!(!data.contains("</"), "{data}");
    assert!(data.contains(r"<\/script>"));
    // And it is still the same JSON once unescaped.
    let library = exported_library(&fx);
    assert_eq!(
        library["pages"][0]["url"],
        "https://example.test/</script><script>x=1</script>"
    );
}
