//! `knowmoretabs serve` through the real binary and a raw TCP client: the
//! three security rules, and every malformed request the server must survive.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::Stdio;
use std::time::Duration;

use common::serve::{A, HIDDEN, Server, archive, page, state};
use common::{Fixture, stderr};
use serde_json::json;

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
