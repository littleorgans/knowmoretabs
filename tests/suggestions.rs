//! Suggested tags through the real binary: `tags --define` and `--imply`,
//! the work folder `tag --prompt` writes, what `tag --import` refuses and
//! what it stores, and how the library and export show the result.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Fixture, assert_success, fingerprint, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const C: &str = "https://c.test/three";
const D: &str = "https://d.test/four";
const HIDDEN: &str = "https://hidden.test/page";
const LOCAL: &str = "http://localhost:3000/dev";

fn archive(fx: &Fixture) {
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[
            (1, A, "Agent harness"),
            (2, B, "Skills for agents"),
            (3, C, "A  design\tsystem"),
            (4, D, "Thin"),
            (5, HIDDEN, "Hidden"),
            (6, LOCAL, "Dev"),
        ],
    );
    assert_success(&fx.run(&["forget", HIDDEN]));
}

/// The vocabulary most tests use: three defined tags, one rule.
fn vocabulary(fx: &Fixture) {
    assert_success(&fx.run(&[
        "tags",
        "--define",
        "Harness",
        "Coding-agent harnesses and what configures them.",
        "--define",
        "Agent",
        "AI agents.",
        "--define",
        "Skills",
        "Reusable skills.",
        "--imply",
        "Harness",
        "Agent",
    ]));
}

fn json_run(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let output = fx.run(&all);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap()
}

fn state(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fx.root.join("library.json")).unwrap()).unwrap()
}

fn work(fx: &Fixture) -> PathBuf {
    fx.home.path().join("work")
}

fn jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn stored(fx: &Fixture) -> Vec<Value> {
    jsonl(&fx.root.join("tags").join("suggested.jsonl"))
}

fn write_answer(path: &Path, lines: &[Value]) {
    let text: String = lines.iter().map(|line| line.to_string() + "\n").collect();
    fs::write(path, text).unwrap();
}

fn answer(url: &str, tags: &[&str]) -> Value {
    json!({"url": url, "tags": tags, "source": "model-one"})
}

/// Runs `tag --import` on `file` with `extra` flags.
fn import(fx: &Fixture, file: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["tag", "--import", file.to_str().unwrap()];
    args.extend_from_slice(extra);
    fx.run(&args)
}

fn exported(fx: &Fixture) -> Value {
    assert_success(&fx.run(&["export"]));
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html[start..].find("</script>").unwrap() + start;
    serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap()
}

fn page(library: &Value, url: &str) -> Value {
    library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["url"] == url)
        .unwrap_or_else(|| panic!("{url} not in pages"))
        .clone()
}

// --- the vocabulary ---------------------------------------------------------

#[test]
fn definitions_and_parent_rules_live_in_the_vocabulary_and_show_in_tags() {
    let fx = Fixture::new();
    archive(&fx);
    let output = fx.run(&[
        "tags",
        "--create",
        "Training",
        "--define",
        "DPO",
        "  Direct   preference\noptimisation. ",
        "--imply",
        "dpo",
        "training",
    ]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "created Training, DPO; defined DPO; DPO now implies Training\n\
         0  DPO — Direct preference optimisation. (implies Training)\n\
         0  Training\n"
    );
    let written = state(&fx);
    assert_eq!(
        written["vocabulary"]["DPO"]["definition"],
        "Direct preference optimisation."
    );
    assert_eq!(written["vocabulary"]["DPO"]["implies"], json!(["Training"]));
    assert!(
        written["vocabulary"]["Training"].get("implies").is_none(),
        "no empty lists written"
    );

    let listed = json_run(&fx, &["tags"]);
    assert_eq!(
        listed["vocabulary"][0]["definition"],
        "Direct preference optimisation."
    );
    assert_eq!(listed["vocabulary"][0]["implies"], json!(["Training"]));
    let version = listed["version"].as_str().unwrap().to_owned();
    assert_eq!(version.len(), 12);

    // The version follows what a tagger would read, and only that.
    assert_success(&fx.run(&["tag", A, "--add", "Training"]));
    assert_eq!(json_run(&fx, &["tags"])["version"], version.as_str());
    let changed = json_run(&fx, &["tags", "--define", "Training", "Learning."]);
    assert_ne!(changed["version"], version.as_str());
    assert_eq!(changed["defined"], json!(["Training"]));
    let cleared = json_run(&fx, &["tags", "--define", "Training", ""]);
    assert_eq!(cleared["version"], version.as_str(), "cleared is as before");
    assert!(
        state(&fx)["vocabulary"]["Training"]
            .get("definition")
            .is_none()
    );
    let unimplied = json_run(&fx, &["tags", "--unimply", "DPO", "Training"]);
    assert_eq!(unimplied["unimplied"], json!([["DPO", "Training"]]));
    assert_ne!(unimplied["version"], version.as_str());
}

