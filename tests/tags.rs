//! `knowmoretabs tag` and `tags` through the real binary: the state file
//! they write, the names they refuse, what export shows, and what a
//! `library.json` from before tags reads as.

mod common;

use std::fs;

use common::{Fixture, assert_success, fingerprint, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const C: &str = "https://c.test/three";
const LOCAL: &str = "http://localhost:3000/dev";

fn archive(fx: &Fixture) {
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, A, "A"), (2, B, "B"), (3, C, "C"), (4, LOCAL, "Dev")],
    );
}

fn state(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fx.root.join("library.json")).unwrap()).unwrap()
}

fn json_run(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let output = fx.run(&all);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap()
}

/// The vocabulary with its timestamps checked and dropped, so the rest can
/// be compared exactly.
fn vocabulary_names(state: &Value) -> Value {
    let mut names = serde_json::Map::new();
    for (name, term) in state["vocabulary"].as_object().unwrap() {
        let created = term["created_at"].as_str().unwrap();
        assert!(
            created.ends_with('Z') && !created.contains('.'),
            "{created}"
        );
        names.insert(
            name.clone(),
            json!({"retired": term.get("retired_at").is_some()}),
        );
    }
    Value::Object(names)
}

fn exported(fx: &Fixture) -> Value {
    assert_success(&fx.run(&["export"]));
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html[start..].find("</script>").unwrap() + start;
    serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap()
}

fn page_tags(library: &Value, url: &str) -> Value {
    library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["url"] == url)
        .unwrap_or_else(|| panic!("{url} not in pages"))["tags"]
        .clone()
}

fn vocabulary(library: &Value) -> Vec<String> {
    library["vocabulary"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            assert!(entry["created_at"].is_string());
            entry["name"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn tag_writes_both_lists_and_creates_the_vocabulary() {
    let fx = Fixture::new();
    archive(&fx);
    let before = fingerprint(&fx.root.join("snapshots"));

    let output = fx.run(&["tag", A, B, "--add", "Harness", "--add", "MCP"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "retagged 2 pages; new tags: Harness, MCP\n"
    );
    let written = state(&fx);
    assert_eq!(written["schema_version"], 1, "tags need no new schema");
    assert_eq!(written["forgotten"], json!([]));
    assert_eq!(
        written["tags"],
        json!({
            A: {"add": ["Harness", "MCP"], "remove": []},
            B: {"add": ["Harness", "MCP"], "remove": []},
        })
    );
    assert_eq!(
        vocabulary_names(&written),
        json!({"Harness": {"retired": false}, "MCP": {"retired": false}})
    );

    // Removing moves the name from one list to the other.
    let value = json_run(&fx, &["tag", A, "--remove", "mcp"]);
    assert_eq!(
        value,
        json!({
            "changed": [A], "unchanged": [], "dismissed": [], "tags": {A: ["Harness"]},
            "created": [], "revived": [],
        })
    );
    assert_eq!(
        state(&fx)["tags"][A],
        json!({"add": ["Harness"], "remove": ["MCP"]})
    );
    // And adding moves it back.
    assert_success(&fx.run(&["tag", A, "--add", "MCP"]));
    assert_eq!(
        state(&fx)["tags"][A],
        json!({"add": ["Harness", "MCP"], "remove": []})
    );

    let output = fx.run(&["tag", A, B, "--add", "harness"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "already tagged that way: 2 pages; nothing changed\n"
    );
    assert_eq!(
        fingerprint(&fx.root.join("snapshots")),
        before,
        "tagging never touches a snapshot"
    );
    assert!(fx.staging_dirs().is_empty());
}

#[test]
fn names_match_the_vocabulary_in_any_case_and_keep_its_spelling() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["tag", A, "--add", "  Model   Context\tProtocol "]));
    assert_success(&fx.run(&["tag", B, "--add", "model context protocol"]));
    let written = state(&fx);
    assert_eq!(written["tags"][B]["add"], json!(["Model Context Protocol"]));
    assert_eq!(
        vocabulary_names(&written),
        json!({"Model Context Protocol": {"retired": false}})
    );
}

#[test]
fn bad_names_and_unknown_urls_are_refused_and_nothing_is_written() {
    let fx = Fixture::new();
    archive(&fx);
    let long = "x".repeat(41);
    for (args, message) in [
        (
            vec!["tag", A, "--add", "   "],
            "cannot use \"   \" as a tag: it is empty; nothing changed",
        ),
        (
            vec!["tag", A, "--add", &long],
            "it is longer than 40 characters",
        ),
        (
            vec!["tag", A, "--add", "a\u{7}b"],
            "it contains a control character",
        ),
        (
            vec!["tag", A, "--add", "MCP", "--remove", "mcp"],
            "it is named both to add and to remove",
        ),
        (
            vec!["tag", A, "https://nowhere.test/", "--add", "MCP"],
            "not in your library: https://nowhere.test/; nothing changed",
        ),
        // Local pages are in the snapshot but not in the library.
        (
            vec!["tag", LOCAL, "--add", "MCP"],
            "not in your library: http://localhost:3000/dev",
        ),
        (
            vec!["tags", "--create", "MCP", "--retire", "Mcp"],
            "it is named both to create and to retire",
        ),
        (
            vec!["tags", "--retire", "Never"],
            "no such tag: Never; nothing changed",
        ),
    ] {
        let output = fx.run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stderr(&output).contains(message), "{}", stderr(&output));
        assert!(!fx.root.join("library.json").exists(), "{args:?}");
    }
    let output = fx.run(&["--json", "tag", A, "--add", ""]);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "tag_name");
    let output = fx.run(&["--json", "tags", "--retire", "Never"]);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "unknown_tag");
}

