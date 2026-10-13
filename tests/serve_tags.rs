//! Tags and the vocabulary over `serve`'s API through the real binary:
//! suggestions confirmed and dismissed, the round trip and its exact undo,
//! retirement, and every request the tag routes refuse.

mod common;

use std::fs;

use common::serve::{A, B, HIDDEN, Server, archive, page, without_generated_at};
use common::{Fixture, state, stderr};
use serde_json::{Value, json};

fn tags_of(library: &Value, url: &str) -> Value {
    page(library, url)["tags"].clone()
}

fn names(vocabulary: &Value) -> Vec<&str> {
    vocabulary
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            assert!(entry["created_at"].is_string(), "{entry}");
            let keys = entry.as_object().unwrap().keys();
            assert!(
                keys.into_iter()
                    .all(|key| ["name", "created_at", "used_at"].contains(&key.as_str())),
                "{entry}"
            );
            entry["name"].as_str().unwrap()
        })
        .collect()
}

/// Suggestions come from `tag --import`; the page confirms one with the
/// existing add, dismisses one with remove, and the exact undo brings it
/// back as a suggestion.
#[test]
fn suggested_tags_are_served_and_the_tag_endpoint_confirms_and_dismisses_them() {
    let fx = Fixture::new();
    archive(&fx);
    let output = fx.run(&["tags", "--create", "Harness", "--create", "Agent"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let file = fx.home.path().join("tags.jsonl");
    let lines = [
        json!({"url": A, "tags": ["Harness", "Agent"], "source": "model-one"}),
        json!({"url": HIDDEN, "tags": ["Agent"], "source": "model-one"}),
        json!({"url": B, "tags": [], "source": "model-one"}),
    ];
    fs::write(&file, lines.map(|line| format!("{line}\n")).concat()).unwrap();
    let output = fx.run(&["tag", "--import", file.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    fs::write(
        &file,
        format!(
            "{}\n",
            json!({"url": A, "tags": ["agent"], "source": "model-two"})
        ),
    )
    .unwrap();
    let output = fx.run(&["tag", "--import", file.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));

    let server = Server::start(&fx);
    let library = server.library();
    assert_eq!(
        page(&library, A)["suggested"],
        json!([
            {"name": "Agent", "sources": ["model-one", "model-two"]},
            {"name": "Harness", "sources": ["model-one"]},
        ])
    );
    assert_eq!(tags_of(&library, A), json!([]), "a suggestion is not a tag");
    assert_eq!(
        page(&library, HIDDEN)["suggested"],
        json!([{"name": "Agent", "sources": ["model-one"]}]),
        "serve keeps forgotten pages, and theirs"
    );
    assert_eq!(page(&library, B)["suggested"], json!([]));
    assert_eq!(names(&library["vocabulary"]), ["Agent", "Harness"]);

    // Confirm one and dismiss the other.
    let reply = server.post(
        "/api/tags",
        &json!({"urls": [A], "add": ["Agent"], "remove": ["Harness"]}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let undo = reply.json()["undo"].clone();
    let decided = server.library();
    assert_eq!(tags_of(&decided, A), json!(["Agent"]));
    assert_eq!(page(&decided, A)["suggested"], json!([]));
    // Confirming is a use; dismissing is not.
    let used = decided["vocabulary"][0]["used_at"].clone();
    assert!(used.is_string(), "{}", decided["vocabulary"]);
    assert!(decided["vocabulary"][1].get("used_at").is_none());

    let reply = server.post("/api/tags", &json!({"undo": undo, "urls": []}));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let mut undone = without_generated_at(server.library());
    assert_eq!(
        undone["vocabulary"][0]["used_at"], used,
        "the undo keeps the use"
    );
    undone["vocabulary"][0]
        .as_object_mut()
        .unwrap()
        .remove("used_at");
    assert_eq!(
        undone,
        without_generated_at(library),
        "the undo brings both suggestions back"
    );
}

/// The page shows what a tag means where it helps you decide, so the served
/// vocabulary carries each definition, and leaves the key out when there is none.
#[test]
fn the_served_vocabulary_carries_definitions() {
    let fx = Fixture::new();
    archive(&fx);
    let output = fx.run(&[
        "tags",
        "--define",
        "Harness",
        "Agent harnesses and the code around a model",
        "--create",
        "Agent",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let server = Server::start(&fx);
    let vocabulary = server.library()["vocabulary"].clone();
    assert_eq!(vocabulary[0]["name"], "Agent");
    assert!(vocabulary[0].get("definition").is_none(), "{vocabulary}");
    assert_eq!(
        vocabulary[1]["definition"],
        "Agent harnesses and the code around a model"
    );
    let reply = server.post("/api/vocabulary", &json!({"create": ["Skills"]}));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.json()["vocabulary"][1]["definition"],
        "Agent harnesses and the code around a model"
    );
}

#[test]
fn tags_round_trip_and_the_swapped_request_undoes_them() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let original = server.library();
    assert_eq!(original["vocabulary"], json!([]));
    for page in original["pages"].as_array().unwrap() {
        assert_eq!(page["tags"], json!([]), "untagged is an empty list");
    }

    let reply = server.post(
        "/api/tags",
        &json!({"urls": [A, HIDDEN, "https://nowhere.test/"], "add": ["Harness", "mcp"]}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let body = reply.json();
    assert_eq!(body["urls"], json!([A, HIDDEN]));
    assert_eq!(
        body["tags"],
        json!({A: ["Harness", "mcp"], HIDDEN: ["Harness", "mcp"]}),
        "a forgotten page is still a page; an unknown URL is only counted"
    );
    assert_eq!(names(&body["vocabulary"]), ["Harness", "mcp"]);
    assert_eq!(
        body["counts"],
        json!({"changed": 2, "unchanged": 0, "unknown": 1})
    );
    let tagged = server.library();
    assert_eq!(tags_of(&tagged, A), json!(["Harness", "mcp"]));
    assert_eq!(tags_of(&tagged, B), json!([]));
    assert_eq!(names(&tagged["vocabulary"]), ["Harness", "mcp"]);
    assert_eq!(
        state(&fx)["tags"][A],
        json!({"add": ["Harness", "mcp"], "remove": []})
    );

    // The same call with the lists swapped is the undo.
    let reply = server.post(
        "/api/tags",
        &json!({"urls": [A, HIDDEN], "remove": ["Harness", "MCP"]}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.json()["tags"], json!({A: [], HIDDEN: []}));
    let mut undone = without_generated_at(server.library());
    // What the undo leaves behind: the vocabulary entries the add created.
    assert_eq!(names(&undone["vocabulary"]), ["Harness", "mcp"]);
    undone["vocabulary"] = json!([]);
    assert_eq!(
        undone,
        without_generated_at(original),
        "tag then untag is the identity"
    );
    assert_eq!(state(&fx)["forgotten"], json!([HIDDEN]));
}

#[test]
fn a_bulk_tag_reports_the_pages_to_undo() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    assert_eq!(
        server
            .post("/api/tags", &json!({"urls": [A], "add": ["Skills"]}))
            .status,
        200
    );
    let before = without_generated_at(server.library());
    let reply = server.post("/api/tags", &json!({"urls": [A, B, A], "add": ["skills"]}));
    assert_eq!(reply.status, 200);
    let body = reply.json();
    assert_eq!(body["urls"], json!([B]), "A already had it");
    assert_eq!(body["tags"], json!({A: ["Skills"], B: ["Skills"]}));
    assert_eq!(
        body["counts"],
        json!({"changed": 1, "unchanged": 1, "unknown": 0})
    );
    let reply = server.post(
        "/api/tags",
        &json!({"urls": body["urls"], "remove": ["skills"]}),
    );
    assert_eq!(reply.status, 200);
    // Tagging B was a use, and removing it again keeps that.
    let mut expected = before;
    expected["vocabulary"][0]["used_at"] = body["vocabulary"][0]["used_at"].clone();
    assert_eq!(without_generated_at(server.library()), expected);
}

#[test]
fn a_retired_tag_leaves_the_library_but_not_the_state_file() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let reply = server.post(
        "/api/vocabulary",
        &json!({"create": ["Harness", " Old  tag "]}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(names(&reply.json()["vocabulary"]), ["Harness", "Old tag"]);
    assert_eq!(reply.json().as_object().unwrap().len(), 2);
    assert_eq!(
        server
            .post(
                "/api/tags",
                &json!({"urls": [A, B], "add": ["old TAG", "Harness"]})
            )
            .status,
        200
    );

    let reply = server.post("/api/vocabulary", &json!({"retire": ["OLD TAG", "Never"]}));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(names(&reply.json()["vocabulary"]), ["Harness"]);
    let library = server.library();
    assert_eq!(names(&library["vocabulary"]), ["Harness"]);
    assert_eq!(tags_of(&library, A), json!(["Harness"]));
    assert_eq!(tags_of(&library, B), json!(["Harness"]));
    let written = state(&fx);
    assert!(written["vocabulary"]["Old tag"]["retired_at"].is_string());
    assert_eq!(written["tags"][B]["add"], json!(["Harness", "Old tag"]));
    assert!(
        written["vocabulary"].get("Never").is_none(),
        "retiring an unknown name creates nothing"
    );

    // Creating it again brings it back, on the pages that had it.
    let reply = server.post("/api/vocabulary", &json!({"create": ["old tag"]}));
    assert_eq!(names(&reply.json()["vocabulary"]), ["Harness", "Old tag"]);
    assert_eq!(tags_of(&server.library(), B), json!(["Harness", "Old tag"]));
}

#[test]
fn creating_names_answers_the_vocabularys_spelling_of_each() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let reply = server.post(
        "/api/vocabulary",
        &json!({"create": ["Harness", "Old tag"]}),
    );
    assert_eq!(reply.json()["resolved"], json!(["Harness", "Old tag"]));
    server.post("/api/vocabulary", &json!({"retire": ["Old tag"]}));

    // Made, already there and brought back; a repeat in any case counts once.
    let reply = server.post(
        "/api/vocabulary",
        &json!({"create": [" new   name ", "HARNESS", "old TAG", "harness", "New Name"]}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.json()["resolved"],
        json!(["new name", "Harness", "Old tag"])
    );
    assert_eq!(
        names(&reply.json()["vocabulary"]),
        ["Harness", "new name", "Old tag"]
    );
    let again = server.post("/api/vocabulary", &json!({"create": ["NEW NAME"]}));
    assert_eq!(again.json()["resolved"], json!(["new name"]));

    let before = fs::read(fx.root.join("library.json")).unwrap();
    let refused = server.post("/api/vocabulary", &json!({"create": ["Fine", "  "]}));
    assert_eq!(refused.status, 400, "{}", refused.text());
    assert!(refused.json().get("resolved").is_none());
    assert_eq!(
        fs::read(fx.root.join("library.json")).unwrap(),
        before,
        "a refused create writes nothing, not even its valid names"
    );
}

#[test]
fn bad_tag_requests_are_refused_and_change_nothing() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let before = fs::read(fx.root.join("library.json")).unwrap();
    for (path, body, needle) in [
        ("/api/tags", json!({"urls": [A]}), "at least one tag"),
        (
            "/api/tags",
            json!({"urls": [A], "add": [], "remove": []}),
            "at least one tag",
        ),
        (
            "/api/tags",
            json!({"urls": [A], "add": ["  "]}),
            "it is empty",
        ),
        (
            "/api/tags",
            json!({"urls": [A], "add": ["x".repeat(41)]}),
            "longer than 40",
        ),
        (
            "/api/tags",
            json!({"urls": [A], "add": ["a\u{0}b"]}),
            "control character",
        ),
        (
            "/api/tags",
            json!({"urls": [A], "add": ["X"], "remove": ["x"]}),
            "both to add and to remove",
        ),
        ("/api/tags", json!({"add": ["X"]}), "expected"),
        ("/api/tags", json!({"urls": [A], "add": "X"}), "expected"),
        ("/api/vocabulary", json!({}), "at least one tag"),
        ("/api/vocabulary", json!({"create": [""]}), "it is empty"),
        (
            "/api/vocabulary",
            json!({"create": ["X"], "retire": ["x"]}),
            "both to create and to retire",
        ),
        ("/api/vocabulary", json!({"create": "X"}), "expected"),
    ] {
        let reply = server.post(path, &body);
        assert_eq!(reply.status, 400, "{path} {body}: {}", reply.text());
        let error = reply.json()["error"].as_str().unwrap().to_owned();
        assert!(error.contains(needle), "{path} {body}: {error}");
    }
    let host = format!("Host: {}", server.host());
    for path in ["/api/tags", "/api/vocabulary"] {
        let reply = server.raw("GET", path, std::slice::from_ref(&host), b"");
        assert_eq!(reply.status, 405);
        assert_eq!(reply.header("allow"), Some("POST"));
        // A form post needs no preflight, so it must never be read as JSON.
        let body = json!({"urls": [A], "add": ["X"], "create": ["X"]}).to_string();
        let reply = server.raw(
            "POST",
            path,
            &[
                host.clone(),
                "Content-Type: text/plain".to_owned(),
                format!("Content-Length: {}", body.len()),
            ],
            body.as_bytes(),
        );
        assert_eq!(reply.status, 415, "{path}");
    }
    assert_eq!(fs::read(fx.root.join("library.json")).unwrap(), before);
}

/// Rules 1 to 3 for the new routes. The bind is the listener's, shared by
/// every route, and `binds_only_the_ipv4_loopback_address` holds it; what a
/// route can get wrong on its own is the Host and Origin check, and CORS.
#[test]
fn foreign_host_or_origin_cannot_tag() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let before = fs::read(fx.root.join("library.json")).unwrap();
    for (path, body) in [
        ("/api/tags", json!({"urls": [A], "add": ["Evil"]})),
        ("/api/vocabulary", json!({"create": ["Evil"]})),
    ] {
        let body = body.to_string();
        for (host, origin) in [
            ("evil.example".to_owned(), None),
            (format!("127.0.0.1.evil.example:{}", server.port), None),
            (server.host(), Some("http://evil.example".to_owned())),
            (server.host(), Some("null".to_owned())),
            (
                server.host(),
                Some(format!("http://127.0.0.1:{}", server.port.wrapping_add(1))),
            ),
        ] {
            let mut headers = vec![format!("Host: {host}")];
            headers.extend(origin.iter().map(|o| format!("Origin: {o}")));
            headers.push("Content-Type: application/json".to_owned());
            headers.push(format!("Content-Length: {}", body.len()));
            let reply = server.raw("POST", path, &headers, body.as_bytes());
            assert_eq!(reply.status, 403, "{path} Host {host} Origin {origin:?}");
            assert!(!reply.text().contains("vocabulary"));
            for (name, _) in &reply.headers {
                assert!(!name.starts_with("access-control-"), "{path} sent {name}");
            }
        }
    }
    assert_eq!(
        fs::read(fx.root.join("library.json")).unwrap(),
        before,
        "a rejected request changed state"
    );
    let reply = server.post("/api/tags", &json!({"urls": [A], "add": ["Ours"]}));
    assert_eq!(reply.status, 200);
    for (name, _) in &reply.headers {
        assert!(!name.starts_with("access-control-"), "sent {name}");
    }
}

#[test]
fn exact_tag_undo_restores_mixed_decisions_without_overwriting_other_tags() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    server.post("/api/tags", &json!({"urls": [A, HIDDEN], "add": ["X"]}));
    server.post("/api/tags", &json!({"urls": [B], "add": ["Y"]}));
    let before = state(&fx);
    let edited = server
        .post(
            "/api/tags",
            &json!({"urls": [A, B, HIDDEN], "add": ["x", "y"]}),
        )
        .json();
    let used = state(&fx)["vocabulary"].clone();
    server.post("/api/tags", &json!({"urls": [A], "add": ["Unrelated"]}));
    let undone = server.post("/api/tags", &json!({"urls": [], "undo": edited["undo"]}));
    assert_eq!(undone.status, 200, "{}", undone.text());
    let mut expected = before;
    expected["tags"][A]["add"] = json!(["Unrelated", "X"]);
    let after = state(&fx);
    expected["vocabulary"]["Unrelated"] = after["vocabulary"]["Unrelated"].clone();
    // The edit's use of both names outlives its undo.
    for name in ["X", "Y"] {
        expected["vocabulary"][name]["used_at"] = used[name]["used_at"].clone();
    }
    assert_eq!(
        after, expected,
        "undo restores both decision lists, including forgotten pages"
    );
    let redone = server.post(
        "/api/tags",
        &json!({"urls": [], "undo": undone.json()["undo"]}),
    );
    assert_eq!(redone.status, 200);
    assert_eq!(tags_of(&server.library(), B), json!(["X", "Y"]));
}

#[test]
fn exact_tag_undo_restores_retirement_and_historical_page_membership() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    server.post("/api/tags", &json!({"urls": [A, HIDDEN], "add": ["Old"]}));
    server.post("/api/vocabulary", &json!({"retire": ["Old"]}));
    let before = state(&fx);
    let revived = server
        .post("/api/tags", &json!({"urls": [A, B], "add": ["OLD"]}))
        .json();
    assert_eq!(tags_of(&server.library(), HIDDEN), json!(["Old"]));
    let used = state(&fx)["vocabulary"]["Old"]["used_at"].clone();
    let undone = server.post("/api/tags", &json!({"urls": [], "undo": revived["undo"]}));
    assert_eq!(undone.status, 200, "{}", undone.text());
    let mut expected = before;
    expected["vocabulary"]["Old"]["used_at"] = used;
    assert_eq!(
        state(&fx),
        expected,
        "retirement time and historical assignments survive undo; the use does too"
    );
    server.post("/api/vocabulary", &json!({"create": ["Old"]}));
    assert_eq!(tags_of(&server.library(), A), json!(["Old"]));
    assert_eq!(tags_of(&server.library(), B), json!([]));
    assert_eq!(tags_of(&server.library(), HIDDEN), json!(["Old"]));
}

#[test]
fn malformed_tag_undo_is_atomic_and_unknown_urls_are_skipped() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    server.post("/api/tags", &json!({"urls": [A], "add": ["X"]}));
    let before = state(&fx);
    for decisions in [
        json!([{"url":A,"name":"X","add":false,"remove":false}, {"url":B,"name":"X","add":true,"remove":true}]),
        json!([{"url":A,"name":"X","add":false,"remove":false}, {"url":A,"name":"x","add":true,"remove":false}]),
        json!([{"url":A,"name":"","add":false,"remove":false}]),
    ] {
        let reply = server.post(
            "/api/tags",
            &json!({"urls":[],"undo":{"tags":decisions,"vocabulary":{}}}),
        );
        assert_eq!(reply.status, 400, "{}", reply.text());
        assert_eq!(state(&fx), before);
    }
    let reply = server.post("/api/tags", &json!({"urls":[],"undo":{"tags":[{"url":"https://unknown.test/","name":"X","add":true,"remove":false}],"vocabulary":{}}}));
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()["counts"]["unknown"], 1);
    assert_eq!(state(&fx), before);
}
