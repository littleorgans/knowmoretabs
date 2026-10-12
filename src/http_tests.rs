//! Regression checks for `http`.
//!
//! slice: triage, platforms
//! why: Loopback socket pairs and spent deadlines settle the wire's
//!      refusals and every time and byte bound without starting a server.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;

use super::*;

#[test]
fn host_must_be_loopback_by_name_and_this_port() {
    for host in ["127.0.0.1:7878", "localhost:7878", "LocalHost:7878"] {
        assert!(host_allowed(host, 7878), "{host}");
    }
    for host in [
        "127.0.0.1:7879",
        "127.0.0.1",
        "localhost",
        "evil.example:7878",
        "127.0.0.1.evil.example:7878",
        "localhost.evil.example:7878",
        "[::1]:7878",
        "0.0.0.0:7878",
        "127.0.0.2:7878",
        "",
        ":7878",
        "localhost:",
    ] {
        assert!(!host_allowed(host, 7878), "{host}");
    }
    assert!(host_allowed("localhost", 80));
    assert!(host_allowed("127.0.0.1:80", 80));
}

#[test]
fn origin_must_be_http_and_our_host() {
    assert!(origin_allowed("http://127.0.0.1:7878", 7878));
    assert!(origin_allowed("http://localhost:7878", 7878));
    for origin in [
        "null",
        "http://evil.example",
        "https://127.0.0.1:7878",
        "http://127.0.0.1:7879",
        "http://127.0.0.1:7878/",
        "file://",
        "",
    ] {
        assert!(!origin_allowed(origin, 7878), "{origin}");
    }
}

