//! `knowmoretabs serve` through the real binary and a raw TCP client: the
//! three security rules, the serve-shaped contract, forget, restore and
//! tagging over the API, and every malformed request the server must survive.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::{Child, Stdio};
use std::time::Duration;

use common::{Fixture, fingerprint, stderr, write_snapshot};
use serde_json::{Value, json};

const A: &str = "https://a.test/one";
const B: &str = "https://b.test/two";
const HIDDEN: &str = "https://hidden.test/page";

fn archive(fx: &Fixture) {
    write_snapshot(
        &fx.root,
        "2026-01-01-000000Z",
        "2026-01-01T00:00:00Z",
        &[
            (1, A, "A"),
            (2, HIDDEN, "Hidden"),
            (3, "http://localhost:3000/", "Dev"),
        ],
    );
    write_snapshot(
        &fx.root,
        "2026-02-01-000000Z",
        "2026-02-01T00:00:00Z",
        &[(1, A, "A again"), (2, B, "B")],
    );
    fs::create_dir_all(&fx.root).unwrap();
    fs::write(
        fx.root.join("library.json"),
        format!(r#"{{"schema_version":1,"forgotten":["{HIDDEN}"]}}"#),
    )
    .unwrap();
}

/// The binary serving on an ephemeral port, killed on drop.
struct Server {
    child: Child,
    port: u16,
    url: String,
}

impl Server {
    fn start(fx: &Fixture) -> Self {
        Self::from_child(
            fx.command()
                .args(["serve", "--port", "0"])
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn serve"),
        )
    }

    /// Takes the port from the banner's first line. The reader is dropped
    /// here, closing the pipe while the server is still writing: a server
    /// that treats that as fatal dies, which is what
    /// `stdout_that_goes_nowhere_does_not_stop_the_server` pins down.
    fn from_child(mut child: Child) -> Self {
        let mut first = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut first)
            .unwrap();
        let url = first
            .trim()
            .strip_prefix("Your library is at ")
            .unwrap_or_else(|| panic!("unexpected first line: {first:?}"))
            .to_owned();
        let port: u16 = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches('/')
            .parse()
            .unwrap_or_else(|_| panic!("no port in {url}"));
        Self { child, port, url }
    }

    fn origin(&self) -> String {
        self.url.trim_end_matches('/').to_owned()
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// A raw request to the server; `headers` are sent verbatim after the
    /// request line, so a test controls Host, Origin and framing exactly.
    fn raw(&self, method: &str, path: &str, headers: &[String], body: &[u8]) -> Reply {
        let mut request = format!("{method} {path} HTTP/1.1\r\n");
        for header in headers {
            request.push_str(header);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        // A server that rejects before reading the body may close first.
        let _ = stream.write_all(body);
        let _ = stream.shutdown(Shutdown::Write);
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).expect("read response");
        Reply::parse(&bytes)
    }

    fn get(&self, path: &str) -> Reply {
        self.raw("GET", path, &[format!("Host: {}", self.host())], b"")
    }

    fn post(&self, path: &str, body: &Value) -> Reply {
        let body = body.to_string();
        self.raw(
            "POST",
            path,
            &[
                format!("Host: {}", self.host()),
                format!("Origin: {}", self.origin()),
                "Content-Type: application/json".to_owned(),
                format!("Content-Length: {}", body.len()),
            ],
            body.as_bytes(),
        )
    }

    fn library(&self) -> Value {
        let reply = self.get("/api/library");
        assert_eq!(reply.status, 200, "{}", reply.text());
        reply.json()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn parse(bytes: &[u8]) -> Self {
        let split = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| {
                panic!(
                    "no header terminator in {:?}",
                    String::from_utf8_lossy(bytes)
                )
            });
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap();
        let status: u16 = status_line.split(' ').nth(1).unwrap().parse().unwrap();
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.trim().to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        Self {
            status,
            headers,
            body: bytes[split + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }
}

fn state(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fx.root.join("library.json")).unwrap()).unwrap()
}

fn page<'a>(library: &'a Value, url: &str) -> &'a Value {
    library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["url"] == url)
        .unwrap_or_else(|| panic!("{url} not in pages"))
}

fn without_generated_at(mut library: Value) -> Value {
    library.as_object_mut().unwrap().remove("generated_at");
    library
}

// --- Rule 1: bound to 127.0.0.1 and nothing else --------------------------

#[test]
fn binds_only_the_ipv4_loopback_address() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    assert!(server.url.starts_with("http://127.0.0.1:"));
    assert!(TcpStream::connect(("127.0.0.1", server.port)).is_ok());
    // Not the IPv6 loopback, which a bind to `localhost` or `::` would cover.
    assert!(
        TcpStream::connect_timeout(
            &SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], server.port)),
            Duration::from_secs(2)
        )
        .is_err(),
        "the server answered on [::1]"
    );
    // Not the address other machines reach this one by, when there is one.
    // No packet is sent: connecting a UDP socket only chooses the interface.
    let outward = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| {
            s.connect("192.0.2.1:9")
                .ok()
                .and_then(|()| s.local_addr().ok())
        })
        .map(|addr| addr.ip())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
    if let Some(ip) = outward {
        assert!(
            TcpStream::connect_timeout(&SocketAddr::new(ip, server.port), Duration::from_secs(2))
                .is_err(),
            "the server answered on {ip}"
        );
    }
}