/// The acceptance's undo: the request with its lists swapped restores what
/// the library shows. It is exact on the pages the request changed, which
/// for a request naming one tag (all the page ever sends) is `changed`; a
/// page that already had the tag is not one to undo.
#[test]
fn the_same_request_with_the_lists_swapped_is_the_undo() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["tag", A, "--add", "Old", "--add", "Keep"]));
    assert_success(&fx.run(&["tag", B, "--add", "Old"]));
    let original = exported(&fx);

    assert_success(&fx.run(&["tag", A, B, "--add", "New", "--remove", "Old"]));
    let tagged = exported(&fx);
    assert_eq!(page_tags(&tagged, A), json!(["Keep", "New"]));
    assert_eq!(page_tags(&tagged, B), json!(["New"]));
    assert_success(&fx.run(&["tag", A, B, "--add", "Old", "--remove", "New"]));
    let undone = exported(&fx);
    for url in [A, B, C] {
        assert_eq!(page_tags(&undone, url), page_tags(&original, url), "{url}");
    }
    // The one thing an undo leaves behind: the entry the add created.
    assert_eq!(vocabulary(&undone), ["Keep", "New", "Old"]);

    // Bulk, over pages that differ: A already carries Keep.
    let value = json_run(&fx, &["tag", A, B, C, "--add", "Keep"]);
    assert_eq!(value["changed"], json!([B, C]));
    assert_eq!(value["unchanged"], json!([A]));
    assert_success(&fx.run(&["tag", B, C, "--remove", "Keep"]));
    let undone = exported(&fx);
    for url in [A, B, C] {
        assert_eq!(page_tags(&undone, url), page_tags(&original, url), "{url}");
    }
}

