//! A post bookmarked on X, saved through `serve` by the real binary: the
//! page, its click line and its text from a stand-in X post API; the
//! origins the routes accept and refuse; the extension's health report and
//! the `doctor` line it makes.

mod common;

use std::fs;
use std::process::Stdio;

use common::serve::{A, Reply, Server, archive, kept_name, page};
use common::site::{self, Site};
use common::{Fixture, stderr, stdout};
use serde_json::{Value, json};

const POST: &str = "https://x.com/example_author/status/100";
const PRIVATE: &str = "https://x.com/example_private/status/401";

/// The extension's origin as `serve` pins it, read where it is declared.
fn extension_origin() -> String {
    let source = include_str!("../src/http.rs");
    let declared = source
        .lines()
        .find_map(|line| line.strip_prefix("pub(crate) const EXTENSION_ORIGIN: &str = \""))
        .expect("EXTENSION_ORIGIN in src/http.rs");
    declared.trim_end_matches("\";").to_owned()
}

/// A synthetic post in the X post API's shape.
fn answer() -> Vec<u8> {
    serde_json::to_vec(&json!({"code": 200, "thread": null, "status": {
        "type": "status", "id": "100", "url": POST,
        "text": "A synthetic post about keeping what you bookmark.",
        "created_timestamp": 1_791_363_600, "lang": "en",
        "author": {"type": "profile", "name": "Example Author", "screen_name": "example_author",
            "protected": false},
        "media": {}, "replying_to": null}}))
    .unwrap()
}

/// The X post API, standing in: post 100 answers, post 401 is private.
fn x_api() -> Site {
    Site::start(|host, path, _| match (host, path) {
        ("api.fxtwitter.com", "/2/status/100") => site::Reply {
            status: 200,
            headers: vec![("Content-Type", "application/json".into())],
            body: answer(),
            stall: None,
        },
        ("api.fxtwitter.com", "/2/status/401") => site::Reply::status(401),
        _ => site::Reply::status(404),
    })
}

/// `serve` with every name resolved to `site`.
fn serve(fx: &Fixture, site: &Site) -> Server {
    Server::from_child(
        fx.command()
            .args(["serve", "--port", "0"])
            .env("KNOWMORETABS_TEST_RESOLVE", site.address.to_string())
            .env("KNOWMORETABS_TEST_TIMEOUT_MS", "400")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn serve"),
    )
}

/// A JSON POST from `origin`, as the extension's service worker sends it.
fn post_from(server: &Server, origin: &str, path: &str, body: &Value) -> Reply {
    let body = body.to_string();
    server.raw(
        "POST",
        path,
        &[
            format!("Host: {}", server.host()),
            format!("Origin: {origin}"),
            "Content-Type: application/json".to_owned(),
            format!("Content-Length: {}", body.len()),
        ],
        body.as_bytes(),
    )
}

fn bookmark(server: &Server, post: &str) -> Reply {
    post_from(
        server,
        &extension_origin(),
        "/api/x/bookmark",
        &json!({"post": post}),
    )
}