#[test]
fn a_port_in_use_is_an_actionable_error() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let output = fx.run(&["serve", "--port", &server.port.to_string()]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(
        err.contains(&format!("cannot listen on 127.0.0.1:{}", server.port))
            && err.contains("--port"),
        "{err}"
    );
}

// --- Rule 2: the Host header must name this server -------------------------

#[test]
fn host_header_must_name_this_server() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    for host in [
        Some("evil.example"),
        Some(&*format!("evil.example:{}", server.port)),
        Some(&*format!("127.0.0.1.evil.example:{}", server.port)),
        Some(&*format!("127.0.0.1:{}", server.port.wrapping_add(1))),
        Some("127.0.0.1"),
        None,
    ] {
        let headers: Vec<String> = host.map(|h| format!("Host: {h}")).into_iter().collect();
        let reply = server.raw("GET", "/api/library", &headers, b"");
        assert_eq!(reply.status, 403, "Host {host:?}: {}", reply.text());
        assert!(
            !reply.text().contains("pages"),
            "leaked the library to {host:?}"
        );

        let body = json!({"urls": [A]}).to_string();
        let mut headers = headers.clone();
        headers.push("Content-Type: application/json".to_owned());
        headers.push(format!("Content-Length: {}", body.len()));
        let reply = server.raw("POST", "/api/forget", &headers, body.as_bytes());
        assert_eq!(reply.status, 403, "Host {host:?}");
    }
    assert_eq!(
        state(&fx)["forgotten"],
        json!([HIDDEN]),
        "a rejected request changed state"
    );
    for host in [
        format!("127.0.0.1:{}", server.port),
        format!("localhost:{}", server.port),
        format!("LOCALHOST:{}", server.port),
    ] {
        let reply = server.raw("GET", "/api/library", &[format!("Host: {host}")], b"");
        assert_eq!(reply.status, 200, "Host {host}");
    }
}

// --- Rule 3: a foreign Origin is refused ------------------------------------

#[test]
fn foreign_origin_is_rejected_and_changes_nothing() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let body = json!({"urls": [A]}).to_string();
    for origin in [
        "http://evil.example".to_owned(),
        "null".to_owned(),
        format!("https://127.0.0.1:{}", server.port),
        format!("http://127.0.0.1:{}", server.port.wrapping_add(1)),
        format!("http://127.0.0.1.evil.example:{}", server.port),
    ] {
        let headers = [
            format!("Host: {}", server.host()),
            format!("Origin: {origin}"),
            "Content-Type: application/json".to_owned(),
            format!("Content-Length: {}", body.len()),
        ];
        let reply = server.raw("POST", "/api/forget", &headers, body.as_bytes());
        assert_eq!(reply.status, 403, "Origin {origin}: {}", reply.text());
        let reply = server.raw("GET", "/api/library", &headers[..2], b"");
        assert_eq!(reply.status, 403, "Origin {origin} on GET");
        assert!(!reply.text().contains("pages"));
    }
    assert_eq!(state(&fx)["forgotten"], json!([HIDDEN]));
    assert!(
        !page(&server.library(), A)["forgotten"]
            .as_bool()
            .unwrap_or(false)
    );

    // Our own origin, under either name, is what the page sends.
    for origin in [server.origin(), format!("http://localhost:{}", server.port)] {
        let headers = [
            format!("Host: {}", server.host()),
            format!("Origin: {origin}"),
            "Content-Type: application/json".to_owned(),
            format!("Content-Length: {}", body.len()),
        ];
        let reply = server.raw("POST", "/api/restore", &headers, body.as_bytes());
        assert_eq!(reply.status, 200, "Origin {origin}: {}", reply.text());
    }
}

#[test]
fn no_cors_headers_ever() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let origins = [server.origin(), "http://evil.example".to_owned()];
    for origin in &origins {
        for path in ["/", "/app.js", "/api/library", "/missing"] {
            let reply = server.raw(
                "GET",
                path,
                &[
                    format!("Host: {}", server.host()),
                    format!("Origin: {origin}"),
                ],
                b"",
            );
            for (name, _) in &reply.headers {
                assert!(
                    !name.starts_with("access-control-"),
                    "{path} with Origin {origin} sent {name}"
                );
            }
        }
    }
}

// --- The contract, forget and restore ---------------------------------------

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

// --- Tags --------------------------------------------------------------------

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
            assert_eq!(entry.as_object().unwrap().len(), 2, "{entry}");
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

    let reply = server.post("/api/tags", &json!({"undo": undo, "urls": []}));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        without_generated_at(server.library()),
        without_generated_at(library),
        "the undo brings both suggestions back"
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
    assert_eq!(without_generated_at(server.library()), before);
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
    assert_eq!(reply.json().as_object().unwrap().len(), 1);
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

// --- Malformed requests -----------------------------------------------------

