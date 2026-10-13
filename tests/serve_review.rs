//! The adversarial review of `knowmoretabs serve` through the real binary
//! and raw sockets: head boundaries and split terminators, framing and
//! header ambiguities, body lengths, authorities on every route, the request
//! deadline, and the archive lock under contention and stalled clients.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use common::Fixture;
use common::serve::{A, B, HIDDEN, Reply, Server, archive, state};
use serde_json::{Value, json};

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
