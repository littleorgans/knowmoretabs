//! `serve`'s library contract through the real binary: the served shape,
//! forget and restore over the API, the state file they write and the
//! snapshots they must never touch.

mod common;

use std::fs;

use common::serve::{A, B, HIDDEN, Server, archive, page, without_generated_at};
use common::{Fixture, fingerprint, state, stderr};
use serde_json::json;

#[test]
fn library_has_the_serve_shape_and_forget_flips_it() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let original = server.library();
    assert_eq!(
        original["stats"],
        json!({"pages": 3, "snapshots": 2, "domains": 3, "sightings": 4, "forgotten": 1})
    );
    assert_eq!(page(&original, HIDDEN)["forgotten"], true);
    assert!(
        page(&original, A).get("forgotten").is_none(),
        "absent means false"
    );
    let hidden_index = original["pages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|p| p["url"] == HIDDEN)
        .unwrap();
    assert!(
        original["snapshots"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab[0] == hidden_index),
        "a forgotten page keeps its sightings in serve mode"
    );
    assert!(
        !original["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["url"] == "http://localhost:3000/")
    );

    let reply = server.post("/api/forget", &json!({"urls": [A, B]}));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert_eq!(
        reply.json(),
        json!({
            "urls": [A, B],
            "counts": {"changed": 2, "unchanged": 0, "unknown": 0, "forgotten": 3},
            "forgotten": [A, B]
        })
    );
    let after = server.library();
    assert_eq!(page(&after, A)["forgotten"], true);
    assert_eq!(page(&after, B)["forgotten"], true);
    assert_eq!(after["stats"]["forgotten"], 3);
    assert_eq!(state(&fx)["forgotten"], json!([A, B, HIDDEN]));

    // The same call with the sign flipped is the undo.
    let reply = server.post("/api/restore", &json!({"urls": [A, B]}));
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()["restored"], json!([A, B]));
    assert_eq!(reply.json()["urls"], json!([A, B]));
    assert_eq!(
        without_generated_at(server.library()),
        without_generated_at(original),
        "forget then restore is the identity"
    );
    assert_eq!(
        state(&fx),
        json!({"schema_version": 1, "forgotten": [HIDDEN]})
    );
}

#[test]
fn forgetting_twice_is_idempotent_and_unknown_urls_are_reported() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [A]})).status,
        200
    );
    let reply = server.post("/api/forget", &json!({"urls": [A, A]}));
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.json(),
        json!({
            "urls": [],
            "counts": {"changed": 0, "unchanged": 1, "unknown": 0, "forgotten": 2},
            "forgotten": []
        })
    );
    assert_eq!(state(&fx)["forgotten"], json!([A, HIDDEN]));

    // A page the archive does not hold is reported, not refused: the list
    // on the page may be older than the archive, and a bulk undo must not
    // fail as a whole because one row went stale.
    let reply = server.post(
        "/api/forget",
        &json!({"urls": ["https://nowhere.test/", "http://localhost:3000/", B]}),
    );
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()["urls"], json!([B]));
    assert_eq!(reply.json()["counts"]["unknown"], 2);
    assert_eq!(state(&fx)["forgotten"], json!([A, B, HIDDEN]));
}

#[test]
fn two_concurrent_forgets_both_survive() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    std::thread::scope(|scope| {
        for url in [A, B] {
            let server = &server;
            scope.spawn(move || {
                let reply = server.post("/api/forget", &json!({"urls": [url]}));
                assert_eq!(reply.status, 200, "{}", reply.text());
                assert_eq!(reply.json()["urls"], json!([url]));
            });
        }
    });
    assert_eq!(state(&fx)["forgotten"], json!([A, B, HIDDEN]));
    let library = server.library();
    assert_eq!(page(&library, A)["forgotten"], true);
    assert_eq!(page(&library, B)["forgotten"], true);
}

#[test]
fn snapshots_are_never_touched() {
    let fx = Fixture::new();
    archive(&fx);
    let before = fingerprint(&fx.root.join("snapshots"));
    let server = Server::start(&fx);
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [A, B]})).status,
        200
    );
    assert_eq!(
        server.post("/api/restore", &json!({"urls": [A]})).status,
        200
    );
    server.library();
    assert_eq!(fingerprint(&fx.root.join("snapshots")), before);
}

#[test]
fn damaged_state_refuses_to_start_and_is_a_500_if_damaged_while_running() {
    let fx = Fixture::new();
    archive(&fx);
    let good = fs::read(fx.root.join("library.json")).unwrap();
    fs::write(fx.root.join("library.json"), b"{").unwrap();
    let output = fx.run(&["serve", "--port", "0"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("cannot read library state"),
        "{}",
        stderr(&output)
    );

    fs::write(fx.root.join("library.json"), &good).unwrap();
    let server = Server::start(&fx);
    fs::write(fx.root.join("library.json"), b"{").unwrap();
    let reply = server.get("/api/library");
    assert_eq!(reply.status, 500);
    assert!(
        reply.json()["error"]
            .as_str()
            .unwrap()
            .contains("cannot read library state")
    );
    let reply = server.post("/api/forget", &json!({"urls": [A]}));
    assert_eq!(reply.status, 500);
    assert_eq!(fs::read(fx.root.join("library.json")).unwrap(), b"{");
}