#[test]
fn bad_definitions_and_rules_are_refused_and_nothing_is_written() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let before = fs::read(fx.root.join("library.json")).unwrap();
    let long = "x".repeat(501);
    for (args, message) in [
        (
            vec!["tags", "--define", "Agent", &long],
            "it is longer than 500 characters",
        ),
        (
            vec!["tags", "--define", "Agent", "a\u{7}b"],
            "it contains a control character",
        ),
        (
            vec!["tags", "--define", "Agent", "x", "--retire", "agent"],
            "it is named both to define and to retire",
        ),
        (
            vec!["tags", "--imply", "Agent", "Nowhere"],
            "no such tag: Nowhere",
        ),
        (
            vec!["tags", "--imply", "Agent", "agent"],
            "a tag cannot imply itself",
        ),
        (
            vec!["tags", "--imply", "Agent", "Harness"],
            "it would imply itself through its parents",
        ),
    ] {
        let output = fx.run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stderr(&output).contains(message), "{}", stderr(&output));
        assert_eq!(
            fs::read(fx.root.join("library.json")).unwrap(),
            before,
            "{args:?}"
        );
    }
    let output = fx.run(&["--json", "tags", "--define", "Agent", &long]);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "tag_definition");
}

// --- the prompt -------------------------------------------------------------

#[test]
fn an_empty_vocabulary_is_refused_before_anything_is_created() {
    let fx = Fixture::new();
    let dir = work(&fx);
    let output = fx.run(&["tag", "--prompt", dir.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("your vocabulary has no tags")
            && stderr(&output).contains("tags --define NAME"),
        "{}",
        stderr(&output)
    );
    assert!(!fx.root.exists(), "not even the archive root");
    assert!(!dir.exists());

    // Retired tags are not a vocabulary either.
    archive(&fx);
    assert_success(&fx.run(&["tags", "--create", "Old"]));
    assert_success(&fx.run(&["tags", "--retire", "Old"]));
    let output = fx.run(&["--json", "tag", "--prompt", dir.to_str().unwrap()]);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "empty_vocabulary");
    assert!(!dir.exists());
}