#[test]
fn head_parses_a_browser_request_and_rejects_garbage() {
    let head = Head::parse(
        b"POST /api/forget?x=1 HTTP/1.1\r\nHost: 127.0.0.1:7878\r\nContent-Type: application/json; charset=utf-8\r\n",
    )
    .ok()
    .unwrap();
    assert_eq!(head.method, "POST");
    assert_eq!(head.target, "/api/forget?x=1");
    assert_eq!(head.header("host"), Some("127.0.0.1:7878"));
    assert!(head.header("content-type").is_some_and(is_json));
    for bad in [
        &b"GET / HTTP/1.1 extra\r\n"[..],
        b"GET /\r\n",
        b"GET / HTTP/2\r\n",
        b"GET http://127.0.0.1/ HTTP/1.1\r\n",
        b"GET / HTTP/1.1\r\nno colon\r\n",
        b"GET / HTTP/1.1\r\nBad Name: x\r\n",
        b"GET / HTTP/1.1\r\n: x\r\n",
        b"GET / HTTP/1.1\r\n  \r\n",
        b"\xff\xfe / HTTP/1.1\r\n",
        b"",
    ] {
        let err = Head::parse(bad).err().unwrap();
        assert_eq!(
            err.status,
            Status::BadRequest,
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn json_content_type_ignores_parameters_and_case() {
    assert!(is_json("application/json"));
    assert!(is_json("Application/JSON; charset=utf-8"));
    assert!(!is_json("text/plain"));
    assert!(!is_json("application/x-www-form-urlencoded"));
    assert!(!is_json(""));
}

#[test]
fn responses_close_the_connection_and_carry_no_cors_headers() {
    let mut out = Vec::new();
    Response::method_not_allowed("GET")
        .write_to(&mut out)
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
    assert!(text.contains("\r\nAllow: GET\r\n"));
    assert!(text.contains("\r\nConnection: close\r\n"));
    assert!(!text.to_ascii_lowercase().contains("access-control"));
    assert!(text.ends_with("\r\n\r\n{\"error\":\"method not allowed\"}"));
}

/// The caller's limit, not `HEAD_LIMIT`, bounds the head, and each way a
/// message can fail to frame has its own name, so a server and a client
/// can each give it their own answer.
#[test]
fn framing_names_why_a_message_could_not_be_read() {
    let alive = || Instant::now() + IO_TIMEOUT;
    let head_of = |sent: &[u8], limit: usize| {
        let (mut socket, mut client) = socket_pair();
        client.write_all(sent).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        read_head(&mut socket, alive(), limit)
    };
    assert_eq!(head_of(b"", 64), Err(Framing::Closed));
    assert_eq!(
        head_of(b"HTTP/1.1 200 OK\r\n", 64),
        Err(Framing::Incomplete)
    );
    assert_eq!(head_of(&[b'x'; 65], 64), Err(Framing::TooLarge));
    assert_eq!(
        head_of(b"HTTP/1.1 200 OK\r\n\r\n{}", 64),
        Ok((b"HTTP/1.1 200 OK".to_vec(), b"{}".to_vec()))
    );

    let body_of = |fields: &str, rest: &[u8], limit: usize| {
        let (mut socket, client) = socket_pair();
        drop(client);
        let fields = Fields::parse(fields.split("\r\n")).unwrap();
        read_body(&mut socket, &fields, rest.to_vec(), alive(), limit)
    };
    assert_eq!(
        body_of("Transfer-Encoding: chunked", b"", 64),
        Err(Framing::Chunked)
    );
    assert_eq!(
        body_of("Content-Length: +1", b"", 64),
        Err(Framing::BadLength)
    );
    // The declared length is refused before a byte is read.
    assert_eq!(
        body_of("Content-Length: 65", b"", 64),
        Err(Framing::TooLarge)
    );
    assert_eq!(
        body_of("Content-Length: 1", b"xy", 64),
        Err(Framing::Overrun)
    );
    assert_eq!(
        body_of("Content-Length: 3", b"xy", 64),
        Err(Framing::Incomplete)
    );
    assert_eq!(body_of("Content-Length: 2", b"xy", 64), Ok(b"xy".to_vec()));
    assert_eq!(body_of("", b"", 64), Ok(Vec::new()));
}

fn socket_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (server, client)
}

/// The drain has to end while bytes are still arriving, so the dribbler
/// writes until it is told to stop rather than for a fixed number of
/// turns: a machine slow enough to outlast a counted dribbler used to
/// end the drain on the client's own close and then fail the ceiling it
/// was measured against. What proves the deadline is total is that
/// `respond` returned at all with a writer still going; the bounds either
/// side of that are a floor the drain cannot legitimately duck under and
/// a guard against a drain that never ends.
#[test]
fn review_linger_has_a_total_deadline_even_when_bytes_keep_arriving() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (mut socket, mut client) = socket_pair();
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let start = Instant::now();
        respond(&mut socket, &Response::error(Status::BadRequest, "test"));
        done.send(start.elapsed()).unwrap();
    });
    client
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    client.read_to_end(&mut Vec::new()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let dribbler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if client.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let elapsed = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the drain never ended while bytes kept arriving");
    stop.store(true, Ordering::Relaxed);
    // The floor cannot be late: `respond` starts LINGER after this clock
    // did, so a drain that runs its budget out lands beyond it however
    // slow the machine is, and only a drain that stopped reading early
    // comes in under. The ceiling is a runaway guard on the other side of
    // it — a drain that ended on some budget other than this one.
    assert!(elapsed >= LINGER, "ended at {elapsed:?}");
    assert!(elapsed < 5 * LINGER, "took {elapsed:?}");
    worker.join().unwrap();
    dribbler.join().unwrap();
}

#[test]
fn review_linger_never_reads_more_than_its_byte_limit() {
    let (mut socket, client) = socket_pair();
    // The writer works through a second handle so the connection outlives
    // it: this end stays open while the leftovers are counted, and a
    // client that closed on unread bytes would have the kernel reset the
    // connection out from under the count.
    let mut sender = client.try_clone().unwrap();
    let writer = std::thread::spawn(move || {
        // An unaligned first read used to make the last read overshoot.
        // Reading the response to its end is the starting gun: the server
        // shuts down its write side on the way into the drain, so that
        // EOF says the single byte is what the drain has in front of it.
        // A sleep here only guessed at the same thing.
        sender.write_all(b"x").unwrap();
        sender.read_to_end(&mut Vec::new()).unwrap();
        sender.write_all(&vec![b'x'; LINGER_LIMIT + 1024]).unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
    });
    respond(&mut socket, &Response::error(Status::BadRequest, "test"));
    // The drain leaves the tail of its own budget on the socket, and
    // counting what it did not take is not on that budget. macOS refuses
    // the option once the client's close has arrived, which is the one
    // case where it is not needed: the bytes and the EOF are both in hand
    // by then, so the reads below cannot block whatever the timeout says.
    let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
    let mut left = Vec::new();
    socket.read_to_end(&mut left).unwrap();
    assert!(left.len() <= LINGER_LIMIT, "drained {} bytes", left.len());
    assert!(left.len() >= 1025, "drained only {} bytes", left.len());
    writer.join().unwrap();
    drop(client);
}