/// Method, path, extra headers, body, expected status.
type Case = (&'static str, &'static str, Vec<String>, &'static [u8], u16);

fn bad_request_cases(host: &str) -> Vec<Case> {
    let host = || host.to_owned();
    let json = || "Content-Type: application/json".to_owned();
    let length = |n: usize| format!("Content-Length: {n}");
    vec![
        (
            "POST",
            "/api/forget",
            vec![host(), json(), length(5)],
            b"{urls",
            400,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), json(), length(12)],
            b"{\"urls\":\"x\"}",
            400,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), json(), length(2)],
            b"[]",
            400,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), json(), "Content-Length: abc".to_owned()],
            b"",
            400,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), json(), "Transfer-Encoding: chunked".to_owned()],
            b"0\r\n\r\n",
            411,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), "Content-Type: text/plain".to_owned(), length(14)],
            b"{\"urls\":[\"x\"]}",
            415,
        ),
        (
            "POST",
            "/api/forget",
            vec![host(), length(14)],
            b"{\"urls\":[\"x\"]}",
            415,
        ),
        // Declared far over the cap: refused before a byte of body is read.
        (
            "POST",
            "/api/forget",
            vec![host(), json(), length(5_000_000)],
            b"",
            413,
        ),
        ("GET", "/api/forget", vec![host()], b"", 405),
        ("POST", "/api/library", vec![host(), length(0)], b"", 405),
        ("DELETE", "/", vec![host()], b"", 405),
        ("GET", "/api/nothing", vec![host()], b"", 404),
        ("GET", "/fixtures/library.json", vec![host()], b"", 404),
        ("GET", "/../Cargo.toml", vec![host()], b"", 404),
        ("GET", "/index.html/..", vec![host()], b"", 404),
    ]
}

#[test]
fn bad_requests_get_the_right_status_and_the_server_survives() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let host = format!("Host: {}", server.host());
    for (method, path, headers, body, expected) in bad_request_cases(&host) {
        let reply = server.raw(method, path, &headers, body);
        assert_eq!(reply.status, expected, "{method} {path}: {}", reply.text());
        assert_eq!(reply.header("content-type"), Some("application/json"));
        assert!(reply.json()["error"].is_string());
    }
    let reply = server.raw("GET", "/api/forget", std::slice::from_ref(&host), b"");
    assert_eq!(reply.header("allow"), Some("POST"));
    assert_eq!(
        state(&fx)["forgotten"],
        json!([HIDDEN]),
        "no bad request changed state"
    );
    assert_eq!(server.get("/api/library").status, 200, "still serving");
}

#[test]
fn unreadable_heads_and_silent_connections_do_not_stop_the_server() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let send = |bytes: &[u8]| {
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let _ = stream.write_all(bytes);
        let _ = stream.shutdown(Shutdown::Write);
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        String::from_utf8_lossy(&reply).into_owned()
    };
    assert!(send(b"\xff\xfe\x00 not http\r\n\r\n").starts_with("HTTP/1.1 400 "));
    assert!(send(b"GET / HTTP/1.1\r\nHost: 127.0.0.1").starts_with("HTTP/1.1 400 "));
    let huge = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nX-Pad: {}\r\n\r\n",
        server.host(),
        "a".repeat(20_000)
    );
    let reply = send(huge.as_bytes());
    assert!(reply.starts_with("HTTP/1.1 431 "), "{reply}");
    // A connection that sends nothing (a browser preconnect) is dropped quietly.
    assert_eq!(send(b""), "");
    assert_eq!(server.get("/api/library").status, 200, "still serving");
}

/// The banner goes to stdout, and stdout can fail: `knowmoretabs serve |
/// head -1` closes the pipe once it has the URL, and a full disk or a
/// closed terminal fail the same way. `println!` panics on all of them, on
/// the main thread, after the port is bound and accepting, so the whole
/// server went down to report that a greeting had nowhere to go.
///
/// The read end is closed before the child is spawned, so the first write
/// fails on every platform rather than racing the child the way a reader
/// that takes one line and drops does: that race is lost about a third of
/// the time on Linux and essentially never on macOS.
#[test]
fn stdout_that_goes_nowhere_does_not_stop_the_server() {
    let fx = Fixture::new();
    archive(&fx);
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let mut child = fx
        .command()
        .args(["--verbose", "serve", "--port", "0"])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    // Read the actual bound port after the failed stdout writes. Reserving
    // then releasing a port let other tests take it before the child bound.
    let mut first = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let url = first
        .trim()
        .strip_prefix("knowmoretabs: listening on ")
        .unwrap_or_else(|| panic!("server did not announce its listener: {first:?}"))
        .to_owned();
    let port = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let mut server = Server { child, port, url };
    assert_eq!(server.get("/api/library").status, 200, "serving");
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [A]})).status,
        200,
        "still mutating"
    );
    assert_eq!(state(&fx)["forgotten"], json!([A, HIDDEN]));
    assert!(
        server.child.try_wait().expect("try_wait").is_none(),
        "the server exited"
    );
}