#[test]
fn the_prompt_carries_the_model_the_vocabulary_the_format_and_a_runnable_check() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    assert_success(&fx.run(&["tags", "--create", "Undefined"]));
    let dir = work(&fx);
    let output = fx.run(&["tag", "--prompt", dir.to_str().unwrap()]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with("wrote a prompt for 4 pages to "),
        "{}",
        stdout(&output)
    );
    assert!(stdout(&output).contains("Read prompt.md and carry it out."));
    assert!(
        stderr(&output).contains("no definition for Undefined"),
        "{}",
        stderr(&output)
    );
    common::assert_private_dir(&dir);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["pages.jsonl", "prompt.md", "vocabulary.json"]);

    let prompt = fs::read_to_string(dir.join("prompt.md")).unwrap();
    let version = json_run(&fx, &["tags"])["version"]
        .as_str()
        .unwrap()
        .to_owned();
    for needle in [
        // The owner's model.
        "**Tags are flat facets.**",
        "The meaning is in the combination",
        "**Broad tags are good.**",
        "**No exclusions.**",
        "There are no \"X, not Y\" rules",
        "**\"Substantially about\" is the bar.**",
        "A passing mention",
        "**Favour recall.**",
        // The vocabulary, defined, with its rule.
        &format!("4 tags, vocabulary version `{version}`."),
        "- **Harness**: Coding-agent harnesses and what configures them. Whenever it applies, Agent applies too; the import adds it if you leave it out.\n",
        "- **Agent**: AI agents.\n",
        "- **Undefined**: (no definition yet: use the plain meaning of the name)\n",
        // How to work.
        "in chunks of about 50 lines",
        "Write only inside this folder",
        "make no network",
        "Don't assign tags with keyword or regex scripts",
        // The format.
        r#"{"url": "https://example.com/a-page", "tags": ["First tag", "Second tag"], "source": "your-model-name"}"#,
        "A page that no tag fits gets `[]`",
        // The check.
        "--dry-run\n```",
        "It passes when it prints a line starting `valid:`",
    ] {
        assert!(prompt.contains(needle), "missing {needle:?} in\n{prompt}");
    }
    assert!(
        !prompt.contains("- `search`") && !prompt.contains("- `description`"),
        "only fields that occur are explained"
    );
    assert!(!prompt.contains('\\'), "no stray escapes");

    let vocabulary: Value =
        serde_json::from_slice(&fs::read(dir.join("vocabulary.json")).unwrap()).unwrap();
    assert_eq!(vocabulary["version"], version.as_str());
    assert_eq!(
        vocabulary["tags"][1],
        json!({"name": "Harness", "definition": "Coding-agent harnesses and what configures them.", "implies": ["Agent"]})
    );
    assert_eq!(
        vocabulary["tags"][3],
        json!({"name": "Undefined"}),
        "no null definition"
    );

    // Not forgotten, not local; titles one line each; url first.
    let pages = fs::read_to_string(dir.join("pages.jsonl")).unwrap();
    assert!(pages.starts_with(&format!("{{\"url\":\"{A}\"")), "{pages}");
    assert_eq!(
        jsonl(&dir.join("pages.jsonl")),
        [
            json!({"url": A, "title": "Agent harness"}),
            json!({"url": B, "title": "Skills for agents"}),
            json!({"url": C, "title": "A design system"}),
            json!({"url": D, "title": "Thin"}),
        ]
    );
}

/// The check the prompt hands the agent is a command that runs as written,
/// and it catches an agent that stopped early.
#[cfg(unix)]
#[test]
fn the_check_in_the_prompt_runs_as_written() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let dir = fx.home.path().join("a folder with 'quotes'");
    assert_success(&fx.run(&["tag", "--prompt", dir.to_str().unwrap()]));
    let prompt = fs::read_to_string(dir.join("prompt.md")).unwrap();
    let start = prompt.find("```sh\n").unwrap() + 6;
    let check = &prompt[start..start + prompt[start..].find("\n```").unwrap()];
    let run = || {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(check)
            .current_dir("/")
            .output()
            .unwrap()
    };
    write_answer(
        &dir.join("tags.jsonl"),
        &[answer(A, &["Harness"]), answer(B, &["Skills"])],
    );
    let output = run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert!(
        stderr(&output)
            .contains("2 pages in pages.jsonl have no line, the first is https://c.test/three"),
        "{}",
        stderr(&output)
    );
    write_answer(
        &dir.join("tags.jsonl"),
        &[
            answer(A, &["Harness"]),
            answer(B, &["Skills"]),
            answer(C, &[]),
            answer(D, &[]),
        ],
    );
    let output = run();
    assert_success(&output);
    assert!(stdout(&output).starts_with("valid: 4 pages from model-one"));
    assert!(!fx.root.join("tags").exists(), "the check stores nothing");
}

