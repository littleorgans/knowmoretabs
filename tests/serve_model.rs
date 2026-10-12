//! `serve --model-port` through the real binary and a stand-in model service
//! on loopback: what reaches the service, what comes back to the page, and
//! every refusal on the way, with the shell still serving after each.

mod common;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Fixture;
use common::serve::{Reply, Server, archive};
use serde_json::json;

const RANK: &str = "/model/api/rank";

/// A stand-in model service: records each request's bytes, writes what
/// `answer` returns for it (nothing for `None`), and holds every connection
/// open, so only a declared length can end a reply.
struct Service {
    port: u16,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Service {
    fn start(answer: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + 'static) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for mut stream in listener.incoming().flatten() {
                let request = request(&mut stream);
                let reply = answer(&request);
                log.lock().unwrap().push(request);
                if let Some(reply) = reply {
                    let _ = stream.write_all(&reply);
                }
                held.push(stream);
            }
        });
        Self { port, seen }
    }

    /// Answers every request with the same bytes.
    fn replying(reply: Vec<u8>) -> Self {
        Self::start(move |_| Some(reply.clone()))
    }

    fn seen(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }

    fn shell(&self, fx: &Fixture) -> Server {
        Server::start_with(fx, &["--model-port", &self.port.to_string()])
    }
}

/// One request: the head, then as many body bytes as it declares.
fn request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0u8];
    while !bytes.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
        bytes.push(byte[0]);
    }
    let length = String::from_utf8_lossy(&bytes)
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: ")?.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).unwrap();
    bytes.extend(body);
    bytes
}

/// A reply head and body as the service writes them.
fn reply(status: &str, fields: &[String], body: &str) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for field in fields {
        head.push_str(field);
        head.push_str("\r\n");
    }
    format!("{head}\r\n{body}").into_bytes()
}

/// The fields of a JSON reply over this archive.
fn json_fields(fx: &Fixture, body: &str) -> Vec<String> {
    vec![
        "Content-Type: application/json; charset=utf-8".to_owned(),
        format!("Content-Length: {}", body.len()),
        format!("X-KMT-Archive: {}", identity(fx)),
    ]
}

