//! `serve`'s static files through the real binary: their types and headers,
//! the page's own policy, and the views it links against an export's.

mod common;

use std::fs;

use common::serve::{Server, archive};
use common::{Fixture, stderr};

const LIBRARY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[test]
fn assets_have_correct_types_and_no_network_implying_headers() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    for (path, content_type, marker) in [
        ("/", "text/html; charset=utf-8", "<!doctype html>"),
        ("/index.html", "text/html; charset=utf-8", "<!doctype html>"),
        ("/app.css", "text/css; charset=utf-8", "{"),
        ("/app.js", "text/javascript; charset=utf-8", "'use strict'"),
        ("/api/library", "application/json", "\"schema_version\""),
    ] {
        let reply = server.get(path);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.header("content-type"), Some(content_type), "{path}");
        assert_eq!(
            reply
                .header("content-length")
                .map(|v| v.parse::<usize>().unwrap()),
            Some(reply.body.len()),
            "{path}"
        );
        assert!(reply.text().to_lowercase().contains(marker), "{path}");
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(reply.header("cache-control"), Some("no-store"));
        assert_eq!(reply.header("connection"), Some("close"));
        assert_eq!(reply.header("referrer-policy"), Some("no-referrer"));
        // The page's own policy, as its header too, which alone can refuse
        // framing; and no inline style, unlike the model service's pages.
        let policy = content_type
            .starts_with("text/html")
            .then_some(LIBRARY_POLICY);
        assert_eq!(reply.header("content-security-policy"), policy, "{path}");
        for name in [
            "access-control-allow-origin",
            "set-cookie",
            "link",
            "location",
            "server",
        ] {
            assert!(reply.header(name).is_none(), "{path} sent {name}");
        }
    }
    let html = server.get("/").text();
    assert!(
        html.contains("<script id=\"library-data\" type=\"application/json\"></script>"),
        "the data element is present and empty, so the page fetches"
    );
    assert!(
        !html.contains("\"snapshots\":["),
        "no fixture data leaks into the served page"
    );
    assert!(html.contains(
        "content=\"default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
         img-src 'self'; base-uri 'none'; form-action 'none'\""
    ));
    assert_eq!(
        server.get("/app.css?v=1").status,
        200,
        "a query string is ignored"
    );
}

/// One nav for the Library and the model service's screens when served,
/// Pages marked; an export links only its own two views and holds nothing
/// of the service's or of the shell's unavailable page.
#[test]
fn the_served_page_links_four_views_and_an_export_two() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let html = server.get("/").text();
    assert!(html.contains(
        r#"<!-- nav:start --><nav class="views" aria-label="Views"><a href="/#pages" aria-current="page">Pages</a><a href="/model/#tag" id="nav-tag">Tag</a><a href="/model/#add" id="nav-add">Add link</a><a href="/#snapshots">Snapshots</a></nav><!-- nav:end -->"#
    ));
    assert!(
        !html.contains("onsubmit"),
        "no inline handler the policy blocks"
    );
    let script = server.get("/unavailable.js");
    assert_eq!(script.status, 200);
    assert_eq!(
        script.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(server.get("/unavailable.html").status, 404);
    let dir = fx.home.path().join("site");
    let output = fx.run(&["export", dir.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["app.css", "app.js", "index.html"]);
    let page = fs::read_to_string(dir.join("index.html")).unwrap();
    assert!(page.contains(
        r##"<nav class="views" aria-label="Views"><a href="#pages" aria-current="page">Pages</a><a href="#snapshots">Snapshots</a></nav>"##
    ));
    assert!(!page.contains("/model/") && !page.contains("unavailable"));
}