fn log_lines(fx: &Fixture, name: &str) -> Vec<Value> {
    fs::read_to_string(fx.root.join("pages").join(name))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn kept_front(fx: &Fixture, url: &str) -> String {
    let file = fx
        .root
        .join("pages")
        .join("content")
        .join(format!("{}.md", kept_name(url)));
    fs::read_to_string(file).unwrap_or_default()
}

#[test]
fn a_bookmark_click_saves_the_post_once_and_marks_it_on_every_click() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);

    let reply = bookmark(
        &server,
        "https://twitter.com/example_author/status/100/photo/1?s=20",
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let line = reply.json();
    assert_eq!(
        (
            &line["stage"],
            &line["url"],
            &line["value"],
            &line["content"]
        ),
        (&json!("done"), &json!(POST), &json!("added"), &json!("ok")),
        "{line}"
    );
    assert!(
        site.paths()
            .contains(&"api.fxtwitter.com/2/status/100".to_owned()),
        "only the post's number was asked: {:?}",
        site.paths()
    );
    let front = kept_front(&fx, POST);
    assert!(front.contains("tier: \"x\""), "{front}");
    let intake = log_lines(&fx, "added.jsonl");
    assert!(intake.iter().all(|line| line["url"] == POST), "{intake:?}");
    assert_eq!(
        intake.last().unwrap()["title"],
        "Example Author (@example_author) on X"
    );
    let clicks = log_lines(&fx, "xbookmarks.jsonl");
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(
        (&clicks[0]["url"], &clicks[0]["post"], &clicks[0]["posts"]),
        (&json!(POST), &json!(POST), &json!(["100"]))
    );
    let library = server.library();
    let saved = page(&library, POST);
    assert_eq!(saved["x"]["clicked"], json!([POST]), "{saved}");
    assert_eq!(saved["x"]["bookmarked_at"], clicks[0]["at"]);
    assert!(
        page(&library, A).get("x").is_none(),
        "only bookmarked pages"
    );

    // The same post again: known, one more click, no new intake line.
    let again = bookmark(&server, POST).json();
    assert_eq!(again["value"], "known", "{again}");
    assert_eq!(log_lines(&fx, "xbookmarks.jsonl").len(), 2);
    assert_eq!(log_lines(&fx, "added.jsonl").len(), intake.len());

    // Forgotten: nothing written, and the reply says so.
    let forget = server.post("/api/forget", &json!({"urls": [POST]}));
    assert_eq!(forget.status, 200, "{}", forget.text());
    let forgotten = bookmark(&server, POST);
    assert_eq!(forgotten.status, 200, "{}", forgotten.text());
    assert_eq!(forgotten.json()["value"], "forgotten");
    assert_eq!(log_lines(&fx, "xbookmarks.jsonl").len(), 2);
    assert_eq!(log_lines(&fx, "added.jsonl").len(), intake.len());
}

#[test]
fn a_private_post_is_saved_without_its_text() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let reply = bookmark(&server, PRIVATE);
    assert_eq!(reply.status, 200, "{}", reply.text());
    let line = reply.json();
    assert_eq!(
        (&line["value"], &line["content"]),
        (&json!("added"), &json!("behind_login")),
        "{line}"
    );
    page(&server.library(), PRIVATE);
}

#[test]
fn the_routes_refuse_what_is_not_a_post_and_every_origin_but_the_extensions() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    for post in [
        "https://x.com/i/status/100",
        "https://x.com/example_author",
        "https://example.org/example_author/status/100",
        "https://x.com/bad-handle/status/100",
        "",
    ] {
        let reply = bookmark(&server, post);
        assert_eq!(reply.status, 400, "{post}: {}", reply.text());
    }
    let shapeless = post_from(&server, &extension_origin(), "/api/x/bookmark", &json!({}));
    assert_eq!(shapeless.status, 400, "{}", shapeless.text());
    for origin in [
        "https://x.com",
        "https://example.org",
        "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        for path in ["/api/x/bookmark", "/api/x/health", "/api/tags"] {
            let reply = post_from(&server, origin, path, &json!({"post": POST}));
            assert_eq!(reply.status, 403, "{origin} {path}: {}", reply.text());
        }
    }
    let body = format!("post={POST}");
    let form = server.raw(
        "POST",
        "/api/x/bookmark",
        &[
            format!("Host: {}", server.host()),
            "Content-Type: application/x-www-form-urlencoded".to_owned(),
            format!("Content-Length: {}", body.len()),
        ],
        body.as_bytes(),
    );
    assert_eq!(form.status, 415, "{}", form.text());
    assert!(
        !fx.root.join("pages").join("xbookmarks.jsonl").exists(),
        "nothing written"
    );
    assert!(site.paths().is_empty(), "nothing asked: {:?}", site.paths());

    // The extension counts as the Library itself on the other routes.
    let created = fx.run(&["tags", "--create", "Harness"]);
    assert!(created.status.success(), "{}", stderr(&created));
    let tagged = post_from(
        &server,
        &extension_origin(),
        "/api/tags",
        &json!({"urls": [A], "add": ["Harness"]}),
    );
    assert_eq!(tagged.status, 200, "{}", tagged.text());
}

