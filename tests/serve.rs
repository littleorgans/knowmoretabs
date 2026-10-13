//! `knowmoretabs serve` through the real binary and a raw TCP client: the
//! three security rules, every malformed request the server must survive,
//! and the adversarial review of its framing, deadlines and archive lock.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use common::serve::{A, B, HIDDEN, Reply, Server, archive, page, state};
use common::{Fixture, stderr};
use serde_json::{Value, json};

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
        "/api/content?url=https%3A%2F%2Fa.test%2Fone",
        "/api/forget",
        "/api/restore",
        "/api/tags",
        "/api/vocabulary",
        "/api/image/0000000000000000000000000000000000000000000000000000000000000000",
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
                        assert_eq!(bytes, b"");
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
    assert_eq!(fx.staging_dirs(), Vec::<PathBuf>::new());
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