/// The same for stderr, which `--verbose` writes to from the connection
/// thread before the response: a panic there unwinds the thread and the
/// client gets a closed socket instead of its answer. stdout is a real
/// pipe here, so the banner still names the port.
#[test]
fn stderr_that_goes_nowhere_does_not_stop_the_server() {
    let fx = Fixture::new();
    archive(&fx);
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let mut server = Server::from_child(
        fx.command()
            .args(["--verbose", "serve", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::from(writer))
            .spawn()
            .expect("spawn serve"),
    );
    assert_eq!(server.get("/api/library").status, 200, "serving");
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [A]})).status,
        200,
        "still mutating"
    );
    assert!(
        server.child.try_wait().expect("try_wait").is_none(),
        "the server exited"
    );
}

// --- Assets -----------------------------------------------------------------

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
    assert!(html.contains("Content-Security-Policy"));
    assert_eq!(
        server.get("/app.css?v=1").status,
        200,
        "a query string is ignored"
    );
}

// --- Adversarial HTTP review -------------------------------------------------

/// The timeouts are the same ten seconds `Server::raw` uses, and they are
/// guards rather than measurements: no test here passes by being quick, so
/// the only thing a tighter one could do is turn a busy runner into a
/// failure. Tests that need a socket to go quiet for a moment, or to outlast
/// the server's own deadline, say so on the socket themselves.
fn review_socket(server: &Server) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
}

fn review_reply(stream: &mut TcpStream) -> Reply {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    Reply::parse(&bytes)
}

fn review_bytes(server: &Server, bytes: &[u8]) -> Reply {
    let mut stream = review_socket(server);
    stream.write_all(bytes).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    review_reply(&mut stream)
}

