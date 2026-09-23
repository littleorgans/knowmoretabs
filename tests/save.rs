//! `knowmoretabs save` through the real binary: discovery, output modes,
//! the snapshot on disk, and the parser's degradation paths end to end.

mod common;

use common::{
    Fixture, SessionBuilder, assert_success, read_snapshot, stderr, stdout, two_tab_session,
};

#[test]
fn default_discovery_saves_the_newest_session_verbatim() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    fx.write_session("Default", 9, b"older, unreadable");
    std::fs::write(fx.sessions_dir("Default").join("Session_junk"), b"ignore").unwrap();
    let before = std::fs::read(fx.sessions_dir("Default").join("Session_20")).unwrap();

    let output = fx.run(&[]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("saved 2 tabs across 1 window to "), "{out}");
    assert!(!out.contains("degraded"), "{out}");

    let dirs = fx.snapshot_dirs();
    assert_eq!(dirs.len(), 1);
    let name = dirs[0].file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name.len(), "2026-09-20-084415Z".len(), "{name}");
    assert!(name.ends_with('Z'));
    assert_eq!(std::fs::read(dirs[0].join("session.snss")).unwrap(), before);
    assert_eq!(
        std::fs::read(fx.sessions_dir("Default").join("Session_20")).unwrap(),
        before
    );

    let snapshot = read_snapshot(&dirs[0]);
    assert_eq!(snapshot["schema_version"], 1);
    assert_eq!(snapshot["id"], name);
    assert_eq!(snapshot["source"]["browser"], "chrome");
    assert_eq!(snapshot["source"]["profile"], "Default");
    assert_eq!(snapshot["source"]["profile_display"], "Person 1");
    assert_eq!(snapshot["source"]["file"], "session.snss");
    assert_eq!(snapshot["source"]["bytes"], before.len());
    assert_eq!(snapshot["source"]["sha256"].as_str().unwrap().len(), 64);
    assert!(snapshot["source"]["saved_at"].is_string());
    assert!(
        snapshot["source"]["session_started_at"]
            .as_str()
            .unwrap()
            .starts_with("1601-01-01T00:00:00.00002"),
        "suffix 20 is 20 microseconds into Chrome's epoch"
    );
    assert_eq!(snapshot["stats"]["tabs"], 2);
    assert_eq!(snapshot["stats"]["windows"], 1);
    assert_eq!(snapshot["stats"]["truncated_bytes"], 0);
    assert_eq!(snapshot["stats"]["marker_ok"], true);
    assert_eq!(snapshot["tabs"][0]["url"], "https://example.test/one");
    assert_eq!(snapshot["tabs"][1]["active"], true);
    assert_eq!(snapshot["windows"][0]["active_tab"], 3);
    assert!(snapshot["groups"].as_array().unwrap().is_empty());
    let pretty = std::fs::read_to_string(dirs[0].join("snapshot.json")).unwrap();
    assert!(
        pretty.contains("\n  \"tabs\": ["),
        "snapshot.json is pretty-printed"
    );

    common::assert_private_dir(&fx.root);
    common::assert_private_dir(&fx.root.join("snapshots"));
}

#[test]
fn explicit_session_needs_no_chrome_and_records_no_browser() {
    let fx = Fixture::new();
    let copy = fx.home.path().join("backup").join("Session_1");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, two_tab_session()).unwrap();
    std::fs::remove_file(fx.user_data.join("Local State")).unwrap();

    let output = fx.run(&["save", "--session", copy.to_str().unwrap()]);
    assert_success(&output);
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert!(snapshot["source"]["browser"].is_null());
    assert!(snapshot["source"]["profile"].is_null());
    assert_eq!(snapshot["source"]["path"], copy.to_str().unwrap());
}