#[test]
fn only_pages_not_yet_tagged_are_in_the_prompt_unless_all() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    // A: the owner's own tag. B: an earlier import's answer, even an empty one.
    assert_success(&fx.run(&["tag", A, "--add", "Skills"]));
    let file = fx.home.path().join("earlier.jsonl");
    write_answer(&file, &[answer(B, &[])]);
    assert_success(&import(&fx, &file, &[]));
    // C: the owner only ever removed a tag; it shows none, so it is due.
    assert_success(&fx.run(&["tag", C, "--remove", "Skills"]));

    let dir = work(&fx);
    let written = json_run(&fx, &["tag", "--prompt", dir.to_str().unwrap()]);
    assert_eq!(written["prompt"]["pages"], 2);
    assert_eq!(written["prompt"]["skipped_tagged"], 1);
    assert_eq!(written["prompt"]["skipped_suggested"], 1);
    let urls: Vec<Value> = jsonl(&dir.join("pages.jsonl"))
        .into_iter()
        .map(|p| p["url"].clone())
        .collect();
    assert_eq!(urls, [C, D]);

    let all = fx.home.path().join("all");
    let written = json_run(&fx, &["tag", "--prompt", all.to_str().unwrap(), "--all"]);
    assert_eq!(written["prompt"]["pages"], 4, "forgotten stays out");

    // Retiring the owner's only tag on A makes A due again.
    assert_success(&fx.run(&["tags", "--retire", "Skills"]));
    let again = fx.home.path().join("again");
    let written = json_run(&fx, &["tag", "--prompt", again.to_str().unwrap()]);
    assert_eq!(written["prompt"]["pages"], 3);

    // Every page answered: nothing to do, and no folder.
    let file = fx.home.path().join("rest.jsonl");
    write_answer(&file, &[answer(A, &[]), answer(C, &[]), answer(D, &[])]);
    assert_success(&import(&fx, &file, &[]));
    let none = fx.home.path().join("none");
    let output = fx.run(&["tag", "--prompt", none.to_str().unwrap()]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with("nothing to tag: "),
        "{}",
        stdout(&output)
    );
    assert!(!none.exists());
}

#[test]
fn enriched_text_goes_in_and_history_only_when_asked() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    // Signals on A in an older snapshot, and on B in the newest, whose
    // referrer the owner has forgotten.
    let path = fx.root.join("snapshots/2026-01-01-000000Z/snapshot.json");
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    snapshot["tabs"][0]["history"] = json!({
        "visits": 3, "typed": 0, "last_visit": "2026-01-01T00:00:00Z",
        "search": {"term": "agent harness", "hops": 1}, "referrer": "https://ref.test/"
    });
    fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    write_snapshot(
        &fx.root,
        "2026-02-01-000000Z",
        "2026-02-01T00:00:00Z",
        &[(1, B, "Skills for agents")],
    );
    let path = fx.root.join("snapshots/2026-02-01-000000Z/snapshot.json");
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    snapshot["tabs"][0]["history"] = json!({
        "visits": 1, "typed": 0, "last_visit": "2026-02-01T00:00:00Z", "referrer": HIDDEN
    });
    fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    fs::create_dir_all(fx.root.join("pages")).unwrap();
    let readme = "word ".repeat(1000);
    let metadata = [
        json!({"url": A, "fetched_at": "2026-09-24T10:00:00Z", "status": "error", "reason": "timeout"}),
        json!({"url": A, "fetched_at": "2026-09-24T11:00:00Z", "status": "ok",
               "title": "agent HARNESS", "description": "  A harness\nfor agents. ",
               "og": {"type": "website", "site_name": "GitHub"}, "jsonld_types": ["SoftwareSourceCode"],
               "github": {"topics": ["agents", "cli"], "readme": readme}, "later": true}),
        json!({"url": B, "fetched_at": "2026-09-24T11:00:00Z", "status": "behind_login",
               "title": "Log in", "description": "Sign in to continue"}),
        json!({"url": C, "fetched_at": "2026-09-24T11:00:00Z", "status": "ok",
               "title": "Design systems", "og": {"type": "article", "description": "From og"}}),
    ];
    let mut text: String = metadata.iter().map(|m| m.to_string() + "\n").collect();
    text.push_str("{\"url\": \"torn");
    fs::write(fx.root.join("pages/metadata.jsonl"), &text).unwrap();
    let metadata_before = fs::read(fx.root.join("pages/metadata.jsonl")).unwrap();

    let dir = work(&fx);
    let output = fx.run(&["tag", "--prompt", dir.to_str().unwrap()]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("skipped 1 line unreadable in"),
        "{}",
        stderr(&output)
    );
    let pages = jsonl(&dir.join("pages.jsonl"));
    let a = &pages[0];
    assert!(a.get("page_title").is_none(), "same title in another case");
    assert_eq!(a["description"], "A harness for agents.");
    assert_eq!(a["site"], "GitHub");
    assert_eq!(a["kind"], "SoftwareSourceCode");
    assert_eq!(a["topics"], json!(["agents", "cli"]));
    assert_eq!(a["readme"].as_str().unwrap().chars().count(), 1500);
    assert_eq!(
        pages[1],
        json!({"url": B, "title": "Skills for agents"}),
        "behind a login: the tab's title only"
    );
    assert_eq!(
        pages[2],
        json!({"url": C, "title": "A design system", "page_title": "Design systems",
               "description": "From og", "kind": "article"})
    );
    assert!(
        pages
            .iter()
            .all(|p| p.get("search").is_none() && p.get("referrer").is_none())
    );
    let prompt = fs::read_to_string(dir.join("prompt.md")).unwrap();
    assert!(prompt.contains("- `readme`: ") && prompt.contains("- `topics`: "));
    assert!(!prompt.contains("- `search`"));

    let with = fx.home.path().join("with");
    let output = fx.run(&["tag", "--prompt", with.to_str().unwrap(), "--with-history"]);
    assert_success(&output);
    assert!(stdout(&output).contains("searches and referrers"));
    let pages = jsonl(&with.join("pages.jsonl"));
    assert_eq!(pages[0]["search"], "agent harness");
    assert_eq!(pages[0]["referrer"], "https://ref.test/");
    assert!(
        pages[1].get("referrer").is_none(),
        "a forgotten referrer stays out"
    );
    let prompt = fs::read_to_string(with.join("prompt.md")).unwrap();
    assert!(prompt.contains("- `search`: ") && prompt.contains("- `referrer`: "));
    assert_eq!(
        fs::read(fx.root.join("pages/metadata.jsonl")).unwrap(),
        metadata_before,
        "the prompt never writes enrich's file"
    );
}