#[test]
fn review_head_boundaries_and_split_terminators() {
    let fx = Fixture::new();
    let server = Server::start(&fx);
    let prefix = format!(
        "GET /api/library HTTP/1.1\r\nHost: {}\r\nX: ",
        server.host()
    );
    for split in 1..=3 {
        let mut stream = review_socket(&server);
        // Place the split at the server's 1024-byte read boundary, and leave
        // the tail unsent until the client has observed no response.
        let head = format!(
            "{prefix}{}\r\n\r\n",
            "a".repeat(1024 - prefix.len() - split)
        );
        stream.write_all(&head.as_bytes()[..1024]).unwrap();
        // An answer here would mean the server had decided on an unterminated
        // head, and it holds the connection for ten seconds before it decides
        // anything, so a hundred milliseconds of silence is a hundred
        // milliseconds a loaded runner can only lengthen.
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let err = stream.read(&mut [0]).unwrap_err();
        assert!(matches!(
            err.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(&head.as_bytes()[1024..]).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        assert_eq!(
            review_reply(&mut stream).status,
            200,
            "split {split}/{}",
            4 - split
        );
    }
    for (size, expected) in [(16_384, 200), (16_385, 431), (17_000, 431)] {
        let head = format!("{prefix}{}\r\n\r\n", "a".repeat(size - prefix.len() - 4));
        assert_eq!(
            review_bytes(&server, head.as_bytes()).status,
            expected,
            "{size}"
        );
    }
    for line in [
        " / HTTP/1.1",
        "GET  HTTP/1.1",
        "GET / ",
        "GET /",
        "GET",
        "GET / HTTP/1.garbage",
        "G\tET / HTTP/1.1",
        "GET /a\0b HTTP/1.1",
    ] {
        let head = format!("{line}\r\nHost: {}\r\n\r\n", server.host());
        assert_eq!(
            review_bytes(&server, head.as_bytes()).status,
            400,
            "{line:?}"
        );
    }
    let absurd = format!(
        "GET /{} HTTP/1.1\r\nHost: {}\r\n\r\n",
        "a".repeat(17_000),
        server.host()
    );
    assert_eq!(review_bytes(&server, absurd.as_bytes()).status, 431);
    assert_eq!(review_bytes(&server, b"GET / HTTP/1.1\r\nHos").status, 400);
}

#[test]
fn review_framing_and_header_ambiguities() {
    let fx = Fixture::new();
    let server = Server::start(&fx);
    let prefix = format!("GET /api/library HTTP/1.1\r\nHost: {}\r\n", server.host());
    for fields in [
        "Content-Length: 0\r\ncontent-length: 1",
        "Content-Length: 1\r\nContent-Length: 0",
        "Content-Length: 0\r\nContent-Length: 0",
        "Content-Length: -1",
        "Content-Length: abc",
        "Content-Length: +0",
        "Content-Length: 18446744073709551616",
        "Content-Length:",
        "Content-Length: 0, 0",
        "Host: evil.test",
        "hOsT: localhost:1",
        "Origin: http://evil.test\r\nOrigin: null",
        " Host: evil.test",
        "Host : evil.test",
        "X: ok\r\n folded",
        "no colon",
        "X: a\0b",
        "X: a\rb",
        "X: a\nb",
        "X\0: a",
        "X@: a",
        "X: a\x7fb",
        "X: \u{000b}",
        "Content-Type: application/json\r\ncontent-type: text/plain",
    ] {
        let head = format!("{prefix}{fields}\r\n\r\n");
        assert_eq!(
            review_bytes(&server, head.as_bytes()).status,
            400,
            "{fields:?}"
        );
    }
    for (first, second) in [
        (server.origin(), "http://evil.test".into()),
        ("http://evil.test".into(), server.origin()),
    ] {
        let head = format!("{prefix}Origin: {first}\r\noRiGiN: {second}\r\n\r\n");
        assert_eq!(review_bytes(&server, head.as_bytes()).status, 400);
    }
    let evil_first = format!(
        "GET /api/library HTTP/1.1\r\nHost: evil.test\r\nHost: {}\r\n\r\n",
        server.host()
    );
    assert_eq!(review_bytes(&server, evil_first.as_bytes()).status, 400);
    for fields in ["", "Content-Length: 0000\r\n", "cOnTeNt-LeNgTh:\t0 \t\r\n"] {
        assert_eq!(
            review_bytes(&server, format!("{prefix}{fields}\r\n").as_bytes()).status,
            200
        );
    }
    // Leave the write half OPEN and send no body. These must answer now,
    // rather than succeed only because a test client half-closed its socket.
    for (fields, expected) in [
        ("Transfer-Encoding: chunked", 411),
        ("Transfer-Encoding: chunked\r\nContent-Length: 100", 411),
        ("Content-Length: 100\r\ntRaNsFeR-EnCoDiNg: chunked", 411),
        ("Content-Length: 4194305", 413),
        ("Content-Length: 18446744073709551615", 413),
    ] {
        let mut stream = review_socket(&server);
        stream
            .write_all(format!("{prefix}{fields}\r\n\r\n").as_bytes())
            .unwrap();
        assert_eq!(review_reply(&mut stream).status, expected, "{fields}");
    }
}

#[test]
fn review_body_lengths_and_no_second_request() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    for (length, body, expected) in [
        ("11", "{\"urls\":[]}", 200),
        ("00011", "{\"urls\":[]}", 200),
        ("12", "{\"urls\":[]}", 400),
        ("10", "{\"urls\":[]}", 400),
    ] {
        let request = format!(
            "POST /api/forget HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n\r\n{body}",
            server.host()
        );
        assert_eq!(
            review_bytes(&server, request.as_bytes()).status,
            expected,
            "{length}"
        );
    }
    let mut body = b"{\"urls\":[]}".to_vec();
    body.resize(4 * 1024 * 1024, b' ');
    assert_eq!(
        server
            .raw(
                "POST",
                "/api/forget",
                &[
                    format!("Host: {}", server.host()),
                    "Content-Type: application/json".into(),
                    format!("Content-Length: {}", body.len())
                ],
                &body
            )
            .status,
        200
    );
    // Bytes after the declared body can arrive with it or after the response;
    // either way this connection must never execute a second request.
    let next_body = json!({"urls": [A]}).to_string();
    let next = format!(
        "POST /api/forget HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{next_body}",
        server.host(),
        next_body.len()
    );
    let first = format!("GET /missing HTTP/1.1\r\nHost: {}\r\n\r\n", server.host());
    let reply = review_bytes(&server, format!("{first}{next}").as_bytes());
    assert!(matches!(reply.status, 400 | 404));
    let mut stream = review_socket(&server);
    stream.write_all(first.as_bytes()).unwrap();
    assert_eq!(review_reply(&mut stream).status, 404);
    let _ = stream.write_all(next.as_bytes());
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eq!(state(&fx)["forgotten"], json!([HIDDEN]));
}

#[test]
fn review_authorities_on_every_route_before_body_read() {
    let fx = Fixture::new();
    let server = Server::start(&fx);
    let port = server.port;
    let bad = [
        format!("127.0.0.1:{port}.evil.com"),
        format!("127.0.0.1:{port}@evil.com"),
        format!("localhost:{port}:{port}"),
        format!("localhost.:{port}"),
        format!("%6cocalhost:{port}"),
        format!("127%2e0%2e0%2e1:{port}"),
        format!("[::1]:{port}"),
        format!("[::ffff:127.0.0.1]:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        format!("127.1:{port}"),
        format!("2130706433:{port}"),
        format!("0x7f.1:{port}"),
        "127.0.0.1".into(),
        "localhost".into(),
        format!("localhost:+{port}"),
    ];
    for authority in &bad {
        for fields in [
            format!("Host: {authority}"),
            format!("Host: {}\r\nOrigin: http://{authority}", server.host()),
        ] {
            let head =
                format!("GET /api/library HTTP/1.1\r\n{fields}\r\nContent-Length: 100\r\n\r\n");
            let mut stream = review_socket(&server);
            stream.write_all(head.as_bytes()).unwrap();
            assert_eq!(review_reply(&mut stream).status, 403, "{fields:?}");
        }
    }
    for path in [
        "/",
        "/index.html",
        "/app.css",
        "/app.js",
        "/api/library",
        "/api/forget",
        "/api/restore",
        "/api/tags",
        "/api/vocabulary",
        "/missing",
    ] {
        for method in ["GET", "POST", "OPTIONS"] {
            for fields in [
                "Host: evil.test".into(),
                format!("Host: {}\r\nOrigin: http://evil.test", server.host()),
            ] {
                let mut stream = review_socket(&server);
                stream
                    .write_all(
                        format!(
                            "{method} {path} HTTP/1.1\r\n{fields}\r\nContent-Length: 100\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                assert_eq!(review_reply(&mut stream).status, 403, "{method} {path}");
            }
        }
    }
    for name in ["LOCALHOST", "LocalHost"] {
        let head = format!(
            "GET /api/library HTTP/1.1\r\nhOsT:\t{name}:{port} \t\r\noRiGiN: http://{name}:{port}\r\n\r\n"
        );
        assert_eq!(review_bytes(&server, head.as_bytes()).status, 200);
    }
}

#[test]
fn review_request_deadline_covers_silent_and_dribbling_clients() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    let fx = Fixture::new();
    let server = Server::start(&fx);
    let body_head = format!(
        "POST /api/forget HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n",
        server.host()
    );
    std::thread::scope(|scope| {
        for (head, dribble) in [
            ("", false),
            ("GET / HTTP/1.1\r\nX: ", false),
            ("GET / HTTP/1.1\r\nX: ", true),
            (body_head.as_str(), false),
            (body_head.as_str(), true),
        ] {
            let server = &server;
            scope.spawn(move || {
                // Before the connection, so the server's own deadline cannot
                // have started earlier than this clock did: the floor below
                // is then a floor on the server's ten seconds, not on
                // whatever was left of them by the time this thread ran.
                let start = Instant::now();
                let mut stream = review_socket(server);
                // Longer than the ceiling this asserts, so a connection the
                // server never ends fails on the assertion rather than on a
                // socket timeout that reads like a slow machine.
                stream
                    .set_read_timeout(Some(Duration::from_secs(25)))
                    .unwrap();
                stream.write_all(head.as_bytes()).unwrap();
                let done = AtomicBool::new(false);
                std::thread::scope(|scope| {
                    if dribble {
                        let mut writer = stream.try_clone().unwrap();
                        let done = &done;
                        scope.spawn(move || {
                            while !done.load(Ordering::Relaxed) {
                                if writer.write_all(b"x").is_err() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(250));
                            }
                        });
                    }
                    let mut bytes = Vec::new();
                    let result = stream.read_to_end(&mut bytes);
                    done.store(true, Ordering::Relaxed);
                    result.unwrap();
                    let elapsed = start.elapsed();
                    // The budget is ten seconds and it is one budget. The
                    // floor says a dribbler was not hung up on early; the
                    // ceiling says it bought nothing, and it sits between one
                    // budget and the two a head and a body would cost if each
                    // started its own — not up against the ten, where a busy
                    // runner decides the verdict.
                    assert!(elapsed >= Duration::from_secs(9), "ended at {elapsed:?}");
                    assert!(elapsed < Duration::from_secs(15), "took {elapsed:?}");
                    if head.is_empty() {
                        assert!(bytes.is_empty());
                    } else {
                        assert_eq!(Reply::parse(&bytes).status, 400);
                    }
                });
            });
        }
    });
}

#[test]
fn review_forget_save_and_export_serialize_under_contention() {
    use common::{SessionBuilder, assert_success};
    use std::sync::Barrier;
    let fx = Fixture::new();
    archive(&fx);
    fx.write_session(
        "Default",
        20,
        &SessionBuilder::new()
            .simple_tab(1, 2, A, "A")
            .simple_tab(1, 3, B, "B")
            .marker()
            .build(),
    );
    let server = Server::start(&fx);
    let gate = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(fx.root.join("lock"))
        .unwrap();
    gate.lock().unwrap();
    let barrier = Barrier::new(4);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            assert_eq!(
                server.post("/api/forget", &json!({"urls": [A]})).status,
                200
            );
        });
        let b = scope.spawn(|| {
            barrier.wait();
            assert_eq!(
                server.post("/api/forget", &json!({"urls": [B]})).status,
                200
            );
        });
        let save = scope.spawn(|| {
            barrier.wait();
            assert_success(&fx.run(&["save", "--force"]));
        });
        let mut export = fx
            .command()
            .arg("export")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        barrier.wait();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(state(&fx)["forgotten"], json!([HIDDEN]));
        assert_eq!(fx.snapshot_dirs().len(), 2);
        assert!(export.try_wait().unwrap().is_none());
        gate.unlock().unwrap();
        a.join().unwrap();
        b.join().unwrap();
        save.join().unwrap();
        assert!(export.wait().unwrap().success());
    });
    assert_eq!(state(&fx)["forgotten"], json!([A, B, HIDDEN]));
    assert_eq!(fx.snapshot_dirs().len(), 3);
    let html = fs::read_to_string(fx.root.join("export/index.html")).unwrap();
    let json = html
        .split("<script id=\"library-data\" type=\"application/json\">")
        .nth(1)
        .unwrap()
        .split("</script>")
        .next()
        .unwrap();
    let exported: Value = serde_json::from_str(json).unwrap();
    let visible = exported["pages"].as_array().unwrap().len();
    // Export may precede, follow, or fall between forgets, but its page list
    // and forgotten count must describe the same serialized state.
    assert_eq!(
        exported["stats"]["forgotten"].as_u64().unwrap() + visible as u64,
        3
    );
    assert!(fx.staging_dirs().is_empty());
}

