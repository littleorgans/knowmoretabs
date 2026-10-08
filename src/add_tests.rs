//! `add` against a temporary archive: what it writes, what it refuses, and
//! that every URL taking command accepts what it added.
//!
//! slice: add
//! why: An added page is only worth having if the rest of the library
//!      treats it as a page, so the proof that `tag`, `content --url` and
//!      `forget` accept it lives beside the command that adds it.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::content::{self, Args as ContentArgs};
use crate::content_test::saved;
use crate::targets::Options;
use crate::{tags, triage};

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const SAVED: &str = "https://saved.test/page";

const QUIET: Log = Log {
    quiet: true,
    verbose: false,
};

fn add(root: &Path, url: &str) -> Outcome {
    apply(root, url, None, false, || {
        panic!("nothing else holds the lock")
    })
    .unwrap()
}

fn log_bytes(root: &Path) -> Option<Vec<u8>> {
    fs::read(intake::path(root)).ok()
}

fn archive() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    (dir, root)
}

#[test]
fn each_refusal_is_named_and_writes_nothing() {
    for (url, reason) in [
        ("not a url", Reason::NotWeb),
        ("", Reason::NotWeb),
        ("ftp://a.test/file", Reason::NotWeb),
        ("file:///tmp/page.html", Reason::NotWeb),
        ("mailto:someone@a.test", Reason::NotWeb),
        ("about:blank", Reason::NotWeb),
        ("http://localhost:3000/dev", Reason::Private),
        ("http://app.localhost/", Reason::Private),
        ("http://127.0.0.1/", Reason::Private),
        ("http://[::1]:8080/", Reason::Private),
        ("http://192.168.1.20/admin", Reason::Private),
        ("http://10.0.0.1/", Reason::Private),
        ("http://printer.local/", Reason::Private),
        ("https://intranet/wiki", Reason::Private),
    ] {
        let (_dir, root) = archive();
        assert_eq!(add(&root, url), Outcome::Refused(reason), "{url:?}");
        assert!(
            !root.exists(),
            "{url:?}: refused before the archive is touched"
        );
    }
}

/// `hidden` is the library's own rule, and today every URL it hides is
/// already refused as not the web or private, so the two cannot disagree.
#[test]
fn every_url_the_library_hides_is_refused() {
    for url in [
        "file:///tmp/a",
        "HTTP://LOCALHOST:8080/a",
        "http://127.0.0.2/a",
        "http://0.0.0.0:3000/a",
        "http://localhost./",
        "http://[::1]:8080/a",
    ] {
        assert!(!library::is_listed(url), "{url}");
        assert!(refusal(url).is_some(), "{url}");
    }
    assert_eq!(refusal(A), None);
    assert_eq!(refusal("http://a.test/?token=x"), None, "content skips it");
}