#[test]
fn the_folder_must_be_new_or_empty_and_outside_the_archive() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let taken = work(&fx);
    fs::create_dir_all(&taken).unwrap();
    fs::write(taken.join("tags.jsonl"), "an earlier agent's work\n").unwrap();
    let file = fx.home.path().join("file");
    fs::write(&file, "x").unwrap();
    let inside = fx.root.join("work");
    for (dir, reason) in [
        (taken.clone(), "it is not empty"),
        (file, "it is not a folder"),
        (inside.clone(), "it overlaps the archive"),
        (fx.home.path().to_path_buf(), "it is not empty"),
    ] {
        let output = fx.run(&["tag", "--prompt", dir.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1), "{}", dir.display());
        assert!(stderr(&output).contains(reason), "{}", stderr(&output));
    }
    assert_eq!(
        fs::read_to_string(taken.join("tags.jsonl")).unwrap(),
        "an earlier agent's work\n"
    );
    assert!(!inside.exists());
    // An empty folder is fine.
    let empty = fx.home.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert_success(&fx.run(&["tag", "--prompt", empty.to_str().unwrap()]));
}

// --- the import -------------------------------------------------------------

#[test]
fn every_problem_is_named_by_line_and_nothing_is_stored() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    assert_success(&fx.run(&["tags", "--create", "Old"]));
    assert_success(&fx.run(&["tags", "--retire", "Old"]));
    let before = fs::read(fx.root.join("library.json")).unwrap();
    let file = fx.home.path().join("tags.jsonl");
    let lines = [
        r#"{"url": "https://a.test/one", "tags": ["Harness"], "source": "m"}"#,
        "not json",
        r#"{"url": "https://b.test/two", "tags": ["Skills"], "source": "m", "reason": "x"}"#,
        r#"{"url": "https://nowhere.test/", "tags": [], "source": "m"}"#,
        r#"{"url": "http://localhost:3000/dev", "tags": [], "source": "m"}"#,
        r#"{"url": "https://a.test/one", "tags": [], "source": "m"}"#,
        r#"{"url": "https://c.test/three", "tags": ["Invented", "Old", "  "], "source": "m"}"#,
        r#"{"url": "https://d.test/four", "tags": "Harness", "source": "m"}"#,
        r#"{"url": "https://hidden.test/page", "tags": ["invented"], "source": "other"}"#,
        "",
    ];
    fs::write(&file, lines.join("\n")).unwrap();
    let output = import(&fx, &file, &[]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    for needle in [
        "has 10 problems; nothing stored: ",
        "line 2: expected ident",
        "line 3: unknown field `reason`",
        "line 4: not in your library: https://nowhere.test/",
        "line 5: not in your library: http://localhost:3000/dev",
        "line 6: https://a.test/one is already on line 1; give each page one line",
        "line 7: cannot use \"  \" as a tag: it is empty",
        "line 8: invalid type: string \"Harness\", expected a sequence",
        "line 9: source \"other\" differs from \"m\" on line 1; one import is one source",
        "no such tag: Invented (line 7, 9); use the vocabulary's names, or pass --accept-new to create it",
        "no such tag: Old (line 7)",
    ] {
        assert!(message.contains(needle), "missing {needle:?} in {message}");
    }
    assert_eq!(message.lines().count(), 1, "one line: {message}");
    assert!(!fx.root.join("tags").exists());
    assert_eq!(fs::read(fx.root.join("library.json")).unwrap(), before);

    let output = fx.run(&["--json", "tag", "--import", file.to_str().unwrap()]);
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "import");
    assert_eq!(
        value["error"]["detail"].as_str().unwrap().lines().count(),
        10
    );

    for (text, needle) in [
        ("", "there are no lines"),
        (
            &format!(
                "{}\n{}\n",
                json!({"url": A, "tags": []}),
                json!({"url": B, "tags": []})
            ),
            "lines have no source (line 1, 2); add \"source\" with the model's name, or pass --source NAME",
        ),
    ] {
        fs::write(&file, text).unwrap();
        let output = import(&fx, &file, &[]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains(needle), "{}", stderr(&output));
    }
    let output = import(&fx, &file, &["--source", " "]);
    assert!(stderr(&output).contains("--source \" \": it is empty"));
    assert!(!fx.root.join("tags").exists());
}