#[test]
fn json_output_and_quiet_mode() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    let output = fx.run(&["--json"]);
    assert_success(&output);
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["saved"]["tabs"], 2);
    assert_eq!(value["saved"]["degraded"], false);
    assert!(
        value["saved"]["path"]
            .as_str()
            .unwrap()
            .ends_with(value["saved"]["id"].as_str().unwrap())
    );
    assert_eq!(value["saved"]["stats"]["commands_by_id"]["6"], 2);

    let output = fx.run(&["--force", "-q"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
    assert_eq!(fx.snapshot_dirs().len(), 2);
}

#[test]
fn verbose_mode_names_the_source_and_command_counts() {
    let fx = Fixture::new();
    let path = fx.write_session("Default", 20, &two_tab_session());
    let output = fx.run(&["-v"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(&format!("source: {}", path.display())),
        "{out}"
    );
    assert!(out.contains("profile: Default (Person 1)"), "{out}");
    assert!(out.contains("commands: 12 ("), "{out}");
    assert!(out.contains("6:2"), "{out}");
}

#[test]
fn version_and_help_work() {
    let fx = Fixture::new();
    let output = fx.run(&["--version"]);
    assert_success(&output);
    assert!(stdout(&output).starts_with("knowmoretabs 0.1.0"));
    let output = fx.run(&["--help"]);
    assert_success(&output);
    let help = stdout(&output);
    for flag in [
        "--root",
        "--session",
        "--profile",
        "--json",
        "--verbose",
        "--quiet",
        "--force",
    ] {
        assert!(help.contains(flag), "help lacks {flag}");
    }
    for command in ["save", "list", "export", "serve", "forget", "restore"] {
        assert!(help.contains(command), "help lacks {command}");
    }
}

#[test]
fn unknown_commands_and_torn_tail_degrade_and_are_reported() {
    let fx = Fixture::new();
    let mut bytes = SessionBuilder::new()
        .raw_command(200, &[1, 2, 3])
        .simple_tab(1, 2, "https://example.test/one", "One")
        .raw_command(201, &[])
        .simple_tab(1, 3, "https://example.test/two", "Two")
        .marker()
        .build();
    bytes.truncate(bytes.len() - 1);
    fx.write_session("Default", 20, &bytes);

    let output = fx.run(&[]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("saved 2 tabs across 1 window"), "{out}");
    assert!(out.contains("degraded: 2 unknown commands (ids 200, 201), 2 trailing bytes unparsed, 0 state markers (expected 1)"), "{out}");
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert_eq!(snapshot["stats"]["unknown_commands"], 2);
    assert_eq!(
        snapshot["stats"]["unknown_command_ids"],
        serde_json::json!([200, 201])
    );
    assert_eq!(snapshot["stats"]["truncated_bytes"], 2);
    assert_eq!(snapshot["stats"]["marker_ok"], false);
    assert_eq!(snapshot["stats"]["tabs"], 2);
}

#[test]
fn dropped_tabs_and_navigation_fallbacks_are_counted_not_fatal() {
    let fx = Fixture::new();
    let bytes = SessionBuilder::new()
        .simple_tab(1, 2, "https://example.test/one", "One")
        // Tab 3: selected index 4 has no navigation; nearest is index 6.
        .set_tab_window(1, 3)
        .set_tab_index(3, 1)
        .navigation(3, 6, "https://example.test/six", "Six")
        .select_navigation(3, 4)
        // Tab 4: window 9 never got a type.
        .set_tab_window(9, 4)
        .set_tab_index(4, 0)
        .navigation(4, 0, "https://example.test/lost", "Lost")
        .select_navigation(4, 0)
        // Tab 5: no navigation at all.
        .set_tab_window(1, 5)
        .set_tab_index(5, 2)
        .select_navigation(5, 0)
        .marker()
        .build();
    fx.write_session("Default", 20, &bytes);
    let output = fx.run(&[]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("saved 2 tabs across 1 window"), "{out}");
    assert!(out.contains("2 tabs dropped"), "{out}");
    assert!(out.contains("1 navigation fallbacks"), "{out}");
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert_eq!(snapshot["stats"]["dropped_tabs"], 2);
    assert_eq!(
        snapshot["stats"]["dropped_tab_reasons"]["window_missing"],
        1
    );
    assert_eq!(
        snapshot["stats"]["dropped_tab_reasons"]["no_navigations"],
        1
    );
    assert_eq!(snapshot["stats"]["navigation_fallbacks"], 1);
    assert_eq!(snapshot["tabs"][1]["url"], "https://example.test/six");
}

#[test]
fn hostile_payloads_degrade_without_losing_recovered_tabs() {
    let fx = Fixture::new();
    let mut builder = SessionBuilder::new().simple_tab(1, 2, "https://example.test/one", "One");
    let mut malformed = 0;
    // Fixed-size commands must include their whole payload. In particular,
    // a close containing only an id must not delete the tab or its window.
    for (id, size, identifier) in [
        (0, 8, 1i32),
        (2, 8, 2),
        (5, 8, 2),
        (7, 8, 2),
        (8, 8, 1),
        (9, 8, 1),
        (11, 8, 2),
        (12, 8, 2),
        (16, 16, 2),
        (17, 16, 1),
        (21, 16, 2),
        (24, 12, 2),
        (25, 32, 2),
    ] {
        for len in [size - 1, size + 1] {
            let mut payload = vec![0; len];
            payload[..4].copy_from_slice(&identifier.to_le_bytes());
            builder = builder.raw_command(id, &payload);
            malformed += 1;
        }
    }
    // Valid Pickle framing, hostile signed URL / UTF-16 length prefixes.
    for length in [i32::MIN, -1, i32::MAX] {
        for title in [false, true] {
            let mut payload = 2i32.to_le_bytes().to_vec();
            payload.extend(1i32.to_le_bytes());
            if title {
                payload.extend(4i32.to_le_bytes());
                payload.extend(b"a://");
            }
            payload.extend(length.to_le_bytes());
            let mut pickle = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
            pickle.extend(payload);
            builder = builder.raw_command(6, &pickle);
            malformed += 1;
        }
    }
    for command in [6, 27] {
        builder = builder.raw_command(command, &u32::MAX.to_le_bytes());
        malformed += 1;
    }
    for count in [i32::MIN, -1, 0] {
        builder = builder.prune_front(2, count).prune(2, 0, count);
        malformed += 2;
    }
    builder = builder.prune(2, -1, 1);
    malformed += 1;
    let complete = builder.marker().build();
    for tail in [vec![0xff], vec![0xff, 0xff, 6, 0], vec![0, 0]] {
        let mut bytes = complete.clone();
        bytes.extend(&tail);
        fx.write_session("Default", 20, &bytes);
        let output = fx.run(&["--force", "--json"]);
        assert_success(&output);
        let result: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        let stats = &result["saved"]["stats"];
        assert_eq!(stats["malformed_commands"], malformed);
        assert_eq!(stats["truncated_bytes"], tail.len());
        assert_eq!(stats["tabs"], 1);
        assert_eq!(stats["dropped_tabs"], 0);
        let snapshot = read_snapshot(std::path::Path::new(
            result["saved"]["path"].as_str().unwrap(),
        ));
        assert_eq!(snapshot["tabs"][0]["url"], "https://example.test/one");
    }
}

#[test]
fn tab_groups_and_last_active_reach_the_snapshot() {
    let fx = Fixture::new();
    let token = (0xAB, 0xCD);
    let bytes = SessionBuilder::new()
        .group_metadata(token, "Research", 7, false, None)
        .simple_tab(1, 2, "https://example.test/one", "One")
        .simple_tab(1, 3, "https://example.test/two", "Two")
        .set_tab_group(2, Some(token))
        .last_active(2, 13_434_341_530_659_553)
        .marker()
        .build();
    fx.write_session("Default", 20, &bytes);
    let output = fx.run(&[]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with("saved 2 tabs across 1 window, 1 group to "),
        "{}",
        stdout(&output)
    );
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    let group = &snapshot["groups"][0];
    assert_eq!(group["id"], "00000000000000AB00000000000000CD");
    assert_eq!(group["title"], "Research");
    assert_eq!(group["colour"], "cyan");
    assert_eq!(group["collapsed"], false);
    assert_eq!(group["window"], 1);
    assert_eq!(snapshot["tabs"][0]["group"], group["id"]);
    assert!(snapshot["tabs"][1]["group"].is_null());
    assert_eq!(
        snapshot["tabs"][0]["last_active"],
        "2026-09-20T01:32:10.659553Z"
    );
    assert!(snapshot["tabs"][1]["last_active"].is_null());
}

#[test]
fn profile_by_display_name_and_directory_name() {
    let fx = Fixture::new();
    fx.write_local_state(
        r#"{"profile": {"last_used": "Default", "info_cache": {
            "Default": {"name": "Person 1"}, "Profile 2": {"name": "Work"}}}}"#,
    );
    fx.write_session("Default", 20, &two_tab_session());
    fx.write_session(
        "Profile 2",
        30,
        &SessionBuilder::new()
            .simple_tab(1, 2, "https://example.test/work", "W")
            .marker()
            .build(),
    );
    for name in ["Work", "Profile 2"] {
        let output = fx.run(&["--profile", name, "--force"]);
        assert_success(&output);
    }
    let dirs = fx.snapshot_dirs();
    assert_eq!(dirs.len(), 2);
    for dir in &dirs {
        let snapshot = read_snapshot(dir);
        assert_eq!(snapshot["source"]["profile"], "Profile 2");
        assert_eq!(snapshot["source"]["profile_display"], "Work");
        assert_eq!(snapshot["tabs"][0]["url"], "https://example.test/work");
    }
    let output = fx.run(&["--profile", "../Default"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("not a Chrome profile directory name"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn no_session_means_no_archive_and_an_actionable_error() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.sessions_dir("Default")).unwrap();
    let output = fx.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(
        err.starts_with("knowmoretabs: no Session_* file under "),
        "{err}"
    );
    assert!(err.contains("--session FILE"), "{err}");
    assert!(
        !fx.root.exists(),
        "no archive is created when there is nothing to save"
    );

    let output = fx.run(&["--json"]);
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "no_session");
}

