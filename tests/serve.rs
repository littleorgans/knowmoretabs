//! `knowmoretabs serve` through the real binary and a raw TCP client: the
//! three security rules, the serve-shaped contract, forget and restore over
//! the API, and every malformed request the server must survive.

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
        let mut child = fx
            .command()
            .args(["serve", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn serve");
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