/// One absolute instant covers the head and the body, so a client that
/// dribbles the head out slowly leaves the body nothing to read with. The
/// first version of this raced a clock to say so: a writer thread had to
/// wake from a 200ms sleep and land the head inside a 300ms deadline, and
/// the 100ms of slack between them was the whole test. A loaded runner
/// takes 100ms without noticing, which is how a documentation-only commit
/// failed on macOS.
///
/// The property does not need a clock to run, only an instant that has
/// already passed — which this constructs rather than waits for. Both
/// halves are asked to read a request that is sitting in the socket in
/// full, against a budget that is already gone, and both must come back
/// with nothing and leave the bytes where they are. A half that started a
/// budget of its own would read what it was sent and say so, which is a
/// different answer and not a slower one. Nothing here sleeps, nothing
/// waits on another thread, and no assertion is about how long anything
/// took; a machine slow enough to break this would have to make a spent
/// deadline unspent.
///
/// That the two halves are handed the *same* instant is `serve`'s to
/// keep, and `review_request_deadline_covers_silent_and_dribbling_clients`
/// is where a connection that dribbles a body is held to one budget
/// rather than two.
#[test]
fn review_head_and_body_share_one_deadline() {
    // The head as `read_head` hands it on: terminator stripped.
    const HEAD: &[u8] = b"GET / HTTP/1.1\r\nContent-Length: 1";

    let (mut socket, mut client) = socket_pair();
    client.write_all(HEAD).unwrap();
    client.write_all(b"\r\n\r\nx").unwrap();
    assert!(
        read_head(&mut socket, Instant::now(), HEAD_LIMIT) == Err(Framing::Closed),
        "a spent deadline read a head anyway"
    );

    let head = Head::parse(HEAD).ok().unwrap();
    let err = read_body(
        &mut socket,
        &head.fields,
        Vec::new(),
        Instant::now(),
        BODY_LIMIT,
    );
    assert_eq!(err, Err(Framing::Incomplete));

    // Both refusals were the clock and not a missing byte: the request is
    // still there, whole, for a budget that has time. This reads it back
    // through the same two functions, which is also the demonstration
    // that the socket was never the problem.
    let alive = Instant::now() + IO_TIMEOUT;
    let Ok((head, rest)) = read_head(&mut socket, alive, HEAD_LIMIT) else {
        panic!("the head is queued; a live budget has to find it");
    };
    assert_eq!(head, HEAD, "the head was consumed by a spent deadline");
    let head = Head::parse(&head).ok().unwrap();
    assert_eq!(
        read_body(&mut socket, &head.fields, rest, alive, BODY_LIMIT).ok(),
        Some(b"x".to_vec()),
        "the body byte was consumed by a spent deadline"
    );
}

/// A client that stops reading must not hold a worker. How much a kernel
/// will absorb before a write blocks is not a constant: one 32 MiB write
/// blocks on Linux and macOS and was swallowed whole by Windows loopback
/// autotuning, which made an earlier version of this pass on two
/// platforms and fail on the third. Neither version was wrong about the
/// guarantee; the first one measured buffers instead of it. The budget is
/// a clock, so this waits on the clock: keep writing until the deadline
/// takes the write away, which happens whether a buffer filled or not.
#[test]
fn review_response_write_has_a_total_deadline() {
    // A deadline already gone: no write is attempted at all, and no
    // socket is involved in saying so.
    let (mut socket, _client) = socket_pair();
    let mut spent = DeadlineWriter {
        stream: &mut socket,
        deadline: Instant::now(),
    };
    assert_eq!(
        spent.write(b"x").unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );

    let (mut socket, _client) = socket_pair();
    let mut writer = DeadlineWriter {
        stream: &mut socket,
        deadline: Instant::now() + Duration::from_millis(100),
    };
    let start = Instant::now();
    let chunk = vec![b'x'; 256 * 1024];
    // The cap is a runaway guard, not the mechanism: once 100ms have
    // passed the next `write` fails before it touches the socket, so a
    // kernel with room for everything still ends the loop on time.
    let stopped = (0..8192).find_map(|_| writer.write_all(&chunk).err());
    let err = stopped.expect("the deadline has to end the write");
    assert!(
        matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ),
        "{err:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "took {:?}",
        start.elapsed()
    );
}