#[test]
fn review_stalled_body_and_response_never_hold_archive_lock() {
    let fx = Fixture::new();
    archive(&fx);
    let server = Server::start(&fx);
    let mut stalled = review_socket(&server);
    stalled
        .write_all(
            format!(
                "POST /api/forget HTTP/1.1\r\nHost: {}\r\nContent-Length: 100\r\n\r\n",
                server.host()
            )
            .as_bytes(),
        )
        .unwrap();
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [A]})).status,
        200
    );
    // A response larger than the socket buffers guarantees that a client
    // which never reads will leave its worker blocked writing.
    let dir = fx.snapshot_dirs()[0].join("snapshot.json");
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&dir).unwrap()).unwrap();
    snapshot["tabs"][1]["title"] = Value::String("x".repeat(8 * 1024 * 1024));
    fs::write(&dir, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let mut reader = review_socket(&server);
    reader
        .write_all(
            format!(
                "GET /api/library HTTP/1.1\r\nHost: {}\r\n\r\n",
                server.host()
            )
            .as_bytes(),
        )
        .unwrap();
    // Waiting for that first byte is waiting for an eight-megabyte library to
    // be read, built and serialized by a debug binary, which is slow and gets
    // slower on a busy machine. Nothing is being timed: what the lock says at
    // the moment the response starts is the same answer however long the
    // server took to start it, so this guard is set where only a server that
    // never answers can hit it.
    reader
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let mut byte = [0];
    assert_eq!(reader.peek(&mut byte).unwrap(), 1, "response started");
    let gate = fs::File::open(fx.root.join("lock")).unwrap();
    gate.try_lock()
        .expect("response write must not retain the lock");
    gate.unlock().unwrap();
    assert_eq!(
        server.post("/api/forget", &json!({"urls": [B]})).status,
        200
    );
    assert_eq!(state(&fx)["forgotten"], json!([A, B, HIDDEN]));
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
    server.post("/api/tags", &json!({"urls": [A], "add": ["Unrelated"]}));
    let undone = server.post("/api/tags", &json!({"urls": [], "undo": edited["undo"]}));
    assert_eq!(undone.status, 200, "{}", undone.text());
    let mut expected = before;
    expected["tags"][A]["add"] = json!(["Unrelated", "X"]);
    let after = state(&fx);
    expected["vocabulary"]["Unrelated"] = after["vocabulary"]["Unrelated"].clone();
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
    let undone = server.post("/api/tags", &json!({"urls": [], "undo": revived["undo"]}));
    assert_eq!(undone.status, 200, "{}", undone.text());
    assert_eq!(
        state(&fx),
        before,
        "retirement time and historical assignments survive undo"
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

const C: &str = "https://c.test/three";
const D: &str = "https://d.test/four";
const E: &str = "https://e.test/five";

/// `archive`, a third snapshot with C and D, and History signals on some
/// tabs: A's in both of its snapshots (the newer must win), a referrer that
/// is a page (B), one that is forgotten (from B), one on this machine (from
/// C), one that is the page itself (E, as saved before capture dropped
/// those), and none at all on D.
fn archive_with_history(fx: &Fixture) {
    archive(fx);
    write_snapshot(
        &fx.root,
        "2026-03-01-000000Z",
        "2026-03-01T00:00:00Z",
        &[(1, C, "C"), (2, D, "D"), (3, E, "E")],
    );
    let signals = |visits: u64, term: &str, referrer: &str| {
        json!({
            "visits": visits, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834,
            "search": {"term": term, "hops": 1},
            "referrer": referrer
        })
    };
    for (id, url, history) in [
        (
            "2026-01-01-000000Z",
            A,
            signals(2, "older search", "https://older.test/"),
        ),
        (
            "2026-01-01-000000Z",
            HIDDEN,
            signals(4, "hidden search", "https://ref.test/"),
        ),
        ("2026-02-01-000000Z", A, signals(12, "secret search", B)),
        ("2026-02-01-000000Z", B, signals(1, "b search", HIDDEN)),
        (
            "2026-03-01-000000Z",
            C,
            signals(1, "c search", "http://localhost:3000/"),
        ),
        ("2026-03-01-000000Z", E, signals(1, "e search", E)),
    ] {
        let path = fx.root.join("snapshots").join(id).join("snapshot.json");
        let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for tab in snapshot["tabs"].as_array_mut().unwrap() {
            if tab["url"] == url {
                tab["history"] = history.clone();
            }
        }
        snapshot["history"] = json!({
            "path": "/profile/History", "bytes": 4096, "schema_version": 70,
            "newest_visit": "2026-01-01T00:00:00Z", "tabs_found": 2,
            "unavailable": [], "error": null, "skipped_by_request": false
        });
        fs::write(&path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
    }
}

/// The export's embedded library and every byte of the export as text.
fn export_to(fx: &Fixture, dir: &str, args: &[&str]) -> (Value, String) {
    let dir = fx.home.path().join(dir);
    let mut all = vec!["export", dir.to_str().unwrap()];
    all.extend_from_slice(args);
    let output = fx.run(&all);
    assert!(output.status.success(), "{}", stderr(&output));
    let html = fs::read_to_string(dir.join("index.html")).unwrap();
    let marker = "<script id=\"library-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html[start..].find("</script>").unwrap() + start;
    let library = serde_json::from_str(&html[start..end].replace("<\\/", "</")).unwrap();
    let text = fingerprint(&dir)
        .into_iter()
        .map(|(_, bytes, _)| String::from_utf8(bytes).unwrap())
        .collect();
    (library, text)
}

/// The library with every page's `history` taken out.
fn without_history(mut library: Value) -> Value {
    for page in library["pages"].as_array_mut().unwrap() {
        page.as_object_mut().unwrap().remove("history");
    }
    without_generated_at(library)
}

#[test]
fn serve_shows_each_pages_newest_history_signals() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    let library = Server::start(&fx).library();
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 12, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834,
            "search": {"term": "secret search", "hops": 1},
            "referrer": {"url": B, "title": "B"}
        })
    );
    // Serve lists forgotten pages flagged, so it names them as referrers too.
    assert_eq!(
        page(&library, B)["history"]["referrer"],
        json!({"url": HIDDEN, "title": "Hidden"})
    );
    assert_eq!(
        page(&library, HIDDEN)["history"]["search"]["term"],
        "hidden search"
    );
    let c = &page(&library, C)["history"];
    assert_eq!(c["visits"], 1);
    assert!(c.get("referrer").is_none(), "{c}");
    assert!(page(&library, D).get("history").is_none());
    let e = &page(&library, E)["history"];
    assert!(e.get("referrer").is_none(), "{e}");
    let text = library.to_string();
    assert!(!text.contains("older search") && !text.contains("localhost"));
}

