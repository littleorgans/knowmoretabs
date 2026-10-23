//! `add --file`, `search` and `read` through the real binary: local
//! documents kept by reference, a committed file in a synthetic git
//! repository and loose files on a shelf, found again by what they say,
//! with no copy and no local path in the archive, and nothing from the
//! owner's snapshots or History ever returned. Every repository, remote,
//! title and text here is invented.

mod common;

#[cfg(unix)]
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::add::{intake, lines};
use common::model_service::{Service, detailed_rank, json_fields, key_file, proven, request_body};
use common::pdf_builder::pdf;
use common::serve::{Server, kept_text};
use common::{Fixture, assert_success, stderr, stdout, write_snapshot};
use serde_json::{Value, json};

const REMOTE: &str = "git@example.test:o/r.git";
const NOTE: &str = "kmt-git:example.test/o/r//notes/evals.md";
/// No result at all.
const NONE: [&str; 0] = [];
const NOTE_TEXT: &str = "# Airship evals\n\nThe quokka benchmark measures zeppelin timetables.\n";

/// A home with an archive root, a git checkout of the synthetic remote
/// holding one committed note, and a checkouts file naming a shelf.
struct Desk {
    fx: Fixture,
    repo: PathBuf,
    shelf: PathBuf,
    checkouts: PathBuf,
}

impl Desk {
    fn new() -> Self {
        let fx = Fixture::new();
        let repo = fx.home.path().join("research");
        std::fs::create_dir_all(repo.join("notes")).unwrap();
        let desk = Self {
            repo,
            shelf: fx.home.path().join("shelf"),
            checkouts: fx.home.path().join("checkouts.json"),
            fx,
        };
        desk.git(&["init", "--quiet"]);
        desk.git(&["remote", "add", "origin", REMOTE]);
        desk.commit("notes/evals.md", NOTE_TEXT.as_bytes());
        desk.set_shelf(true);
        desk
    }

    /// Git in the checkout, with the fixture's home and nothing of the
    /// machine's configuration.
    fn git(&self, args: &[&str]) {
        let mut git = Command::new("git");
        common::point_home_at(&mut git, self.fx.home.path());
        let status = git
            .arg("-C")
            .arg(&self.repo)
            .args([
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=s@example.test",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn commit(&self, path: &str, bytes: &[u8]) {
        std::fs::write(self.repo.join(path), bytes).unwrap();
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", "synthetic"]);
    }

    fn set_shelf(&self, on: bool) {
        let mut file = json!({"schema_version": 1, "repos": {}});
        if on {
            file["shelf"] = json!(self.shelf);
        }
        std::fs::write(&self.checkouts, file.to_string()).unwrap();
    }

    fn checkouts(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.checkouts).unwrap()).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.fx
            .command()
            .arg("--checkouts")
            .arg(&self.checkouts)
            .args(args)
            .output()
            .unwrap()
    }

    fn add(&self, file: &Path) -> Output {
        self.run(&["add", "--json", "--file", file.to_str().unwrap()])
    }

    fn search(&self, query: &str) -> Value {
        json_answer(self.run(&["search", "--json", query]))
    }

    #[cfg(unix)]
    fn ranked(&self, service: &Service, args: &[&str]) -> Output {
        let key = key_file(&self.fx);
        let port = service.port.to_string();
        self.run(
            &[
                args,
                &[
                    "--model-port",
                    &port,
                    "--model-secret-file",
                    key.to_str().unwrap(),
                ],
            ]
            .concat(),
        )
    }