fn doctor_line(fx: &Fixture) -> Value {
    let output = fx.run(&["--json", "doctor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["tier"] == "extension")
        .cloned()
        .unwrap_or_else(|| panic!("no extension line: {report}"))
}

fn health(state: &str) -> Value {
    json!({"state": state, "since": "2026-10-27T09:00:00Z", "version": "0.2.0", "views": 12,
        "posts": 140, "controls": 140, "intents": 3, "confirmed": 3, "dropped": 0,
        "last_dropped": null})
}

#[test]
fn the_extensions_health_report_is_kept_and_doctor_says_it() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    assert_eq!(doctor_line(&fx)["state"], "missing");

    let report = |body: &Value| post_from(&server, &extension_origin(), "/api/x/health", body);
    let ok = report(&health("ok"));
    assert_eq!((ok.status, ok.json()), (200, json!({})), "{}", ok.text());
    let kept: Value =
        serde_json::from_slice(&fs::read(fx.root.join("extension.json")).unwrap()).unwrap();
    assert_eq!(kept["state"], "ok");
    assert!(kept["reported_at"].is_string(), "{kept}");
    assert_eq!(doctor_line(&fx)["state"], "ready");

    let blind = report(&health("blind"));
    assert_eq!(blind.status, 200, "{}", blind.text());
    let line = doctor_line(&fx);
    assert_eq!(line["state"], "degraded", "{line}");
    assert_eq!(
        line["detail"],
        "sees posts on X but no bookmark button since 2026-10-27"
    );

    let mut with_url = health("ok");
    with_url["url"] = json!(POST);
    let mut bad_version = health("ok");
    bad_version["version"] = json!("latest");
    for bad in [health("lost"), with_url, bad_version] {
        let reply = report(&bad);
        assert_eq!(reply.status, 400, "{bad}: {}", reply.text());
    }
    assert_eq!(
        doctor_line(&fx)["state"],
        "degraded",
        "the last good report stands"
    );
}

#[test]
fn bookmarks_dropped_while_the_library_was_down_degrade_doctor() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let report = |body: &Value| post_from(&server, &extension_origin(), "/api/x/health", body);
    let last = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
    let mut dropped = health("ok");
    dropped["dropped"] = json!(3);
    dropped["last_dropped"] = json!(last.to_string());
    let kept = report(&dropped);
    assert_eq!(kept.status, 200, "{}", kept.text());
    let written: Value =
        serde_json::from_slice(&fs::read(fx.root.join("extension.json")).unwrap()).unwrap();
    assert_eq!(
        (&written["dropped"], &written["last_dropped"]),
        (&json!(3), &dropped["last_dropped"]),
        "{written}"
    );
    let line = doctor_line(&fx);
    assert_eq!(line["state"], "degraded", "{line}");
    assert_eq!(
        line["detail"],
        format!(
            "3 bookmarks on X were not saved because knowmoretabs was not answering; last {}",
            last.strftime("%Y-%m-%d")
        )
    );
    assert_eq!(
        line["hint"],
        "keep `just up` running while you bookmark on X"
    );

    let mut bad = Vec::new();
    for (count, last) in [
        (json!(-1), dropped["last_dropped"].clone()),
        (json!(1.5), dropped["last_dropped"].clone()),
        (json!(1_000_001), dropped["last_dropped"].clone()),
        (json!(3), json!(null)),
        (json!(0), dropped["last_dropped"].clone()),
        (json!(3), json!("yesterday")),
    ] {
        let mut body = health("ok");
        body["dropped"] = count;
        body["last_dropped"] = last;
        bad.push(body);
    }
    let mut missing = health("ok");
    missing.as_object_mut().unwrap().remove("dropped");
    bad.push(missing);
    for body in bad {
        let reply = report(&body);
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
    }

    let ok = report(&health("ok"));
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(doctor_line(&fx)["state"], "ready", "no drops, ready again");
}