#[test]
fn a_new_page_writes_one_line_and_a_second_add_writes_nothing() {
    let (_dir, root) = archive();
    assert_eq!(
        apply(&root, A, Some("One"), false, || panic!("not held")).unwrap(),
        Outcome::Added
    );
    let first = log_bytes(&root).unwrap();
    let lines: Vec<serde_json::Value> = String::from_utf8(first.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(
        (
            &lines[0]["url"],
            &lines[0]["title"],
            &lines[0]["schema_version"]
        ),
        (&A.into(), &"One".into(), &1.into())
    );
    assert!(lines[0]["added_at"].as_str().unwrap().ends_with('Z'));

    assert_eq!(add(&root, A), Outcome::Known);
    assert_eq!(
        apply(&root, A, Some("Another"), false, || panic!("not held")).unwrap(),
        Outcome::Known
    );
    assert_eq!(log_bytes(&root).unwrap(), first, "byte for byte");
}

#[test]
fn a_page_a_snapshot_holds_is_known_and_never_written() {
    let (_dir, root) = archive();
    saved(&root, &[SAVED]);
    assert_eq!(add(&root, SAVED), Outcome::Known);
    assert_eq!(log_bytes(&root), None);
}

#[test]
fn a_forgotten_page_stays_forgotten() {
    let (_dir, root) = archive();
    saved(&root, &[SAVED]);
    assert_eq!(add(&root, A), Outcome::Added);
    triage::apply(
        &root,
        &[SAVED.to_owned(), A.to_owned()],
        triage::Action::Forget,
        true,
        QUIET,
    )
    .unwrap();
    let before = log_bytes(&root);
    assert_eq!(add(&root, SAVED), Outcome::Forgotten);
    assert_eq!(add(&root, A), Outcome::Forgotten);
    assert_eq!(log_bytes(&root), before);

    // Forgotten with no snapshot or line left that names it: still refused.
    let (_other, fresh) = archive();
    Archive::open(&fresh).unwrap();
    fs::write(
        fresh.join(library::STATE_FILE),
        format!(r#"{{"schema_version":1,"forgotten":["{B}"]}}"#),
    )
    .unwrap();
    assert_eq!(add(&fresh, B), Outcome::Forgotten);
    assert_eq!(log_bytes(&fresh), None);
}

#[test]
fn a_retry_refuses_a_page_the_library_does_not_hold_and_writes_nothing() {
    let retry = |root: &Path, url: &str| apply(root, url, None, true, || panic!("not held"));
    let (_dir, root) = archive();
    assert_eq!(
        retry(&root, A).unwrap(),
        Outcome::Refused(Reason::NotInLibrary)
    );
    assert!(!root.exists(), "no archive created");

    saved(&root, &[SAVED]);
    assert_eq!(
        retry(&root, A).unwrap(),
        Outcome::Refused(Reason::NotInLibrary)
    );
    assert_eq!(log_bytes(&root), None, "no intake line");
    assert_eq!(retry(&root, SAVED).unwrap(), Outcome::Known);
    assert_eq!(add(&root, B), Outcome::Added);
    let before = log_bytes(&root);
    assert_eq!(retry(&root, B).unwrap(), Outcome::Known);
    triage::apply(&root, &[B.to_owned()], triage::Action::Forget, true, QUIET).unwrap();
    assert_eq!(retry(&root, B).unwrap(), Outcome::Forgotten);
    assert_eq!(log_bytes(&root), before);
    assert_eq!(
        retry(&root, "http://localhost/").unwrap(),
        Outcome::Refused(Reason::Private)
    );
}

#[test]
fn the_library_lists_added_pages_beside_saved_ones() {
    let (_dir, root) = archive();
    saved(&root, &[SAVED]);
    assert_eq!(
        apply(&root, B, Some("Two"), false, || panic!("not held")).unwrap(),
        Outcome::Added
    );
    assert_eq!(add(&root, A), Outcome::Added);
    let loaded = library::load(&Archive::at(&root)).unwrap();
    let ids: Vec<_> = loaded.snapshots.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["2026-10-07-090000Z", intake::SNAPSHOT_ID]);
    let known = library::known_urls(&loaded.snapshots);
    assert!([SAVED, A, B].iter().all(|url| known.contains(url)));
    let added: Vec<_> = loaded.snapshots[1]
        .tabs
        .iter()
        .map(|t| (t.url.as_str(), t.title.as_str()))
        .collect();
    assert_eq!(added, [(B, "Two"), (A, "")]);
}

#[test]
fn a_held_lock_is_waited_for_and_said_once() {
    let (_dir, root) = archive();
    let archive = Archive::open(&root).unwrap();
    let held = archive.lock(|| {}).unwrap();
    let (said, waited) = mpsc::channel();
    let adding = std::thread::spawn({
        let root = root.clone();
        move || apply(&root, A, None, false, move || said.send(()).unwrap())
    });
    waited.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(log_bytes(&root), None, "nothing written while waiting");
    drop(held);
    assert_eq!(adding.join().unwrap().unwrap(), Outcome::Added);
}

#[test]
fn tag_content_and_forget_accept_an_added_page() {
    let (_dir, root) = archive();
    saved(&root, &[SAVED]);
    let urls = [A.to_owned()];
    let tag = || tags::apply(&root, &urls, &["Reading".to_owned()], &[], &[], true, QUIET);
    let content = || {
        content::command(
            &root,
            ContentArgs {
                options: Options {
                    dry_run: true,
                    limit: None,
                    refetch: false,
                },
                urls: &urls,
                no_images: true,
                browser: crate::platform::CHROME,
                no_browser: true,
                signed_in: false,
                events: None,
                retry: None,
            },
            false,
            QUIET,
        )
    };
    let forget = || triage::apply(&root, &urls, triage::Action::Forget, true, QUIET);
    assert!(matches!(tag(), Err(Error::NotInLibrary(_))));
    assert!(matches!(content(), Err(Error::NotInLibrary(_))));
    assert!(matches!(forget(), Err(Error::NotInLibrary(_))));

    assert_eq!(add(&root, A), Outcome::Added);
    assert_eq!(tag().unwrap().vocabulary[0].name, "Reading");
    let state = State::read(&root).unwrap();
    assert_eq!(
        state.page_tags(A, &state.spellings(false)),
        ["Reading".to_owned()]
    );
    content().unwrap();
    assert_eq!(forget().unwrap().changed, [A.to_owned()]);
    assert!(State::read(&root).unwrap().forgotten.contains(A));
}

#[test]
fn json_lines_are_the_contract_shape() {
    assert_eq!(
        library_line("running"),
        json!({"stage": "library", "state": "running"})
    );
    assert_eq!(
        library_line("waiting"),
        json!({"stage": "library", "state": "waiting"})
    );
    assert_eq!(
        library_done(Outcome::Added),
        json!({"stage": "library", "state": "done", "value": "added"})
    );
    assert_eq!(
        library_done(Outcome::Known),
        json!({"stage": "library", "state": "done", "value": "known"})
    );
    assert_eq!(
        library_done(Outcome::Forgotten),
        json!({"stage": "library", "state": "done", "value": "forgotten"})
    );
    for (reason, name) in [
        (Reason::NotWeb, "not_web"),
        (Reason::Private, "private"),
        (Reason::Hidden, "hidden"),
        (Reason::NotInLibrary, "not_in_library"),
    ] {
        assert_eq!(
            library_done(Outcome::Refused(reason)),
            json!({"stage": "library", "state": "done", "value": "refused", "reason": name})
        );
    }
    assert_eq!(
        done_line(A, Outcome::Known, &Ended::default()),
        json!({"stage": "done", "state": "done", "url": A, "value": "known",
            "content": null, "image": null})
    );
    assert!(Outcome::Added.in_library() && Outcome::Known.in_library());
    assert!(!Outcome::Forgotten.in_library());
    assert!(!Outcome::Refused(Reason::NotWeb).in_library());
}