#[test]
fn a_retired_tag_leaves_the_library_but_stays_in_the_state_file() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["tag", A, "--add", "Harness", "--add", "Old"]));
    assert_success(&fx.run(&["tag", B, "--add", "Old"]));
    assert_success(&fx.run(&["forget", C]));
    assert_success(&fx.run(&["tag", C, "--add", "Harness"]));

    let output = fx.run(&["tags"]);
    assert_success(&output);
    // C is forgotten, so the library does not count it.
    assert_eq!(stdout(&output), "1  Harness\n2  Old\n");

    let output = fx.run(&["tags", "--retire", "old"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "retired Old\n1  Harness\n");
    let written = state(&fx);
    assert!(written["vocabulary"]["Old"]["retired_at"].is_string());
    assert_eq!(
        written["tags"][B],
        json!({"add": ["Old"], "remove": []}),
        "retiring never touches a page"
    );

    let library = exported(&fx);
    assert_eq!(vocabulary(&library), ["Harness"]);
    assert_eq!(page_tags(&library, A), json!(["Harness"]));
    assert_eq!(page_tags(&library, B), json!([]));

    let output = fx.run(&["tags", "--all"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "1  Harness\n2  Old  (retired)\n");
    let value = json_run(&fx, &["tags", "--all"]);
    assert_eq!(value["vocabulary"][1]["name"], "Old");
    assert_eq!(value["vocabulary"][1]["pages"], 2);
    assert!(value["vocabulary"][1]["retired_at"].is_string());
    assert!(value["vocabulary"][0].get("retired_at").is_none());

    // Adding a retired name brings it back, and with it every page it was on.
    let output = fx.run(&["tag", C, "--add", "OLD"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "retagged 1 page; back from retirement: Old\n"
    );
    assert!(state(&fx)["vocabulary"]["Old"].get("retired_at").is_none());
    let library = exported(&fx);
    assert_eq!(page_tags(&library, B), json!(["Old"]));
    assert_eq!(vocabulary(&library), ["Harness", "Old"]);
}

#[test]
fn tags_creates_lists_in_order_and_says_so_when_empty() {
    let fx = Fixture::new();
    archive(&fx);
    let output = fx.run(&["tags"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "No tags yet; add one with `knowmoretabs tag <URL> --add NAME`.\n"
    );
    assert!(!fx.root.join("library.json").exists());

    let output = fx.run(&["tags", "--create", "mcp", "--create", "Harness"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "created mcp, Harness\n0  Harness\n0  mcp\n"
    );
    let value = json_run(&fx, &["tags", "--create", "MCP", "--retire", "harness"]);
    assert_eq!(value["created"], json!([]), "already there, in any case");
    assert_eq!(value["retired"], json!(["Harness"]));
    assert_eq!(value["vocabulary"].as_array().unwrap().len(), 1);
    assert_eq!(value["vocabulary"][0]["name"], "mcp");
    assert_eq!(value["vocabulary"][0]["pages"], 0);
    let output = fx.run(&["tags", "--create", "Harness"]);
    assert_eq!(
        stdout(&output),
        "brought back Harness\n0  Harness\n0  mcp\n"
    );
}

#[test]
fn export_shows_tags_and_the_active_vocabulary() {
    let fx = Fixture::new();
    archive(&fx);
    assert_success(&fx.run(&["tag", A, "--add", "zeta", "--add", "Alpha", "--add", "beta"]));
    assert_success(&fx.run(&["tag", B, "--add", "beta"]));
    let library = exported(&fx);
    // Alphabetical, ignoring case, on the page and in the vocabulary.
    assert_eq!(page_tags(&library, A), json!(["Alpha", "beta", "zeta"]));
    assert_eq!(page_tags(&library, B), json!(["beta"]));
    assert_eq!(
        page_tags(&library, C),
        json!([]),
        "untagged is an empty list"
    );
    assert_eq!(vocabulary(&library), ["Alpha", "beta", "zeta"]);
    assert_eq!(
        library["vocabulary"][0].as_object().unwrap().len(),
        2,
        "name and created_at, nothing else"
    );
}

#[test]
fn a_state_file_without_tags_reads_as_untagged_and_stays_that_shape() {
    let fx = Fixture::new();
    archive(&fx);
    // No state file at all.
    let library = exported(&fx);
    assert_eq!(library["vocabulary"], json!([]));
    for page in library["pages"].as_array().unwrap() {
        assert_eq!(page["tags"], json!([]));
    }
    // One written before tags existed.
    fs::write(
        fx.root.join("library.json"),
        br#"{"schema_version":1,"forgotten":[]}"#,
    )
    .unwrap();
    let library = exported(&fx);
    assert_eq!(library["vocabulary"], json!([]));
    assert_eq!(page_tags(&library, A), json!([]));
    // A forget on it writes the shape it always did: no empty tag maps
    // for an older build, or a reader, to trip over.
    assert_success(&fx.run(&["forget", A]));
    assert_eq!(state(&fx), json!({"schema_version": 1, "forgotten": [A]}));
    // Tag and untag back to nothing, and the page's entry goes with it.
    assert_success(&fx.run(&["tag", B, "--add", "X"]));
    assert_success(&fx.run(&["tag", B, "--remove", "X"]));
    assert_eq!(state(&fx)["tags"], json!({B: {"add": [], "remove": ["X"]}}));
}

#[test]
fn fields_this_build_does_not_know_survive_a_retag() {
    let fx = Fixture::new();
    archive(&fx);
    fs::write(
        fx.root.join("library.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "forgotten": [],
            "tags": {A: {"add": ["Harness"], "remove": [], "suggested": ["MCP"]}},
            "vocabulary": {"Harness": {"created_at": "2026-09-24T10:00:00Z", "definition": "d"}},
            "notes": {A: "kept"},
        }))
        .unwrap(),
    )
    .unwrap();
    assert_success(&fx.run(&["tag", A, "--add", "Skills"]));
    let written = state(&fx);
    assert_eq!(written["notes"], json!({A: "kept"}));
    assert_eq!(written["tags"][A]["suggested"], json!(["MCP"]));
    assert_eq!(written["vocabulary"]["Harness"]["definition"], "d");
    assert_eq!(
        written["vocabulary"]["Harness"]["created_at"],
        "2026-09-24T10:00:00Z"
    );
    // Even with both lists emptied, an entry carrying other fields is kept.
    assert_success(&fx.run(&["tag", A, "--remove", "Harness", "--remove", "Skills"]));
    assert_eq!(state(&fx)["tags"][A]["suggested"], json!(["MCP"]));
}

#[test]
fn damaged_tags_are_refused_like_any_damaged_state() {
    let fx = Fixture::new();
    archive(&fx);
    let body = br#"{"schema_version":1,"forgotten":[],"tags":{"x":{"add":"Harness"}}}"#;
    fs::write(fx.root.join("library.json"), body).unwrap();
    for args in [vec!["tag", A, "--add", "MCP"], vec!["tags"], vec!["export"]] {
        let output = fx.run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains("cannot read library state"),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(fs::read(fx.root.join("library.json")).unwrap(), body);
}

#[test]
fn two_concurrent_tags_both_survive() {
    let fx = Fixture::new();
    archive(&fx);
    let children: Vec<_> = [(A, "One"), (B, "Two")]
        .iter()
        .map(|(url, name)| {
            fx.command()
                .args(["tag", url, "--add", name])
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        assert_success(&child.wait_with_output().unwrap());
    }
    let written = state(&fx);
    assert_eq!(written["tags"][A]["add"], json!(["One"]));
    assert_eq!(written["tags"][B]["add"], json!(["Two"]));
    assert_eq!(
        vocabulary_names(&written),
        json!({"One": {"retired": false}, "Two": {"retired": false}})
    );
}

#[test]
fn tag_writers_wait_for_the_lock_and_read_the_state_after_it() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    for args in [
        vec!["tag", A, "--add", "After"],
        vec!["tags", "--create", "After"],
    ] {
        let fx = Fixture::new();
        archive(&fx);
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
            .args(&args)
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
        assert!(
            waiting.contains("holds the archive; waiting"),
            "{args:?}: {waiting}"
        );
        assert!(child.try_wait().unwrap().is_none());
        assert!(!fx.root.join("library.json").exists());
        // Publish another writer's change while we still own the lock.
        fs::write(
            fx.root.join("library.json"),
            serde_json::to_vec(&json!({
                "schema_version": 1, "forgotten": [B],
                "tags": {B: {"add": ["Before"], "remove": []}},
                "vocabulary": {"Before": {"created_at": "2026-01-01T00:00:00Z"}}
            }))
            .unwrap(),
        )
        .unwrap();
        drop(lock);
        assert_success(&child.wait_with_output().unwrap());
        reader.join().unwrap();
        let written = state(&fx);
        assert_eq!(written["forgotten"], json!([B]));
        assert_eq!(written["tags"][B]["add"], json!(["Before"]));
        assert!(written["vocabulary"]["Before"].is_object());
        assert!(written["vocabulary"]["After"].is_object());
    }
}