#[cfg(unix)]
fn identity(fx: &Fixture) -> String {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(&fx.root).unwrap();
    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn identity(_fx: &Fixture) -> String {
    "0:0".to_owned()
}

fn rank(shell: &Server, query: &str) -> Reply {
    shell.post(RANK, &json!({ "query": query }))
}

fn rank_as(shell: &Server, fields: &[String]) -> Reply {
    shell.raw("POST", RANK, fields, br#"{"query":"x"}"#)
}

/// The Library still loads, whatever the forward just did.
fn still_serving(shell: &Server) {
    assert_eq!(shell.get("/api/library").status, 200);
}

fn fixture() -> Fixture {
    let fx = Fixture::new();
    archive(&fx);
    fx
}

#[cfg(unix)]
#[test]
fn keys_come_back_byte_for_byte_from_a_request_built_from_scratch() {
    let fx = fixture();
    let keys = r#"{"keys": ["https://b.test/two", "https://a.test/one"]}"#;
    let mut fields = json_fields(&fx, keys);
    fields.push("Set-Cookie: session=1".to_owned());
    fields.push("Access-Control-Allow-Origin: *".to_owned());
    let service = Service::replying(reply("200 OK", &fields, keys));
    let shell = service.shell(&fx);
    let body = r#"{"query":"night train"}"#;
    let answer = shell.raw(
        "POST",
        "/model/api/rank?from=page",
        &[
            format!("Host: {}", shell.host()),
            format!("Origin: {}", shell.origin()),
            "Content-Type: application/json".to_owned(),
            format!("Content-Length: {}", body.len()),
            "Cookie: theme=dark".to_owned(),
            "User-Agent: Synthetic/1.0".to_owned(),
            "Referer: http://127.0.0.1/".to_owned(),
        ],
        body.as_bytes(),
    );
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.body, keys.as_bytes());
    assert_eq!(answer.header("content-type"), Some("application/json"));
    for (name, _) in &answer.headers {
        assert!(
            !["set-cookie", "x-kmt-archive"].contains(&name.as_str())
                && !name.starts_with("access-control"),
            "relayed {name}"
        );
    }
    let expected = format!(
        "POST /api/rank HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        service.port,
        body.len()
    );
    assert_eq!(service.seen(), vec![expected.into_bytes()]);
    still_serving(&shell);
}

#[cfg(unix)]
#[test]
fn the_services_refusals_pass_through_and_other_statuses_do_not() {
    let fx = fixture();
    let blank = r#"{"error": "type a search"}"#;
    let busy = r#"{"error": "busy"}"#;
    let broken = r#"{"error": "boom"}"#;
    let replies = [
        reply("400 Bad Request", &json_fields(&fx, blank), blank),
        reply("503 Service Unavailable", &json_fields(&fx, busy), busy),
        reply(
            "500 Internal Server Error",
            &json_fields(&fx, broken),
            broken,
        ),
        reply("302 Found", &json_fields(&fx, broken), broken),
    ];
    let turn = Mutex::new(0);
    let service = Service::start(move |_| {
        let mut turn = turn.lock().unwrap();
        *turn += 1;
        Some(replies[*turn - 1].clone())
    });
    let shell = service.shell(&fx);
    let relayed = rank(&shell, "");
    assert_eq!((relayed.status, relayed.text()), (400, blank.to_owned()));
    let relayed = rank(&shell, "x");
    assert_eq!((relayed.status, relayed.text()), (503, busy.to_owned()));
    for _ in 0..2 {
        let failed = rank(&shell, "x");
        assert_eq!(failed.status, 502, "{}", failed.text());
        assert!(!failed.text().contains("boom"));
    }
    still_serving(&shell);
}

#[cfg(unix)]
#[test]
fn a_reply_for_another_archive_or_none_is_refused_before_its_body() {
    let fx = fixture();
    let keys = r#"{"keys": []}"#;
    for archive in [None, Some("1:2".to_owned())] {
        let mut fields = json_fields(&fx, keys);
        fields.pop();
        fields.extend(archive.map(|named| format!("X-KMT-Archive: {named}")));
        // The head arrives, the body never does: only a refusal made on the
        // head alone answers before the deadline turns it into a 504.
        let service = Service::replying(reply("200 OK", &fields, ""));
        let shell = service.shell(&fx);
        let refused = rank(&shell, "x");
        assert_eq!(refused.status, 502, "{}", refused.text());
        assert!(
            refused.text().contains("another archive"),
            "{}",
            refused.text()
        );
        still_serving(&shell);
    }
}

#[cfg(unix)]
#[test]
fn a_declared_length_over_the_limit_is_refused_before_its_body() {
    let fx = fixture();
    let mut fields = json_fields(&fx, "");
    fields[1] = format!("Content-Length: {}", 8 * 1024 * 1024 + 1);
    let service = Service::replying(reply("200 OK", &fields, ""));
    let shell = service.shell(&fx);
    let refused = rank(&shell, "x");
    assert_eq!(refused.status, 502, "{}", refused.text());
    assert!(refused.text().contains("too much"), "{}", refused.text());
    still_serving(&shell);
}

#[cfg(unix)]
#[test]
fn chunked_html_or_unframed_replies_are_refused() {
    let fx = fixture();
    let archive = format!("X-KMT-Archive: {}", identity(&fx));
    let html = "<p>keys</p>";
    let cases = [
        reply(
            "200 OK",
            &[
                "Content-Type: application/json".to_owned(),
                "Transfer-Encoding: chunked".to_owned(),
                archive.clone(),
            ],
            "",
        ),
        reply(
            "200 OK",
            &["Content-Type: application/json".to_owned(), archive.clone()],
            "",
        ),
        reply(
            "200 OK",
            &[
                "Content-Type: text/html".to_owned(),
                format!("Content-Length: {}", html.len()),
                archive.clone(),
            ],
            html,
        ),
        reply(
            "200 OK",
            &[
                "Content-Type: application/json".to_owned(),
                "Content-Length: 2".to_owned(),
                "Content-Length: 2".to_owned(),
                archive.clone(),
            ],
            "{}",
        ),
        b"HTTP/2 200 OK\r\n\r\n".to_vec(),
    ];
    for case in cases {
        let service = Service::replying(case);
        let shell = service.shell(&fx);
        let refused = rank(&shell, "x");
        assert_eq!(refused.status, 502, "{}", refused.text());
        assert!(!refused.text().contains("keys"));
        still_serving(&shell);
    }
}

#[test]
fn a_service_that_is_not_running_is_unavailable() {
    let fx = fixture();
    let port = {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let shell = Server::start_with(&fx, &["--model-port", &port.to_string()]);
    let refused = rank(&shell, "x");
    assert_eq!(refused.status, 503, "{}", refused.text());
    still_serving(&shell);
}

#[test]
fn a_service_that_never_answers_times_out_on_one_deadline() {
    let fx = fixture();
    let service = Service::start(|_| None);
    let shell = service.shell(&fx);
    let start = Instant::now();
    let late = rank(&shell, "x");
    let took = start.elapsed();
    assert_eq!(late.status, 504, "{}", late.text());
    assert!(took >= Duration::from_secs(2), "answered at {took:?}");
    assert!(took < Duration::from_secs(8), "took {took:?}");
    still_serving(&shell);
}

#[test]
fn without_a_model_port_search_is_unavailable() {
    let fx = fixture();
    let shell = Server::start(&fx);
    let off = rank(&shell, "x");
    assert_eq!(off.status, 503, "{}", off.text());
    still_serving(&shell);
}

#[test]
fn refusals_the_shell_makes_never_reach_the_service() {
    let fx = fixture();
    let service = Service::start(|_| None);
    let shell = service.shell(&fx);
    let json = || "Content-Type: application/json".to_owned();
    let length = || "Content-Length: 13".to_owned();
    let host = format!("Host: {}", shell.host());
    for (fields, status) in [
        (
            vec!["Host: attacker.test".to_owned(), json(), length()],
            403,
        ),
        (
            vec![
                host.clone(),
                "Origin: http://attacker.test".to_owned(),
                json(),
                length(),
            ],
            403,
        ),
        (
            vec![
                host.clone(),
                "Content-Type: text/plain".to_owned(),
                length(),
            ],
            415,
        ),
        (vec![host.clone(), length()], 415),
    ] {
        let refused = rank_as(&shell, &fields);
        assert_eq!(refused.status, status, "{fields:?}: {}", refused.text());
    }
    for (method, path) in [
        ("GET", RANK),
        ("POST", "/model/api/search"),
        ("POST", "/model/"),
        ("POST", "/model/api/rank/"),
        ("POST", "/model/../api/library"),
        ("POST", "/model//api/rank"),
    ] {
        let fields = [host.clone(), json(), length()];
        let missing = shell.raw(method, path, &fields, br#"{"query":"x"}"#);
        assert_eq!(missing.status, 404, "{method} {path}: {}", missing.text());
    }
    assert!(service.seen().is_empty(), "the service was contacted");
    still_serving(&shell);
}

#[test]
fn the_model_port_cannot_be_the_shells_own() {
    let fx = fixture();
    let port = {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port().to_string()
    };
    let mut child = fx
        .command()
        .args(["serve", "--port", &port, "--model-port", &port])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("serve kept running on its own port as the model port");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = common::stderr(&output);
    assert!(stderr.contains("--model-port"), "{stderr}");
}

/// No directory identity off Unix, so no reply is ever this archive's.
#[cfg(not(unix))]
#[test]
fn without_an_archive_identity_no_reply_is_taken() {
    let fx = fixture();
    let keys = r#"{"keys": []}"#;
    let service = Service::replying(reply("200 OK", &json_fields(&fx, keys), keys));
    let shell = service.shell(&fx);
    assert_eq!(rank(&shell, "x").status, 502);
    still_serving(&shell);
}