#[test]
fn a_newer_snapshot_without_history_keeps_the_last_recorded_signals() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    write_snapshot(
        &fx.root,
        "2026-04-01-000000Z",
        "2026-04-01T00:00:00Z",
        &[(1, A, "A without new signals")],
    );
    let served = Server::start(&fx).library();
    let (exported, _) = export_to(&fx, "full", &["--with-history"]);
    for library in [served, exported] {
        let a = page(&library, A);
        assert_eq!(a["title"], "A without new signals");
        assert_eq!(a["history"]["visits"], 12);
        assert_eq!(a["history"]["search"]["term"], "secret search");
    }
}

#[test]
fn export_leaves_out_searches_and_referrers_unless_asked() {
    let fx = Fixture::new();
    archive_with_history(&fx);
    let (library, text) = export_to(&fx, "plain", &[]);
    assert_eq!(
        page(&library, A)["history"],
        json!({
            "visits": 12, "typed": 3,
            "first_visit": "2025-12-01T00:00:00Z", "last_visit": "2026-01-01T00:00:00Z",
            "foreground_seconds": 1834
        })
    );
    let json = library.to_string();
    assert!(
        !json.contains("\"search\"") && !json.contains("\"referrer\""),
        "{json}"
    );
    for secret in [
        "secret search",
        "b search",
        "c search",
        "hidden.test",
        "ref.test",
    ] {
        assert!(!text.contains(secret), "{secret} in the export");
    }

    let (library, text) = export_to(&fx, "full", &["--with-history"]);
    let a = &page(&library, A)["history"];
    assert_eq!(a["search"], json!({"term": "secret search", "hops": 1}));
    assert_eq!(a["referrer"], json!({"url": B, "title": "B"}));
    // An export never names a forgotten page, not even as where B came from.
    let b = &page(&library, B)["history"];
    assert_eq!(b["search"]["term"], "b search");
    assert!(b.get("referrer").is_none(), "{b}");
    assert!(!text.contains("hidden.test") && !text.contains("hidden search"));
}

/// The only thing History adds to the library or the export is each page's
/// `history`: the projection stays explicit, and the snapshot-level
/// provenance, older signals and every other field stay out.
#[test]
fn history_adds_only_the_page_history_to_the_library_and_the_export() {
    let before = Fixture::new();
    archive(&before);
    write_snapshot(
        &before.root,
        "2026-03-01-000000Z",
        "2026-03-01T00:00:00Z",
        &[(1, C, "C"), (2, D, "D"), (3, E, "E")],
    );
    let after = Fixture::new();
    archive_with_history(&after);

    let served = |fx: &Fixture| without_history(Server::start(fx).library());
    assert_eq!(served(&after), served(&before));
    for args in [&[][..], &["--with-history"][..]] {
        let (plain, _) = export_to(&before, "before", args);
        let (with, text) = export_to(&after, "after", args);
        assert_eq!(without_history(with), without_history(plain));
        assert!(!text.contains("/profile/History") && !text.contains("schema_version\":70"));
    }
    let library = Server::start(&after).library();
    assert!(!library.to_string().contains("/profile/History"));
}