#[test]
fn an_import_stores_one_line_per_page_with_its_source_date_and_version() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let before = fingerprint(&fx.root.join("snapshots"));
    let version = json_run(&fx, &["tags"])["version"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = fx.home.path().join("tags.jsonl");
    write_answer(
        &file,
        &[
            json!({"url": A, "tags": ["harness", "Harness"], "source": " model  one ", "note": "a hard call"}),
            json!({"url": B, "tags": ["Skills", "Agent"], "source": "MODEL ONE"}),
            json!({"url": HIDDEN, "tags": [], "source": "model one"}),
        ],
    );

    // A dry run says what it would store, and stores nothing.
    let output = import(&fx, &file, &["--dry-run"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        format!(
            "valid: 3 pages from model one: 2 with tags (4 suggestions, 1 from parent rules), 1 with none. Nothing stored: {} is ready to import\n",
            file.display()
        )
    );
    assert!(!fx.root.join("tags").exists());

    let output = import(&fx, &file, &[]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with(
            "imported suggestions for 3 pages from model one: 2 with tags (4 suggestions, 1 from parent rules), 1 with none. Stored in "
        ),
        "{}",
        stdout(&output)
    );
    let lines = stored(&fx);
    assert_eq!(lines.len(), 3);
    let imported_at = lines[0]["imported_at"].as_str().unwrap().to_owned();
    assert!(imported_at.ends_with('Z') && !imported_at.contains('.'));
    assert_eq!(
        lines[0],
        json!({"url": A, "tags": ["Agent", "Harness"], "source": "model one",
               "imported_at": imported_at, "vocabulary_version": version}),
        "vocabulary spelling, deduplicated, parents added, no note"
    );
    assert_eq!(lines[1]["tags"], json!(["Agent", "Skills"]));
    assert_eq!(lines[2]["tags"], json!([]), "an empty answer is kept");
    #[cfg(unix)]
    common::assert_private_dir(&fx.root.join("tags"));
    assert_eq!(
        fingerprint(&fx.root.join("snapshots")),
        before,
        "an import never touches a snapshot"
    );
    assert!(
        state(&fx).get("tags").is_none(),
        "suggestions are not the owner's decisions"
    );

    // The same answer again adds nothing; a changed one is appended.
    let again = json_run(&fx, &["tag", "--import", file.to_str().unwrap()]);
    assert_eq!(again["imported"]["unchanged"], 3);
    assert_eq!(stored(&fx).len(), 3);
    write_answer(
        &file,
        &[json!({"url": A, "tags": ["Skills"], "source": "model one"})],
    );
    assert_success(&import(&fx, &file, &[]));
    assert_eq!(stored(&fx).len(), 4);

    // `--source` names another source, whatever the lines say.
    write_answer(&file, &[json!({"url": A, "tags": ["Skills"]})]);
    let second = json_run(
        &fx,
        &[
            "tag",
            "--import",
            file.to_str().unwrap(),
            "--source",
            "model-two",
        ],
    );
    assert_eq!(second["imported"]["source"], "model-two");
    assert_eq!(stored(&fx)[4]["source"], "model-two");
}

#[test]
fn accept_new_creates_or_brings_back_tags_and_parent_rules_apply_to_them() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    assert_success(&fx.run(&["tags", "--create", "Old", "--imply", "Old", "Skills"]));
    assert_success(&fx.run(&["tags", "--retire", "Old"]));
    let file = fx.home.path().join("tags.jsonl");
    write_answer(&file, &[answer(A, &["Invented", "old"])]);
    let dry = json_run(
        &fx,
        &[
            "tag",
            "--import",
            file.to_str().unwrap(),
            "--accept-new",
            "--dry-run",
        ],
    );
    assert_eq!(dry["imported"]["created"], json!(["Invented"]));
    assert_eq!(dry["imported"]["revived"], json!(["Old"]));
    assert!(state(&fx)["vocabulary"].get("Invented").is_none());
    assert!(state(&fx)["vocabulary"]["Old"]["retired_at"].is_string());

    let output = import(&fx, &file, &["--accept-new"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("; new tags: Invented; back from retirement: Old."),
        "{}",
        stdout(&output)
    );
    let written = state(&fx);
    assert!(written["vocabulary"]["Invented"].is_object());
    assert!(written["vocabulary"]["Old"].get("retired_at").is_none());
    assert_eq!(stored(&fx)[0]["tags"], json!(["Invented", "Old", "Skills"]));
}

#[test]
fn missing_pages_need_partial_and_a_changed_vocabulary_keeps_the_prompts_version() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let dir = work(&fx);
    assert_success(&fx.run(&["tag", "--prompt", dir.to_str().unwrap()]));
    let prompted = json_run(&fx, &["tags"])["version"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = dir.join("tags.jsonl");
    write_answer(&file, &[answer(A, &["Harness"])]);
    let output = import(&fx, &file, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("3 pages in pages.jsonl have no line, the first is https://b.test/two; tag every page, or pass --partial"));

    assert_success(&fx.run(&["tags", "--define", "Skills", "Any skills at all."]));
    let output = import(&fx, &file, &["--partial"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("; 3 pages from the prompt left out (--partial)"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains(&format!(
            "your vocabulary has changed since this prompt was written, so these are recorded under its version {prompted}"
        )),
        "{}",
        stdout(&output)
    );
    assert_eq!(stored(&fx)[0]["vocabulary_version"], prompted.as_str());
}

// --- what the library shows -------------------------------------------------

#[test]
fn export_shows_undecided_suggestions_with_their_sources() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let one = fx.home.path().join("one.jsonl");
    write_answer(
        &one,
        &[
            json!({"url": A, "tags": ["Harness"], "source": "model-one"}),
            json!({"url": B, "tags": ["Skills"], "source": "model-one"}),
            json!({"url": HIDDEN, "tags": ["Skills"], "source": "model-one"}),
        ],
    );
    assert_success(&import(&fx, &one, &[]));
    let two = fx.home.path().join("two.jsonl");
    write_answer(
        &two,
        &[json!({"url": A, "tags": ["Agent", "Skills"], "source": "Model-Two"})],
    );
    assert_success(&import(&fx, &two, &[]));

    let library = exported(&fx);
    assert_eq!(
        page(&library, A),
        json!({"url": A, "title": "Agent harness", "domain": "a.test", "tags": [],
        "suggested": [
            {"name": "Agent", "sources": ["model-one", "Model-Two"]},
            {"name": "Harness", "sources": ["model-one"]},
            {"name": "Skills", "sources": ["Model-Two"]},
        ]})
    );
    assert_eq!(page(&library, C)["suggested"], json!([]), "always present");
    assert!(
        !library["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["url"] == HIDDEN),
        "export leaves forgotten pages out"
    );

    // The owner's decisions win: an add confirms, a remove dismisses, a
    // retired tag is not shown, and a source's newer answer replaces its older.
    assert_success(&fx.run(&["tag", A, "--add", "harness", "--remove", "Skills"]));
    assert_success(&fx.run(&["tags", "--retire", "Skills"]));
    let library = exported(&fx);
    assert_eq!(page(&library, A)["tags"], json!(["Harness"]));
    assert_eq!(
        page(&library, A)["suggested"],
        json!([{"name": "Agent", "sources": ["model-one", "Model-Two"]}])
    );
    assert_eq!(page(&library, B)["suggested"], json!([]));
    write_answer(
        &one,
        &[json!({"url": A, "tags": [], "source": "MODEL-ONE"})],
    );
    assert_success(&import(&fx, &one, &["--partial"]));
    assert_eq!(
        page(&exported(&fx), A)["suggested"],
        json!([{"name": "Agent", "sources": ["Model-Two"]}])
    );
}

#[test]
fn a_damaged_suggestions_file_is_refused_like_damaged_state() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    fs::create_dir_all(fx.root.join("tags")).unwrap();
    let body = b"{\"url\": \"https://a.test/one\", \"tags\": \"x\"}\n";
    fs::write(fx.root.join("tags/suggested.jsonl"), body).unwrap();
    let file = fx.home.path().join("tags.jsonl");
    write_answer(&file, &[answer(A, &[])]);
    let dir = work(&fx);
    for args in [
        vec!["export"],
        vec!["tag", "--prompt", dir.to_str().unwrap()],
        vec!["tag", "--import", file.to_str().unwrap()],
    ] {
        let output = fx.run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains("suggested.jsonl: line 1: "),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(
        fs::read(fx.root.join("tags/suggested.jsonl")).unwrap(),
        body
    );
}

// --- the lock ---------------------------------------------------------------

#[test]
fn an_import_waits_for_the_lock_and_a_dry_run_does_not() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let file = fx.home.path().join("tags.jsonl");
    write_answer(&file, &[answer(A, &["Invented"])]);
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(fx.root.join("lock"))
        .unwrap();
    lock.lock().unwrap();

    // A dry run is what an agent repeats; it never waits.
    assert_success(&import(&fx, &file, &["--dry-run", "--accept-new"]));

    let mut child = fx
        .command()
        .args(["tag", "--import", file.to_str().unwrap(), "--accept-new"])
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
    assert!(!fx.root.join("tags").exists());
    // Another writer's change, published while we still hold the lock.
    let mut written = state(&fx);
    written["forgotten"] = json!([B, HIDDEN]);
    fs::write(
        fx.root.join("library.json"),
        serde_json::to_vec(&written).unwrap(),
    )
    .unwrap();
    drop(lock);
    assert_success(&child.wait_with_output().unwrap());
    reader.join().unwrap();
    let after = state(&fx);
    assert_eq!(
        after["forgotten"],
        json!([B, HIDDEN]),
        "read after the lock"
    );
    assert!(after["vocabulary"]["Invented"].is_object());
    assert_eq!(stored(&fx).len(), 1);
}

#[test]
fn two_concurrent_imports_both_survive() {
    let fx = Fixture::new();
    archive(&fx);
    vocabulary(&fx);
    let files: Vec<PathBuf> = ["one", "two"]
        .iter()
        .map(|name| {
            let file = fx.home.path().join(format!("{name}.jsonl"));
            write_answer(
                &file,
                &[json!({"url": A, "tags": ["Skills"], "source": name})],
            );
            file
        })
        .collect();
    let children: Vec<_> = files
        .iter()
        .map(|file| {
            fx.command()
                .args(["tag", "--import", file.to_str().unwrap()])
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        assert_success(&child.wait_with_output().unwrap());
    }
    let mut sources: Vec<Value> = stored(&fx)
        .into_iter()
        .map(|l| l["source"].clone())
        .collect();
    sources.sort_by_key(ToString::to_string);
    assert_eq!(sources, ["one", "two"]);
}
