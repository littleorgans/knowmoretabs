//! `GET /api/content` through the real binary: one page's latest content
//! and image lines and its kept text, read only, and only for a page the
//! library lists.

mod common;

use std::fs;

use common::serve::{A, B, HIDDEN, Reply, Server, append_line, archive, kept_name};
use common::{Fixture, fingerprint, write_snapshot};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

/// A body that would run if it were ever taken for HTML.
const BODY: &str =
    "# Sandbox\n\nRuns a <script>alert(1)</script> command in a box.\n\nOne rule per line.\n";

fn content(server: &Server, url: &str) -> Reply {
    server.get(&format!(
        "/api/content?url={}",
        utf8_percent_encode(url, NON_ALPHANUMERIC)
    ))
}

fn served(server: &Server, url: &str) -> Value {
    let reply = content(server, url);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("content-type"), Some("application/json"));
    reply.json()
}

/// A content line for `url` and, given a body, its file as `content` writes
/// it: front matter, then the markdown.
fn kept_text(fx: &Fixture, url: &str, line: &Value, body: Option<&str>) {
    append_line(fx, "content.jsonl", line);
    if let Some(body) = body {
        let dir = fx.root.join("pages").join("content");
        fs::create_dir_all(&dir).unwrap();
        let front = format!(
            "---\nschema_version: 1\nurl: {}\nfetched_at: \"2026-10-07T09:00:00Z\"\n---\n",
            json!(url)
        );
        fs::write(dir.join(format!("{}.md", kept_name(url))), front + body).unwrap();
    }
}

fn ok_line(url: &str) -> Value {
    json!({"schema_version": 1, "url": url, "attempted_at": "2026-10-07T09:00:00Z",
        "status": "ok", "tier": "github", "chars": BODY.len()})
}

#[test]
fn a_fetched_page_serves_its_latest_lines_and_its_text_without_front_matter() {
    let fx = Fixture::new();
    archive(&fx);
    kept_text(
        &fx,
        A,
        &json!({"schema_version": 1, "url": A, "attempted_at": "2026-10-06T09:00:00Z", "status": "error", "reason": "HTTP 503"}),
        None,
    );
    kept_text(&fx, A, &ok_line(A), Some(BODY));
    append_line(
        &fx,
        "images.jsonl",
        &json!({"schema_version": 1, "url": A, "attempted_at": "2026-10-07T09:00:00Z", "status": "ok"}),
    );
    let before = fingerprint(&fx.root.join("pages"));
    let server = Server::start(&fx);
    let reply = served(&server, A);
    assert_eq!(reply["url"], A);
    assert_eq!(reply["content"]["status"], "ok", "the latest line");
    assert_eq!(reply["content"]["tier"], "github");
    assert_eq!(reply["content"]["attempted_at"], "2026-10-07T09:00:00Z");
    assert_eq!(reply["text"], BODY, "the body exactly, as text");
    assert_eq!(reply["image"]["status"], "ok");
    assert_eq!(
        fingerprint(&fx.root.join("pages")),
        before,
        "nothing written"
    );
}

#[test]
fn a_listed_page_with_nothing_fetched_has_no_content() {
    let fx = Fixture::new();
    archive(&fx);
    kept_text(&fx, A, &ok_line(A), Some(BODY));
    let server = Server::start(&fx);
    for url in [B, HIDDEN] {
        assert_eq!(
            served(&server, url),
            json!({"url": url, "content": null, "text": null, "image": null}),
            "{url}"
        );
    }
}

#[test]
fn a_failed_image_carries_its_recorded_reason() {
    let fx = Fixture::new();
    archive(&fx);
    kept_text(&fx, B, &ok_line(B), Some(BODY));
    append_line(
        &fx,
        "images.jsonl",
        &json!({"schema_version": 1, "url": B, "attempted_at": "2026-10-07T09:00:00Z",
            "status": "error", "http_status": 503, "reason": "HTTP 503"}),
    );
    let server = Server::start(&fx);
    let image = &served(&server, B)["image"];
    assert_eq!(image["status"], "error");
    assert_eq!(image["reason"], "HTTP 503");
    assert_eq!(image["http_status"], 503);
}

#[test]
fn an_address_the_library_does_not_list_is_not_found() {
    let fx = Fixture::new();
    archive(&fx);
    // Captured once, but in no snapshot; and a tab on this machine, which
    // a snapshot keeps but the library never lists.
    let elsewhere = "https://elsewhere.test/page";
    kept_text(&fx, elsewhere, &ok_line(elsewhere), Some(BODY));
    let server = Server::start(&fx);
    for url in [
        elsewhere,
        "http://localhost:3000/",
        "https://a.test/one/",
        "https://A.test/one",
    ] {
        let reply = content(&server, url);
        assert_eq!(reply.status, 404, "{url}");
        assert_eq!(reply.header("content-type"), Some("application/json"));
        assert!(!reply.text().contains("Sandbox"), "{url}");
    }
}

#[test]
fn the_address_is_decoded_exactly_and_a_bad_query_is_refused() {
    let fx = Fixture::new();
    archive(&fx);
    let query = "https://q.test/find?x=1&url=2#top";
    write_snapshot(
        &fx.root,
        "2026-03-01-000000Z",
        "2026-03-01T00:00:00Z",
        &[(1, query, "Query")],
    );
    kept_text(&fx, query, &ok_line(query), Some(BODY));
    let server = Server::start(&fx);
    assert_eq!(served(&server, query)["text"], BODY);
    for path in [
        "/api/content",
        "/api/content?",
        "/api/content?url=",
        "/api/content?page=https%3A%2F%2Fa.test%2Fone",
        "/api/content?url=%FF",
    ] {
        let reply = server.get(path);
        assert_eq!(reply.status, 400, "{path}");
        assert_eq!(reply.header("content-type"), Some("application/json"));
    }
    let reply = server.post("/api/content", &json!({"url": A}));
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("GET"));
}

#[cfg(unix)]
#[test]
fn a_text_file_that_is_a_symlink_is_not_read() {
    let fx = Fixture::new();
    archive(&fx);
    kept_text(&fx, A, &ok_line(A), None);
    let outside = fx.root.join("private.txt");
    fs::write(&outside, "---\n---\nprivate bytes\n").unwrap();
    let dir = fx.root.join("pages").join("content");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(outside, dir.join(format!("{}.md", kept_name(A)))).unwrap();
    let server = Server::start(&fx);
    let reply = served(&server, A);
    assert_eq!(reply["content"]["status"], "ok");
    assert_eq!(reply["text"], Value::Null);
}