    /// A file outside the checkout.
    fn loose(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = self.fx.home.path().join("downloads");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn pins(&self) -> Vec<Value> {
        std::fs::read_to_string(self.fx.root.join("pages").join("sources.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

/// Every byte of every file under `dir`, as text.
fn all_text(dir: &Path) -> String {
    common::fingerprint(dir)
        .into_iter()
        .map(|(_, bytes, _)| String::from_utf8_lossy(&bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn result<'a>(found: &'a Value, url: &str) -> &'a Value {
    found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["url"] == url)
        .unwrap_or_else(|| panic!("{url} not in {found}"))
}

fn urls(found: &Value) -> Vec<&str> {
    found["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["url"].as_str().unwrap())
        .collect()
}

fn blob_key(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("kmt-blob:sha256-{:x}", Sha256::digest(bytes))
}

#[test]
fn a_committed_note_and_loose_files_join_by_reference_and_are_found_by_their_text() {
    let desk = Desk::new();
    let output = desk.add(&desk.repo.join("notes").join("evals.md"));
    assert_success(&output);
    let done = lines(&output).pop().unwrap();
    assert_eq!(done["url"], NOTE);
    assert_eq!(done["value"], "added");
    assert_eq!(done["pinned"], true);
    assert_eq!(done["source"]["repo"], "example.test/o/r");
    assert_eq!(done["source"]["path"], "notes/evals.md");
    assert_eq!(done["source"]["blob"].as_str().unwrap().len(), 40);
    assert_eq!(done["source"]["commit"].as_str().unwrap().len(), 40);
    assert_eq!(intake(&desk.fx)[0]["title"], "Airship evals");
    // The checkout is remembered for later, and never printed.
    let checkout = std::fs::canonicalize(&desk.repo).unwrap();
    assert_eq!(
        desk.checkouts()["repos"]["example.test/o/r"],
        json!(checkout)
    );
    assert_eq!(desk.checkouts()["shelf"], json!(desk.shelf));
    assert!(!stdout(&output).contains(desk.fx.home.path().to_str().unwrap()));

    let paper = pdf(
        &[
            &["Quokka migration paper."],
            &["Zeppelin routes, page two."],
        ],
        Some("A paper"),
    );
    let output = desk.add(&desk.loose("paper.pdf", &paper));
    assert_success(&output);
    let pdf_key = blob_key(&paper);
    assert_eq!(lines(&output).pop().unwrap()["url"], pdf_key);
    let sha = &pdf_key["kmt-blob:sha256-".len()..];
    assert_eq!(
        std::fs::read(desk.shelf.join(format!("{sha}.pdf"))).unwrap(),
        paper
    );

    let text = b"Plain quokka notes, no heading.\n";
    assert_success(&desk.add(&desk.loose("plain.txt", text)));
    assert_eq!(intake(&desk.fx)[2]["title"], "plain.txt");

    // No copy of any text and no local path is in the archive.
    let archive = all_text(&desk.fx.root);
    for never in [
        "quokka",
        "Quokka",
        desk.fx.home.path().to_str().unwrap(),
        "research",
    ] {
        assert!(!archive.contains(never), "the archive holds {never:?}");
    }

    let found = desk.search("quokka");
    assert_eq!(found["schema_version"], 1);
    assert_eq!(found["ranking"], "keyword");
    assert_eq!(
        urls(&found),
        [NOTE, pdf_key.as_str(), blob_key(text).as_str()]
    );
    let note = result(&found, NOTE);
    assert_eq!(note["kind"], "git");
    assert_eq!(note["state"], "fresh");
    assert_eq!(note["why"], Value::Null);
    assert_eq!(note["matched"], json!(["text"]));
    assert!(
        note["snippet"]
            .as_str()
            .unwrap()
            .contains("quokka benchmark")
    );
    assert_eq!(result(&found, &pdf_key)["kind"], "blob");
    assert_eq!(result(&found, &pdf_key)["title"], "A paper");
    assert!(
        !found
            .to_string()
            .contains(desk.fx.home.path().to_str().unwrap())
    );
    assert_eq!(urls(&desk.search("evals airship")), [NOTE]);
    assert_eq!(urls(&desk.search("quokka absent-word")), NONE);

    let read = desk.run(&["read", NOTE]);
    assert_success(&read);
    assert_eq!(stdout(&read), NOTE_TEXT);
    let read = desk.run(&["read", &pdf_key]);
    assert_success(&read);
    assert_eq!(
        stdout(&read),
        "Quokka migration paper.\n\nZeppelin routes, page two.\n"
    );
    let read = desk.run(&["--json", "read", NOTE]);
    assert_success(&read);
    let read: Value = serde_json::from_str(&stdout(&read)).unwrap();
    assert_eq!(
        (read["state"].as_str(), read["text"].as_str()),
        (Some("fresh"), Some(NOTE_TEXT))
    );

    // Added again, unchanged: known, and no second pin.
    let output = desk.add(&desk.repo.join("notes").join("evals.md"));
    assert_success(&output);
    assert_eq!(lines(&output).pop().unwrap()["value"], "known");
    assert_eq!(lines(&output).pop().unwrap()["pinned"], false);
    assert_eq!(desk.pins().len(), 3);
}

#[test]
fn a_file_with_changes_or_not_committed_is_refused_and_nothing_is_written() {
    let desk = Desk::new();
    std::fs::write(desk.repo.join("notes").join("evals.md"), "edited\n").unwrap();
    std::fs::write(desk.repo.join("notes").join("new.md"), "# New\n").unwrap();
    for (file, reason) in [
        (
            "evals.md",
            "it has changes not committed; commit them first",
        ),
        ("new.md", "it is not committed; commit it first"),
    ] {
        let output = desk.run(&[
            "add",
            "--file",
            desk.repo.join("notes").join(file).to_str().unwrap(),
        ]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(
            stderr(&output).trim(),
            format!("knowmoretabs: not added: {file}: {reason}; nothing changed")
        );
        assert!(!stderr(&output).contains(desk.fx.home.path().to_str().unwrap()));
    }
    let output = desk.run(&[
        "--json",
        "add",
        "--file",
        desk.repo.join("notes").join("new.md").to_str().unwrap(),
    ]);
    assert_eq!(lines(&output)[0]["error"]["kind"], "source_file");
    assert!(!desk.fx.root.exists(), "nothing written");
    assert_eq!(desk.checkouts()["repos"], json!({}));
}

#[test]
fn a_loose_file_without_a_shelf_is_refused_and_one_of_another_kind_too() {
    let desk = Desk::new();
    desk.set_shelf(false);
    let output = desk.run(&[
        "add",
        "--file",
        desk.loose("a.md", b"# A\n").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no shelf is set for loose files"),
        "{}",
        stderr(&output)
    );
    let output = desk.run(&[
        "add",
        "--file",
        desk.loose("a.html", b"<p>x</p>").to_str().unwrap(),
    ]);
    assert!(
        stderr(&output).contains("not markdown, text or PDF"),
        "{}",
        stderr(&output)
    );
    let output = desk.run(&[
        "add",
        "--file",
        desk.loose("bad.pdf", b"%PDF-1.5\nno").to_str().unwrap(),
    ]);
    assert!(
        stderr(&output).contains("unreadable PDF"),
        "{}",
        stderr(&output)
    );
    assert!(!desk.fx.root.exists(), "nothing written");
    assert!(!desk.shelf.exists());
}

#[test]
fn pasted_text_lands_on_the_shelf_as_markdown() {
    let desk = Desk::new();
    let text = b"# Pasted\nA quokka from the clipboard.\n";
    let mut child = desk
        .fx
        .command()
        .arg("--checkouts")
        .arg(&desk.checkouts)
        .args(["add", "--json", "--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), text).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_success(&output);
    let key = blob_key(text);
    assert_eq!(lines(&output).pop().unwrap()["url"], key);
    let sha = &key["kmt-blob:sha256-".len()..];
    assert_eq!(
        std::fs::read(desk.shelf.join(format!("{sha}.md"))).unwrap(),
        text
    );
    assert_eq!(intake(&desk.fx)[0]["title"], "Pasted");
    assert_eq!(urls(&desk.search("clipboard")), [key.as_str()]);
}

#[test]
fn a_stale_pin_is_searched_as_added_and_an_unresolved_one_says_why() {
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    let paper = b"A loose quokka note.\n";
    let loose = blob_key(paper);
    assert_success(&desk.add(&desk.loose("loose.md", paper)));
    desk.commit(
        "notes/evals.md",
        b"# Airship evals\n\nRewritten about ferries.\n",
    );

    let found = desk.search("quokka");
    let note = result(&found, NOTE);
    assert_eq!(note["state"], "stale");
    assert_eq!(note["why"], "changed at HEAD since it was added");
    assert_eq!(
        urls(&desk.search("ferries")),
        NONE,
        "HEAD's text is not the pin's"
    );
    let read = desk.run(&["read", NOTE]);
    assert_success(&read);
    assert_eq!(stdout(&read), NOTE_TEXT);
    assert!(stderr(&read).contains("is stale"), "{}", stderr(&read));

    // Added again, the new bytes are pinned and fresh.
    let output = desk.add(&desk.repo.join("notes").join("evals.md"));
    assert_eq!(lines(&output).pop().unwrap()["pinned"], true);
    assert_eq!(result(&desk.search("ferries"), NOTE)["state"], "fresh");

    desk.git(&["rm", "--quiet", "notes/evals.md"]);
    desk.git(&["commit", "--quiet", "-m", "gone"]);
    assert_eq!(
        result(&desk.search("airship"), NOTE)["why"],
        "gone from HEAD since it was added"
    );

    // No checkout and no shelf: still found by title, text null.
    desk.set_shelf(false);
    let found = desk.search("airship");
    let note = result(&found, NOTE);
    assert_eq!(note["state"], "unresolved");
    assert_eq!(
        note["why"],
        "no checkout of its repository in checkouts.json"
    );
    assert_eq!(note["snippet"], Value::Null);
    assert_eq!(note["matched"], json!(["title"]));
    assert_eq!(
        result(&desk.search("loose"), &loose)["why"],
        "no shelf in checkouts.json"
    );
    let read = desk.run(&["read", NOTE]);
    assert_eq!(read.status.code(), Some(1));
    assert_eq!(
        stderr(&read).trim(),
        format!(
            "knowmoretabs: cannot read {NOTE}: no checkout of its repository in checkouts.json"
        )
    );
}

#[test]
fn search_never_returns_history_sessions_or_forgotten_pages() {
    const MARKER: &str = "xyzzymarker";
    let desk = Desk::new();
    write_snapshot(
        &desk.fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[(1, "https://snap.test/xyzzymarker", "A xyzzymarker tab")],
    );
    let history = json!({"schema_version": 1, "pages": {"https://hist.test/": {"search": MARKER}}});
    std::fs::create_dir_all(desk.fx.root.join("pages")).unwrap();
    std::fs::write(
        desk.fx.root.join("pages").join("history.json"),
        history.to_string(),
    )
    .unwrap();
    assert_success(&desk.run(&[
        "add",
        "https://web.test/plain",
        "--title",
        "Plain",
        "--no-content",
    ]));
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));

    let found = desk.search(MARKER);
    assert_eq!(urls(&found), NONE, "{found}");
    for field in [
        "2026-01-01-000000Z",
        "visits",
        "referrer",
        "snap.test",
        "hist.test",
    ] {
        assert!(!found.to_string().contains(field), "{field} in {found}");
    }
    let web = desk.search("plain");
    assert_eq!(urls(&web), ["https://web.test/plain"]);
    assert_eq!(result(&web, "https://web.test/plain")["kind"], "web");
    assert_eq!(result(&web, "https://web.test/plain")["state"], Value::Null);

    assert_success(&desk.run(&["forget", NOTE]));
    assert_eq!(urls(&desk.search("quokka")), NONE);
    // Read answers what search does: a page known only from a snapshot is
    // refused as not added, and so is a key nothing names.
    for key in ["https://snap.test/xyzzymarker", "kmt-blob:sha256-00"] {
        let read = desk.run(&["read", key]);
        assert_eq!(read.status.code(), Some(1));
        assert_eq!(stdout(&read), "");
        assert_eq!(
            stderr(&read).trim(),
            format!("knowmoretabs: cannot read {key}: not a page you added; search lists them")
        );
    }
}

#[test]
fn read_refuses_forgotten_sources_until_restored_without_changing_the_archive() {
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    let bytes = b"# Loose note\nA quokka on the shelf.\n";
    let loose = blob_key(bytes);
    assert_success(&desk.add(&desk.loose("loose.md", bytes)));

    for key in [NOTE, loose.as_str()] {
        assert_success(&desk.run(&["forget", key]));
        let before = common::fingerprint(&desk.fx.root);
        for args in [vec!["read", key], vec!["--json", "read", key]] {
            let output = desk.run(&args);
            assert_eq!(output.status.code(), Some(1));
            if args[0] == "--json" {
                let error: Value = serde_json::from_str(&stdout(&output)).unwrap();
                assert_eq!(error["error"]["kind"], "source_read");
                assert!(
                    error["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("forgotten")
                );
                assert!(error.get("text").is_none());
            } else {
                assert_eq!(stdout(&output), "");
                assert!(stderr(&output).contains("forgotten"));
            }
        }
        assert_eq!(common::fingerprint(&desk.fx.root), before);
        assert_success(&desk.run(&["restore", key]));
        let read = desk.run(&["read", key]);
        assert_success(&read);
        assert_eq!(
            stdout(&read),
            if key == NOTE {
                NOTE_TEXT
            } else {
                std::str::from_utf8(bytes).unwrap()
            }
        );
    }
}

#[test]
fn the_drawer_shows_a_documents_text_as_read_prints_it() {
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    let loose = b"# Loose note\nA quokka on the shelf.\n";
    assert_success(&desk.add(&desk.loose("loose.md", loose)));
    let paper = pdf(&[&["Quokka migration paper."]], Some("A paper"));
    assert_success(&desk.add(&desk.loose("paper.pdf", &paper)));
    let server = Server::start_with(&desk.fx, &["--checkouts", desk.checkouts.to_str().unwrap()]);
    let before = common::fingerprint(&desk.fx.root);
    for key in [NOTE, &blob_key(loose), &blob_key(&paper)] {
        let read = desk.run(&["read", key]);
        assert_success(&read);
        let drawer = server.served(key);
        assert_eq!(drawer["text"], stdout(&read), "{key}");
        assert_eq!(
            (&drawer["content"], &drawer["image"]),
            (&Value::Null, &Value::Null)
        );
    }

    // Stale, the drawer shows the pinned text, as read does.
    desk.commit(
        "notes/evals.md",
        b"# Airship evals\n\nRewritten about ferries.\n",
    );
    assert_eq!(server.served(NOTE)["text"], NOTE_TEXT);
    // Its checkout gone, the page is still answered, with no text.
    std::fs::rename(&desk.repo, desk.fx.home.path().join("moved")).unwrap();
    assert_eq!(server.served(NOTE)["text"], Value::Null);
    assert_eq!(common::fingerprint(&desk.fx.root), before);
}

#[test]
fn read_answers_added_web_pages_and_several_keys_as_json_lines() {
    const WEB: &str = "https://web.test/kept";
    const WEB_TEXT: &str = "# Kept\n\nA quokka timetable, as content kept it.\n";
    let desk = Desk::new();
    assert_success(&desk.run(&["add", WEB, "--title", "Kept", "--no-content"]));
    let line = json!({"schema_version": 1, "url": WEB, "attempted_at": "2026-10-07T09:00:00Z",
        "status": "ok", "tier": "web", "chars": WEB_TEXT.len()});
    kept_text(&desk.fx, WEB, &line, Some(WEB_TEXT));
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    let paper = pdf(&[&["Quokka migration paper."]], Some("A paper"));
    let pdf_key = blob_key(&paper);
    assert_success(&desk.add(&desk.loose("paper.pdf", &paper)));
    let gone = b"# Gone\nA forgotten quokka.\n";
    let forgotten = blob_key(gone);
    assert_success(&desk.add(&desk.loose("gone.md", gone)));
    assert_success(&desk.run(&["forget", &forgotten]));
    let server = Server::start_with(&desk.fx, &["--checkouts", desk.checkouts.to_str().unwrap()]);
    let before = common::fingerprint(&desk.fx.root);

    let read = desk.run(&["read", WEB]);
    assert_success(&read);
    assert_eq!(stdout(&read), WEB_TEXT, "the body, without front matter");

    let unknown = "https://unknown.test/";
    let keys = [WEB, NOTE, &pdf_key, unknown, &forgotten];
    let mut args = vec!["--json", "read", "--"];
    args.extend(keys);
    let output = desk.run(&args);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let read = lines(&output);
    assert_eq!(
        read.iter().map(|line| &line["url"]).collect::<Vec<_>>(),
        keys
    );
    for (line, kind) in read.iter().zip(["web", "git", "blob"]) {
        assert_eq!(line["schema_version"], 1);
        assert_eq!(line["kind"], kind);
        assert_eq!(
            line["text"],
            server.served(line["url"].as_str().unwrap())["text"]
        );
    }
    assert_eq!(read[0]["text"], WEB_TEXT);
    assert_eq!(
        (&read[0]["state"], &read[0]["why"], &read[0]["source"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    assert_eq!(
        (&read[1]["state"], &read[1]["title"]),
        (&json!("fresh"), &json!("Airship evals"))
    );
    assert_eq!(read[1]["source"]["path"], "notes/evals.md");
    assert_eq!(read[2]["title"], "A paper");
    for (line, reason) in read[3..].iter().zip([
        "not a page you added; search lists them",
        "it is forgotten; restore it first",
    ]) {
        assert_eq!(line["schema_version"], 1);
        assert_eq!(line["error"]["kind"], "source_read");
        let url = line["url"].as_str().unwrap();
        assert_eq!(
            line["error"]["message"],
            format!("cannot read {url}: {reason}")
        );
        assert!(line.get("text").is_none());
    }

    let output = desk.run(&["--json", "read", WEB, &pdf_key]);
    assert_success(&output);
    assert_eq!(lines(&output).len(), 2);

    let output = desk.run(&["read", WEB, NOTE]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output).trim(),
        "knowmoretabs: several keys need --json, one line each; nothing read"
    );
    assert_eq!(common::fingerprint(&desk.fx.root), before);
}

#[test]
fn search_on_a_missing_archive_finds_nothing_and_creates_nothing() {
    let desk = Desk::new();
    assert_eq!(urls(&desk.search("quokka")), NONE);
    assert!(!desk.fx.root.exists());
}

#[test]
fn search_skips_a_reader_failure_and_warns_without_losing_readable_pages() {
    const WEB: &str = "https://readable.test/airship";
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    assert_success(&desk.run(&["add", WEB, "--title", "Airship", "--no-content"]));
    std::fs::write(&desk.checkouts, "not json").unwrap();
    let before = common::fingerprint(&desk.fx.root);

    let output = desk.run(&["search", "--json", "airship"]);
    assert_success(&output);
    let found: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(found["schema_version"], 1);
    assert_eq!(urls(&found), [WEB]);
    let warnings = found["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["url"], NOTE);
    assert_eq!(warnings[0]["error"]["kind"], "checkouts");
    let message = warnings[0]["error"]["message"].as_str().unwrap();
    assert!(message.contains("cannot use checkouts.json"));
    assert!(stderr(&output).contains(NOTE));
    assert!(stderr(&output).contains(message));

    let quiet = desk.run(&["search", "--json", "--quiet", "airship"]);
    assert_success(&quiet);
    assert_eq!(stderr(&quiet), "");
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&quiet)).unwrap(),
        found
    );

    let human = desk.run(&["search", "airship"]);
    assert_success(&human);
    assert!(stdout(&human).contains(WEB));
    assert!(stderr(&human).contains(message));
    assert_eq!(common::fingerprint(&desk.fx.root), before);
}

#[test]
fn search_fails_when_forgotten_state_cannot_be_read() {
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    std::fs::create_dir(desk.fx.root.join("library.json")).unwrap();
    let before = common::fingerprint(&desk.fx.root);

    let output = desk.run(&["search", "--json", "airship"]);
    assert_eq!(output.status.code(), Some(1));
    let failed: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("read library state")
    );
    assert!(failed.get("results").is_none());
    assert_eq!(common::fingerprint(&desk.fx.root), before);
}

#[cfg(unix)]
#[test]
fn search_modes_control_model_ranking_and_default_is_combined() {
    let desk = Desk::new();
    assert_success(&desk.add(&desk.repo.join("notes").join("evals.md")));
    let paper = b"# Airship paper\nOn zeppelins.\n";
    let loose = blob_key(paper);
    assert_success(&desk.add(&desk.loose("paper.md", paper)));
    let unindexed = b"# Late quokka\n";
    assert_success(&desk.add(&desk.loose("late.md", unindexed)));
    let keys = detailed_rank(&[&loose, NOTE]).to_string();
    let fields = json_fields(&desk.fx, &keys);
    let service = Service::start(move |request| Some(proven(request, "200 OK", &fields, &keys)));
    let key = key_file(&desk.fx);
    let port = service.port.to_string();
    let args = [
        "search",
        "--json",
        "quokka",
        "--model-port",
        &port,
        "--model-secret-file",
        key.to_str().unwrap(),
    ];
    let keyword = desk.run(&[args.as_slice(), &["--mode", "keyword"]].concat());
    assert_success(&keyword);
    let keyword: Value = serde_json::from_str(&stdout(&keyword)).unwrap();
    assert_eq!(keyword["ranking"], "keyword");
    assert_eq!(urls(&keyword), [NOTE, blob_key(unindexed).as_str()]);
    assert_eq!(service.seen(), [] as [Vec<u8>; 0]);

    let output = desk.run(&args);
    assert_success(&output);
    let found: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(found["ranking"], "model");
    assert_eq!(
        urls(&found),
        [loose.as_str(), NOTE, blob_key(unindexed).as_str()]
    );
    let ranked: Vec<_> = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["ranked"].clone())
        .collect();
    assert_eq!(ranked, [true, true, false]);
    let combined = desk.run(&[args.as_slice(), &["--mode", "combined"]].concat());
    assert_success(&combined);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&combined)).unwrap(),
        found
    );
    let seen = service.seen();
    assert_eq!(seen.len(), 2);
    assert!(String::from_utf8_lossy(&seen[0]).starts_with("POST /api/rank HTTP/1.1\r\n"));
    let request = request_body(&seen[0]);
    assert_eq!(request["schema_version"], 1);
    assert_eq!(request["query"], "quokka");
    assert_eq!(request["snippet_chars"], 200);
    assert_eq!(request["among"].as_array().unwrap().len(), 3);
    for query in ["", " \t\n"] {
        let output = desk.run(
            &[args.as_slice(), &["--mode", "combined"]]
                .concat()
                .iter()
                .map(|a| if *a == "quokka" { query } else { *a })
                .collect::<Vec<_>>(),
        );
        assert_success(&output);
        let listed: Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(listed["total"], 3);
        assert_eq!(listed["fallback"], json!([]));
    }
    assert_eq!(service.seen().len(), 2);

    // A service that does not answer leaves search by keyword, said once.
    drop(service);
    let output = desk.run(&[
        "search",
        "--json",
        "quokka",
        "--model-port",
        "1",
        "--model-secret-file",
        key.to_str().unwrap(),
    ]);
    assert_success(&output);
    let found: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(found["ranking"], "keyword");
    assert!(
        stderr(&output).contains("search is by keyword"),
        "{}",
        stderr(&output)
    );
}

fn json_answer(output: Output) -> Value {
    assert_success(&output);
    let bytes = output.stdout;
    serde_json::from_slice(&bytes).unwrap()
}

#[cfg(unix)]
fn filtered_search_pages(desk: &Desk) -> (Vec<String>, Service) {
    let keys: Vec<_> = (0..65).map(|i| format!("https://scope.test/{i}")).collect();
    let pages = desk.fx.root.join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    let mut rows = Vec::new();
    let mut tags = json!({});
    for (i, key) in keys.iter().enumerate() {
        rows.push(json!({"schema_version":1,"url":key,"title":"Quokka",
            "added_at": if i == 0 {"2026-01-01T00:00:00Z"} else if i == 1 {"2026-03-01T00:00:00Z"} else {"2026-02-01T00:00:00Z"}}).to_string());
        tags[key] = json!({"add": if i == 2 {vec!["Research"]} else {vec!["Research","Notes"]}});
    }
    rows.push(json!({"schema_version":1,"url":"https://sub.scope.test/excluded","title":"Quokka","added_at":"2026-02-01T00:00:00Z"}).to_string());
    tags["https://sub.scope.test/excluded"] = json!({"add":["Research","Notes"]});
    std::fs::write(pages.join("added.jsonl"), rows.join("\n") + "\n").unwrap();
    std::fs::write(desk.fx.root.join("library.json"), json!({"schema_version":1,"forgotten":[],
        "tags":tags,"vocabulary":{"Research":{"created_at":"2026-01-01T00:00:00Z"},"Notes":{"created_at":"2026-01-01T00:00:00Z"}}}).to_string()).unwrap();
    let service = Service::ranking(&desk.fx, move |body| {
        let keys: Vec<_> = body["among"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| {
                let key = p["key"].as_str().unwrap();
                (!key.ends_with("/64")).then_some(key)
            })
            .rev()
            .collect();
        detailed_rank(&keys)
    });
    (keys, service)
}

#[cfg(unix)]
fn detailed_passage(desk: &Desk, web: &str) -> (Value, String) {
    use sha2::{Digest as _, Sha256};
    assert_success(&desk.run(&["add", web, "--title", "Quokka note", "--no-content"]));
    let body = format!("lead\n{}", "🙂é\r\n".repeat(70));
    let quote: String = body[5..].chars().take(200).collect();
    let hash = common::model_service::hex(&Sha256::digest(body.as_bytes()));
    kept_text(
        &desk.fx,
        web,
        &json!({"schema_version":1,"url":web,
        "attempted_at":"2026-10-07T09:00:00Z","status":"thin","tier":"web"}),
        Some(&body),
    );
    let mut valid = detailed_rank(&[web]);
    let hit = &mut valid["hits"][0];
    hit["sources"]["text"]["input"] = json!("C");
    hit["semantic_source"] = json!("C");
    hit["chunk_status"] = json!("ready");
    hit["section"] = json!("Passage");
    hit["chunk"] = json!({"id":"a".repeat(64),"start":5,"end":body.len(),
        "hash":"b".repeat(64),"page_hash":"c".repeat(64),"score":0.5});
    hit["snippet"] = json!(quote);
    hit["snippet_range"] = json!({"start":5,"end":5+quote.len()});
    hit["content_sha256"] = json!(hash);
    valid["fallback"] = json!([]);
    (valid, body)
}

#[cfg(unix)]
fn walk_search_pages(desk: &Desk, service: &Service, args: &[&str]) -> Vec<Value> {
    let mut cursor: Option<String> = None;
    let mut gathered = Vec::new();
    loop {
        let mut request_args = args.to_vec();
        request_args.extend(["--limit", "17"]);
        if let Some(ref token) = cursor {
            request_args.extend(["--cursor", token]);
        }
        let found = json_answer(desk.ranked(service, &request_args));
        assert_eq!(found["total"], 62);
        gathered.extend(found["results"].as_array().unwrap().iter().cloned());
        assert_eq!(found["truncated"], !found["next_cursor"].is_null());
        cursor = found["next_cursor"].as_str().map(ToOwned::to_owned);
        if cursor.is_none() {
            return gathered;
        }
    }
}

#[cfg(unix)]
fn assert_stale_cursor(desk: &Desk, service: &Service, args: &[&str]) {
    let output = desk.ranked(service, args);
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["error"]["kind"], "stale_cursor");
}

/// A detailed reply is used only as a whole, with exact current body bytes.
#[cfg(unix)]
#[test]
fn signed_rank_checks_passages_and_rejects_invalid_or_legacy_details() {
    use sha2::{Digest as _, Sha256};
    use std::sync::{Arc, Mutex};
    const WEB: &str = "https://passage.test/note";
    let desk = Desk::new();
    let (valid, body) = detailed_passage(&desk, WEB);
    let quote = valid["hits"][0]["snippet"].as_str().unwrap();
    let hash = valid["hits"][0]["content_sha256"].as_str().unwrap();
    let reply = Arc::new(Mutex::new(valid.clone()));
    let answer = Arc::clone(&reply);
    let service = Service::ranking(&desk.fx, move |_| answer.lock().unwrap().clone());
    let args = ["search", "quokka", "--json"];
    let found = json_answer(desk.ranked(&service, &args));
    let hit = result(&found, WEB);
    assert_eq!(found["schema_version"], 1);
    assert_eq!(hit["snippet"], quote);
    assert_eq!(hit["content_sha256"], hash);
    assert_eq!(
        hit["id"],
        common::model_service::hex(&Sha256::digest(WEB.as_bytes()))
    );
    assert_eq!(hit["rank"], 1);
    assert_eq!(hit["section"], "Passage");
    assert_eq!(hit["capture_status"], "thin");
    assert_eq!(hit["readability"]["status"], "readable");
    assert_eq!(hit["sources"]["text"]["rank"], 1);
    assert_eq!(hit["chunk"]["score"], 0.5);
    assert!(hit["score"].as_f64().unwrap().is_finite());
    let request = request_body(&service.seen()[0]);
    assert_eq!(request["among"][0]["content_sha256"], hash);

    for (pointer, value) in [
        ("/hits/0/score", Value::Null),
        ("/hits/0/sources/text/rank", json!(2)),
        ("/hits/0/semantic_source", json!("A")),
        ("/hits/0/chunk/end", json!(body.len() + 1)),
        ("/hits/0/snippet_range/end", json!(6)),
        ("/hits/0/snippet", json!("rewritten")),
        ("/hits/0/content_sha256", json!("0".repeat(64))),
        ("/keys/0", json!("https://outside.test/")),
    ] {
        let mut bad = valid.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        *reply.lock().unwrap() = bad;
        let found = json_answer(desk.ranked(&service, &args));
        assert_eq!(found["ranking"], "keyword", "{pointer}");
        assert_eq!(
            found["fallback"][0]["code"], "rank_invalid_reply",
            "{pointer}"
        );
        assert_eq!(found["results"][0]["snippet"], Value::Null);
    }
    let mut duplicate = valid.clone();
    duplicate["keys"].as_array_mut().unwrap().push(json!(WEB));
    duplicate["hits"]
        .as_array_mut()
        .unwrap()
        .push(valid["hits"][0].clone());
    *reply.lock().unwrap() = duplicate;
    let output = desk.ranked(&service, &args);
    let found: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(found["fallback"][0]["code"], "rank_invalid_reply");
    for (response, code) in [
        (json!({"keys":[WEB]}), "rank_contract_unsupported"),
        (json!({"schema_version":2}), "rank_contract_unsupported"),
        (
            json!({"schema_version":1,"keys":[],"hits":[],"fallback":[{"stage":"rank","code":"rank_response_too_large"}]}),
            "rank_response_too_large",
        ),
    ] {
        *reply.lock().unwrap() = response;
        let found = json_answer(desk.ranked(&service, &args));
        assert_eq!(found["fallback"][0]["code"], code);
    }
    *reply.lock().unwrap() = valid.clone();
    kept_text(
        &desk.fx,
        WEB,
        &json!({"schema_version":1,"url":WEB,
        "attempted_at":"2026-10-07T09:00:00Z","status":"ok"}),
        Some("changed bytes"),
    );
    let output = desk.ranked(&service, &args);
    let found: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(found["fallback"][0]["code"], "rank_invalid_reply");
}

#[cfg(unix)]
#[test]
fn filters_precede_ranking_and_complete_paging_reaches_the_literal_tail() {
    let desk = Desk::new();
    let (keys, service) = filtered_search_pages(&desk);
    let pages = desk.fx.root.join("pages");
    let args = [
        "search",
        "quokka",
        "--json",
        "--tag",
        "RESEARCH",
        "--tag",
        "notes",
        "--domain",
        "WWW.SCOPE.TEST",
        "--domain",
        "elsewhere.test",
        "--kind",
        "web",
        "--kind",
        "blob",
        "--after",
        "2026-02-01T01:00:00+01:00",
        "--before",
        "2026-03-01T00:00:00Z",
    ];
    let all = json_answer(desk.ranked(&service, &args));
    assert_eq!(all["total"], 62);
    assert_eq!(all["returned"], 62);
    assert_eq!(all["limit"], Value::Null);
    assert_eq!(all["next_cursor"], Value::Null);
    assert_eq!(all["results"][0]["url"], keys[63]);
    let tail = &all["results"][61];
    assert_eq!(tail["url"], keys[64]);
    assert_eq!(tail["rank"], 62);
    assert_eq!(tail["score"], Value::Null);
    assert_eq!(tail["ranked"], false);
    assert_eq!(tail["readability"]["status"], "unavailable");
    assert_eq!(tail["capture_status"], Value::Null);
    assert_eq!(tail["tags"], json!(["Notes", "Research"]));
    let among = request_body(&service.seen()[0])["among"].clone();
    assert_eq!(among.as_array().unwrap().len(), 62);
    assert!(!among.to_string().contains("excluded"));
    assert!(
        !among
            .as_array()
            .unwrap()
            .iter()
            .any(|p| keys[..3].iter().any(|k| p["key"] == *k))
    );
    assert_eq!(all["results"][0]["sources"]["text"]["rank"], 1);

    let gathered = walk_search_pages(&desk, &service, &args);
    assert_eq!(json!(gathered), all["results"]);
    let first = desk.ranked(&service, &[args.as_slice(), &["--limit", "1"]].concat());
    let first: Value = serde_json::from_str(&stdout(&first)).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    let resized = desk.ranked(
        &service,
        &[args.as_slice(), &["--limit", "3", "--cursor", cursor]].concat(),
    );
    assert_success(&resized);
    let resized: Value = serde_json::from_str(&stdout(&resized)).unwrap();
    assert_eq!(
        resized["results"],
        json!(&all["results"].as_array().unwrap()[1..4])
    );
    for changed in ["--tag", "--mode"] {
        let mut next = args.to_vec();
        next.extend([
            "--cursor",
            cursor,
            changed,
            if changed == "--tag" {
                "unknown"
            } else {
                "keyword"
            },
        ]);
        assert_stale_cursor(&desk, &service, &next);
    }
    let empty = desk.ranked(&service, &[args.as_slice(), &["--tag", "unknown"]].concat());
    assert_success(&empty);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&empty)).unwrap()["total"],
        0
    );
    let before = common::fingerprint(&desk.fx.root);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(pages.join("added.jsonl"))
        .unwrap();
    writeln!(file,"{}",json!({"schema_version":1,"url":keys[63],"title":"Changed title","added_at":"2026-02-01T00:00:00Z"})).unwrap();
    assert_ne!(common::fingerprint(&desk.fx.root), before);
    assert_stale_cursor(
        &desk,
        &service,
        &[args.as_slice(), &["--cursor", cursor]].concat(),
    );
}

#[test]
fn search_usage_errors_are_versioned_and_json_after_the_terminator_is_query_text() {
    let desk = Desk::new();
    for args in [
        vec!["search", "--json"],
        vec!["search", "quokka", "--json", "--limit", "0"],
        vec!["search", "quokka", "--json", "--limit", "bad"],
        vec!["search", "quokka", "--json", "--cursor", "v2:0:bad"],
        vec!["search", "quokka", "--json", "--kind", "other"],
        vec!["search", "quokka", "--json", "--domain", "host/path"],
        vec!["search", "quokka", "--json", "--after", "yesterday"],
        vec![
            "search",
            "quokka",
            "--json",
            "--after",
            "2026-03-01T00:00:00Z",
            "--before",
            "2026-02-01T00:00:00Z",
        ],
    ] {
        let output = desk.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error: Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(error["schema_version"], 1);
        assert_eq!(error["error"]["kind"], "usage");
        assert_eq!(stderr(&output), "");
    }
    let output = desk.run(&["search", "--bad", "--", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("--bad"));
}