#[test]
fn tabs_file_is_refused_by_name_never_parsed() {
    let fx = Fixture::new();
    let tabs = fx.home.path().join("Tabs_123");
    std::fs::write(&tabs, two_tab_session()).unwrap();
    let output = fx.run(&["--session", tabs.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("Tabs_* file"),
        "{}",
        stderr(&output)
    );
    assert!(!fx.root.exists());
}

#[test]
fn encrypted_and_garbage_headers_are_the_only_fatal_parse_errors() {
    let fx = Fixture::new();
    fx.write_session(
        "Default",
        20,
        &SessionBuilder::with_version(5).marker().build(),
    );
    let output = fx.run(&["-v"]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("encrypted session format"), "{err}");
    assert!(!fx.root.join("snapshots").join("x").exists());
    assert!(fx.snapshot_dirs().is_empty());

    fx.write_session("Default", 21, b"not a session at all");
    let output = fx.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("not a Chrome session file"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn newest_file_with_a_bad_header_falls_back_to_the_next_with_a_warning() {
    let fx = Fixture::new();
    fx.write_session("Default", 20, &two_tab_session());
    fx.write_session("Default", 21, b"SNSS\x09\0\0\0");
    let output = fx.run(&[]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("skipped newer file"),
        "{}",
        stderr(&output)
    );
    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    assert!(
        snapshot["source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("Session_20")
    );
}

/// A report that goes nowhere is not a failure: the snapshot is already on
/// disk. `knowmoretabs | head` closes stdout, and so does a full disk; the
/// exit status has to stay the one the work earned, in both output modes.
/// `--json` matters most, because a half-written document is worse to hand
/// a reader than none at all.
#[test]
fn a_dead_stdout_still_saves_and_exits_zero() {
    for args in [vec![], vec!["--json"]] {
        let fx = Fixture::new();
        fx.write_session("Default", 20, &two_tab_session());
        let output = fx.run_with_dead_stdout(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(!stderr(&output).contains("panicked"), "{}", stderr(&output));
        assert_eq!(fx.snapshot_dirs().len(), 1, "{args:?}: nothing was saved");
    }
}

#[test]
fn localhost_tabs_are_left_out_counted_and_leave_no_gaps() {
    let fx = Fixture::new();
    let (kept, local) = ((1, 1), (2, 2));
    let session = SessionBuilder::new()
        .simple_tab(1, 2, "https://example.test/one", "One")
        .simple_tab(1, 3, "http://localhost:5173/", "Dev server")
        .simple_tab(1, 4, "https://example.test/two", "Two")
        .simple_tab(1, 5, "http://127.0.0.1:7878/#pages", "Library")
        // The dev server is the front tab of window 1.
        .selected_tab(1, 1)
        // A window of nothing but this machine, between two real ones.
        .simple_tab(2, 6, "http://[::1]:3000/", "Only local")
        .simple_tab(3, 7, "https://example.test/three", "Three")
        .set_tab_group(4, Some(kept))
        .group_metadata(kept, "Reading", 1, false, None)
        .set_tab_group(5, Some(local))
        .group_metadata(local, "Local", 2, false, None)
        .marker()
        .build();
    fx.write_session("Default", 20, &session);
    let output = fx.run(&[]);
    assert_success(&output);
    assert!(
        stdout(&output)
            .starts_with("saved 3 tabs across 2 windows, 1 group (3 localhost tabs left out) to "),
        "{}",
        stdout(&output)
    );

    let snapshot = read_snapshot(&fx.snapshot_dirs()[0]);
    let tabs: Vec<(u64, i64, &str)> = snapshot["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["window"].as_u64().unwrap(),
                t["position"].as_i64().unwrap(),
                t["url"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        tabs,
        [
            (1, 0, "https://example.test/one"),
            (1, 1, "https://example.test/two"),
            (2, 0, "https://example.test/three"),
        ]
    );
    let windows = snapshot["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["tabs"], 2);
    assert!(
        windows[0]["active_tab"].is_null(),
        "the front tab was left out, so no kept tab is in front"
    );
    assert_eq!(windows[1]["number"], 2);
    let groups = snapshot["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["title"], "Reading");

    let stats = &snapshot["stats"];
    assert_eq!(stats["excluded_tabs"], 3);
    assert_eq!(stats["tabs"], 3);
    assert_eq!(stats["windows"], 2);
    assert_eq!(stats["groups"], 1);
    assert_eq!(stats["dropped_tabs"], 0, "leaving out is not degradation");

    // The browser's own file is still copied byte for byte.
    let copied = std::fs::read(fx.snapshot_dirs()[0].join("session.snss")).unwrap();
    assert_eq!(copied, session);
}
