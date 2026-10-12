//! `serve --model-port` through the real binary and a stand-in model service
//! on loopback: what reaches the service, what comes back to the page, and
//! every refusal on the way, with the shell still serving after each. The
//! stand-in proves its replies with the shared vectors' synthetic key, by its
//! own copy of the wire, so the shell's side is checked against both.

mod common;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Fixture;
use common::serve::{Reply, Server, archive};
use hmac::{Hmac, Mac as _};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const RANK: &str = "/model/api/rank";

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/service_auth.json")).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// HMAC-SHA256 of the framed fields under `key`, lowercase hex.
fn mac(key: &[u8], fields: &[&str]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).unwrap();
    for field in fields {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    hex(&mac.finalize().into_bytes())
}

/// The key `just up` would write, as the shared vectors hold it.
fn key() -> Vec<u8> {
    let text = vectors()["key"].as_str().unwrap().to_owned();
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// The key in a private file beside the archive, written once: a shell
/// reads it as it starts.
fn key_file(fx: &Fixture) -> PathBuf {
    let path = fx.home.path().join("session.key");
    if path.exists() {
        return path;
    }
    std::fs::write(&path, format!("{}\n", hex(&key()))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

/// A head field of a request or reply, by name in any case.
fn field<'a>(message: &'a [u8], name: &str) -> Option<&'a str> {
    let end = message.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&message[..end]).ok()?;
    head.split("\r\n").skip(1).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The request's method and path.
fn request_line(request: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(request);
    let mut words = text.split(' ');
    (
        words.next().unwrap().to_owned(),
        words.next().unwrap().to_owned(),
    )
}

/// `reply` with the proof `key` gives it as the answer to `request`: over
/// the request's nonce, method and path, and the status, type, archive and
/// body the reply declares.
fn prove_with(
    key: &[u8],
    request: &[u8],
    status: &str,
    fields: &[String],
    body: impl AsRef<[u8]>,
) -> Vec<u8> {
    let body = body.as_ref();
    let unproven = reply(status, fields, body);
    let (method, path) = request_line(request);
    let proof = mac(
        key,
        &[
            "kmt-s2-response-v1",
            field(request, "x-kmt-nonce").unwrap_or(""),
            &method,
            &path,
            &status[..3],
            field(&unproven, "content-type").unwrap_or(""),
            field(&unproven, "x-kmt-archive").unwrap_or(""),
            &body.len().to_string(),
            &hex(&Sha256::digest(body)),
        ],
    );
    let mut fields = fields.to_vec();
    fields.push(format!("X-KMT-Response-MAC: {proof}"));
    reply(status, &fields, body)
}

/// The service's own answer to `request`.
fn proven(request: &[u8], status: &str, fields: &[String], body: impl AsRef<[u8]>) -> Vec<u8> {
    prove_with(&key(), request, status, fields, body)
}

/// Whether the request carries this key's proof of exactly what it sent.
fn request_proven(request: &[u8]) -> bool {
    let (method, path) = request_line(request);
    let at = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let body = &request[at..];
    let expected = mac(
        &key(),
        &[
            "kmt-s2-request-v1",
            field(request, "x-kmt-nonce").unwrap_or(""),
            field(request, "x-kmt-time").unwrap_or(""),
            &method,
            &path,
            field(request, "content-type").unwrap_or(""),
            &body.len().to_string(),
            &hex(&Sha256::digest(body)),
        ],
    );
    field(request, "x-kmt-request-mac") == Some(expected.as_str())
}

#[test]
fn the_stand_ins_wire_matches_the_shared_vectors() {
    let vectors = vectors();
    let case = &vectors["responses"][0];
    let text = |name: &str| case[name].as_str().unwrap().to_owned();
    let fields = [
        format!("Content-Type: {}", text("content_type")),
        format!("X-KMT-Archive: {}", text("archive")),
    ];
    let request = format!(
        "{} {} HTTP/1.1\r\nX-KMT-Nonce: {}\r\n\r\n",
        text("method"),
        text("path"),
        text("nonce")
    );
    let answer = proven(request.as_bytes(), "200 OK", &fields, text("body"));
    assert_eq!(
        field(&answer, "x-kmt-response-mac"),
        Some(text("mac").as_str())
    );
}

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

    /// Answers every request with the same reply, proven for that request.
    fn proving(status: &'static str, fields: Vec<String>, body: String) -> Self {
        Self::start(move |request| Some(proven(request, status, &fields, &body)))
    }

    fn seen(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }

    fn shell(&self, fx: &Fixture) -> Server {
        shell_on(fx, self.port)
    }
}

/// The shell forwarding to `port`, with the launch's key.
fn shell_on(fx: &Fixture, port: u16) -> Server {
    let key = key_file(fx);
    Server::start_with(
        fx,
        &[
            "--model-port",
            &port.to_string(),
            "--model-secret-file",
            key.to_str().unwrap(),
        ],
    )
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
fn reply(status: &str, fields: &[String], body: impl AsRef<[u8]>) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for field in fields {
        head.push_str(field);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    [head.as_bytes(), body.as_ref()].concat()
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
    let service = Service::proving("200 OK", fields, keys.to_owned());
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
            !["set-cookie", "x-kmt-archive", "x-kmt-response-mac"].contains(&name.as_str())
                && !name.starts_with("access-control"),
            "relayed {name}"
        );
    }
    let seen = service.seen();
    assert_eq!(seen.len(), 1);
    assert!(
        request_proven(&seen[0]),
        "the request carries the key's proof"
    );
    let nonce = field(&seen[0], "x-kmt-nonce").unwrap();
    assert!(nonce.len() == 64 && nonce.bytes().all(|b| b.is_ascii_hexdigit()));
    let time: u128 = field(&seen[0], "x-kmt-time").unwrap().parse().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    assert!(now.abs_diff(time) < 10_000, "the request's time is now");
    let expected = format!(
        "POST /api/rank HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nX-KMT-Nonce: {nonce}\r\nX-KMT-Time: {time}\r\n\
         X-KMT-Request-MAC: {}\r\nConnection: close\r\n\r\n{body}",
        service.port,
        body.len(),
        field(&seen[0], "x-kmt-request-mac").unwrap(),
    );
    assert_eq!(String::from_utf8_lossy(&seen[0]), expected);
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
        ("400 Bad Request", json_fields(&fx, blank), blank),
        ("503 Service Unavailable", json_fields(&fx, busy), busy),
        ("403 Forbidden", json_fields(&fx, busy), busy),
        ("404 Not Found", json_fields(&fx, busy), busy),
        ("409 Conflict", json_fields(&fx, busy), busy),
        ("415 Unsupported Media Type", json_fields(&fx, busy), busy),
        (
            "500 Internal Server Error",
            json_fields(&fx, broken),
            broken,
        ),
        ("302 Found", json_fields(&fx, broken), broken),
        ("421 Misdirected Request", json_fields(&fx, broken), broken),
    ];
    let turn = Mutex::new(0);
    let service = Service::start(move |request| {
        let mut turn = turn.lock().unwrap();
        *turn += 1;
        let (status, fields, body) = &replies[*turn - 1];
        Some(proven(request, status, fields, body))
    });
    let shell = service.shell(&fx);
    let relayed = rank(&shell, "");
    assert_eq!((relayed.status, relayed.text()), (400, blank.to_owned()));
    for status in [503, 403, 404, 409, 415] {
        let relayed = rank(&shell, "x");
        assert_eq!((relayed.status, relayed.text()), (status, busy.to_owned()));
    }
    for _ in 0..3 {
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
    let refusing = Refusing::new();
    let shell = shell_on(&fx, refusing.port);
    let refused = rank(&shell, "x");
    assert_eq!(refused.status, 503, "{}", refused.text());
    assert!(
        refused.text().contains("not answering"),
        "{}",
        refused.text()
    );
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
        ("POST", "/model/api/nothing"),
        ("POST", "/model/"),
        ("POST", "/model/index.html"),
        ("POST", "/model/api/rank/"),
        ("POST", "/model/../api/library"),
        ("POST", "/model//api/rank"),
        ("GET", "/model/server.py"),
        ("GET", "/model/static/app.js"),
        ("GET", "/model/..%2Fserver.py"),
        ("GET", "/model/img/not-a-hash"),
        ("GET", "/model/api/sessions/1a"),
        ("GET", "/modelx/api/library"),
    ] {
        let fields = [host.clone(), json(), length()];
        let missing = shell.raw(method, path, &fields, br#"{"query":"x"}"#);
        assert_eq!(missing.status, 404, "{method} {path}: {}", missing.text());
    }
    let fields = [host.clone(), length()];
    let bodied = shell.raw("GET", "/model/api/library", &fields, br#"{"query":"x"}"#);
    assert_eq!(bodied.status, 400, "{}", bodied.text());
    assert!(service.seen().is_empty(), "the service was contacted");
    still_serving(&shell);
}

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// A synthetic JPEG's first and last bytes: not text, so only bytes carry it.
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\xff\xd9";
/// The service's page, as small as can show what the shell does with it.
const PAGE: &str = "<!doctype html><title>Tag</title><header><!-- nav:start --><!-- nav:end --></header>\
                    <script src=\"core.js\" defer></script>";

/// One service path's answer: status line, type and body.
type Answer = (String, &'static str, &'static str, Vec<u8>);

/// A stand-in answering each service path from `table`, proven.
fn table_service(fx: &Fixture, table: Vec<Answer>) -> Service {
    let archive = format!("X-KMT-Archive: {}", identity(fx));
    Service::start(move |request| {
        let (_, path) = request_line(request);
        let (_, status, kind, body) = table.iter().find(|(p, ..)| *p == path)?;
        let fields = vec![
            format!("Content-Type: {kind}"),
            format!("Content-Length: {}", body.len()),
            archive.clone(),
        ];
        Some(proven(request, status, &fields, body))
    })
}

#[cfg(unix)]
#[test]
fn every_family_forwards_its_method_and_path_and_comes_back_typed() {
    let fx = fixture();
    let module = r#"export * from "./lit-html.js";"#;
    let json = br#"{"pages": 3}"#.to_vec();
    let html = "text/html; charset=utf-8";
    let ok = |path: &str, kind: &'static str, body: &[u8]| -> Answer {
        (path.to_owned(), "200 OK", kind, body.to_vec())
    };
    let image = format!("/img/{HASH}");
    let table = vec![
        ok("/", html, PAGE.as_bytes()),
        ok("/index.html", html, PAGE.as_bytes()),
        ok("/app.css", "text/css; charset=utf-8", b"body{}"),
        ok("/lit.js", "application/javascript", module.as_bytes()),
        ok(&image, "image/jpeg", JPEG),
        ok("/api/library", "application/json; charset=utf-8", &json),
        ok("/api/add/12", "application/json", &json),
        ok("/api/search", "application/json", &json),
        ok("/api/sessions/3/pages/1", "application/json", &json),
    ];
    let service = table_service(&fx, table.clone());
    let shell = service.shell(&fx);
    let css = "text/css; charset=utf-8";
    let script = "text/javascript; charset=utf-8";
    let api = "application/json";
    for (method, path, upstream, kind) in [
        ("GET", "/model/".to_owned(), "/", html),
        ("GET", "/model/index.html".to_owned(), "/index.html", html),
        ("GET", "/model/app.css?v=1".to_owned(), "/app.css", css),
        ("GET", "/model/lit.js".to_owned(), "/lit.js", script),
        ("GET", format!("/model{image}"), &image, "image/jpeg"),
        ("GET", "/model/api/library".to_owned(), "/api/library", api),
        ("GET", "/model/api/add/12".to_owned(), "/api/add/12", api),
        ("POST", "/model/api/search".to_owned(), "/api/search", api),
        (
            "POST",
            "/model/api/sessions/3/pages/1".to_owned(),
            "/api/sessions/3/pages/1",
            api,
        ),
    ] {
        let answer = if method == "POST" {
            shell.post(&path, &json!({ "query": "x" }))
        } else {
            shell.get(&path)
        };
        assert_eq!(answer.status, 200, "{path}: {}", answer.text());
        assert_eq!(answer.header("content-type"), Some(kind), "{path}");
        assert_eq!(answer.header("referrer-policy"), Some("no-referrer"));
        let policy = answer.header("content-security-policy");
        if kind == html {
            let policy = policy.unwrap_or_default();
            assert!(
                policy.contains("style-src 'self' 'unsafe-inline';"),
                "{policy}"
            );
            assert!(policy.contains("script-src 'self';"), "{policy}");
            assert!(policy.ends_with("frame-ancestors 'none'"), "{policy}");
            let navigated = PAGE.replace(
                "<!-- nav:start --><!-- nav:end -->",
                &format!("<!-- nav:start -->{SERVED_NAV}<!-- nav:end -->"),
            );
            assert_eq!(
                answer.text(),
                navigated,
                "{path}: the nav, and nothing else"
            );
        } else {
            assert_eq!(policy, None, "{path}");
            let (.., body) = table.iter().find(|(p, ..)| p == upstream).unwrap();
            assert_eq!(&answer.body, body, "{path}");
        }
        assert!(
            answer
                .headers
                .iter()
                .all(|(name, _)| !name.starts_with("x-kmt"))
        );
        let seen = service.seen();
        let request = seen.last().unwrap();
        assert_eq!(
            request_line(request),
            (method.to_owned(), upstream.to_owned())
        );
        assert!(request_proven(request), "{path}");
        assert_eq!(field(request, "content-type").is_some(), method == "POST");
        assert_eq!(field(request, "origin"), None);
    }
}

#[cfg(unix)]
#[test]
fn a_success_of_another_type_or_a_refusal_that_is_not_json_is_refused() {
    let fx = fixture();
    let missing = br#"{"error": "no image"}"#.to_vec();
    let (a, b) = ("a".repeat(64), "b".repeat(64));
    let page = PAGE.as_bytes().to_vec();
    let table: Vec<Answer> = vec![
        (
            "/api/library".to_owned(),
            "200 OK",
            "text/html",
            page.clone(),
        ),
        ("/".to_owned(), "200 OK", "application/json", b"{}".to_vec()),
        (
            "/app.css".to_owned(),
            "200 OK",
            "text/javascript",
            b"x".to_vec(),
        ),
        ("/lit.js".to_owned(), "200 OK", "text/plain", b"x".to_vec()),
        (format!("/img/{HASH}"), "200 OK", "text/html", page.clone()),
        (format!("/img/{a}"), "404 Not Found", "text/html", page),
        (
            format!("/img/{b}"),
            "404 Not Found",
            "application/json",
            missing.clone(),
        ),
    ];
    let service = table_service(&fx, table);
    let shell = service.shell(&fx);
    for (path, status) in [
        ("/model/api/library".to_owned(), 502),
        ("/model/".to_owned(), 502),
        ("/model/app.css".to_owned(), 502),
        ("/model/lit.js".to_owned(), 502),
        (format!("/model/img/{HASH}"), 502),
        (format!("/model/img/{a}"), 502),
        (format!("/model/img/{b}"), 404),
    ] {
        let answer = shell.get(&path);
        assert_eq!(answer.status, status, "{path}: {}", answer.text());
        assert!(!answer.text().contains("<title>Tag"), "{path}");
    }
    assert_eq!(shell.get(&format!("/model/img/{b}")).body, missing);
    still_serving(&shell);
}

#[test]
fn the_bare_prefix_redirects_below_it() {
    let fx = fixture();
    let service = Service::start(|_| None);
    let shell = service.shell(&fx);
    let moved = shell.get("/model");
    assert_eq!(moved.status, 308);
    assert_eq!(moved.header("location"), Some("/model/"));
    let fields = [
        format!("Host: {}", shell.host()),
        "Content-Length: 0".to_owned(),
    ];
    assert_eq!(shell.raw("POST", "/model", &fields, b"").status, 404);
    assert!(service.seen().is_empty(), "the service was contacted");
}

/// Send answers only once its knowmoretabs runs are done: well past the
/// deadline a search waits.
#[cfg(unix)]
#[test]
fn send_waits_past_the_rank_deadline_for_its_own() {
    let fx = fixture();
    let sent = r#"{"ok": true}"#;
    let fields = json_fields(&fx, sent);
    let service = Service::start(move |request| {
        std::thread::sleep(Duration::from_secs(3));
        Some(proven(request, "200 OK", &fields, sent))
    });
    let shell = service.shell(&fx);
    let start = Instant::now();
    let answer = shell.post("/model/api/send", &json!({ "preview": 1 }));
    assert_eq!((answer.status, answer.text()), (200, sent.to_owned()));
    assert!(start.elapsed() >= Duration::from_secs(3));
}

/// The shell's nav as it serves it, no view marked.
const SERVED_NAV: &str = r#"<nav class="views" aria-label="Views"><a href="/#pages">Pages</a><a href="/model/#tag" id="nav-tag">Tag</a><a href="/model/#add" id="nav-add">Add link</a><a href="/#snapshots">Snapshots</a></nav>"#;

/// A loopback port nothing listens on, held so no other test is given it.
/// Merely binding without listening times out on macOS instead of refusing,
/// so an outgoing connection is kept alive: its local port has no listener,
/// and ephemeral binds cannot allocate it to another test. Windows retries
/// a refused loopback connect for about two seconds before it reports the
/// refusal, so the check waits longer than that.
struct Refusing {
    port: u16,
    _held: (TcpStream, TcpStream),
}

impl Refusing {
    fn new() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let reserved = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (peer, _) = listener.accept().unwrap();
        let address = reserved.local_addr().unwrap();
        assert_eq!(
            TcpStream::connect_timeout(&address, Duration::from_secs(10))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::ConnectionRefused,
            "the reserved model port must refuse connections"
        );
        Self {
            port: address.port(),
            _held: (reserved, peer),
        }
    }
}

/// Whatever keeps the service's page from the shell, Tag and Add link get
/// the shell's own page at the same address, with the nav and a reason,
/// under the Library's policy; the API answers JSON and an unknown path is
/// still not found.
#[cfg(unix)]
#[test]
fn the_page_is_the_shells_own_when_the_service_cannot_give_it() {
    let fx = fixture();
    let refusing = Refusing::new();
    let unproven = {
        let fields = vec![
            "Content-Type: text/html; charset=utf-8".to_owned(),
            format!("Content-Length: {}", PAGE.len()),
            format!("X-KMT-Archive: {}", identity(&fx)),
        ];
        Service::start(move |_| Some(reply("200 OK", &fields, PAGE)))
    };
    let html = "text/html; charset=utf-8";
    let answering = |kind: &str, body: &str| {
        let fields = vec![
            format!("Content-Type: {kind}"),
            format!("Content-Length: {}", body.len()),
            format!("X-KMT-Archive: {}", identity(&fx)),
        ];
        Service::proving("200 OK", fields, body.to_owned())
    };
    let bare = answering(html, "<!doctype html><title>Tag</title>");
    let typed = answering("text/plain", PAGE);
    let unkeyed = Server::start_with(&fx, &["--model-port", &bare.port.to_string()]);
    let shells = [
        ("no key", unkeyed, 503, "No model service is set"),
        (
            "not running",
            shell_on(&fx, refusing.port),
            503,
            "The model service is not answering",
        ),
        (
            "unproven",
            unproven.shell(&fx),
            502,
            "The model service did not prove its answer",
        ),
        (
            "no nav",
            bare.shell(&fx),
            502,
            "a form this server does not take",
        ),
        (
            "not a page",
            typed.shell(&fx),
            502,
            "a form this server does not take",
        ),
    ];
    for (case, shell, status, said) in shells {
        for path in ["/model/", "/model/index.html", "/model/?x=1"] {
            let answer = shell.get(path);
            assert_eq!(answer.status, status, "{case} {path}: {}", answer.text());
            assert_eq!(answer.header("content-type"), Some(html), "{case}");
            let policy = answer.header("content-security-policy").unwrap_or_default();
            assert!(policy.contains("style-src 'self';"), "{case}: {policy}");
            let text = answer.text();
            assert!(text.contains(SERVED_NAV), "{case}: {text}");
            assert!(text.contains(said), "{case}: {text}");
            assert!(text.contains(r#"<script src="/unavailable.js"></script>"#));
            assert!(
                !text.contains("<title>Tag</title>"),
                "{case}: service bytes"
            );
        }
        let api = shell.get("/model/api/library");
        assert_eq!(api.status, status, "{case}");
        assert_eq!(api.header("content-type"), Some("application/json"));
        assert_eq!(shell.get("/model/nothing").status, 404, "{case}");
        still_serving(&shell);
    }
}

/// What a squatter on the port, a replay, or bytes changed after the proof
/// would send: the shell takes none of them.
#[cfg(unix)]
#[test]
fn a_reply_without_this_requests_proof_is_refused() {
    type Answer = Box<dyn Fn(&[u8]) -> Vec<u8> + Send>;
    let fx = fixture();
    let keys = r#"{"keys": ["https://a.test/one"]}"#;
    let fields = json_fields(&fx, keys);
    let replace = |bytes: Vec<u8>, from: &str, to: &str| {
        String::from_utf8(bytes)
            .unwrap()
            .replacen(from, to, 1)
            .into_bytes()
    };
    let cases: Vec<(&str, Answer)> = vec![
        ("no proof", {
            let fields = fields.clone();
            Box::new(move |_| reply("200 OK", &fields, keys))
        }),
        ("a squatter without the key", {
            let fields = fields.clone();
            Box::new(move |request| prove_with(&[7; 32], request, "200 OK", &fields, keys))
        }),
        ("another request's proof", {
            let fields = fields.clone();
            Box::new(move |_| {
                let earlier = format!(
                    "POST /api/rank HTTP/1.1\r\nX-KMT-Nonce: {}\r\n\r\n",
                    "0".repeat(64)
                );
                proven(earlier.as_bytes(), "200 OK", &fields, keys)
            })
        }),
        ("a body changed after the proof", {
            let fields = fields.clone();
            Box::new(move |request| {
                replace(
                    proven(request, "200 OK", &fields, keys),
                    "a.test/one",
                    "b.test/two",
                )
            })
        }),
        ("a status changed after the proof", {
            let fields = fields.clone();
            Box::new(move |request| {
                replace(
                    proven(request, "400 Bad Request", &fields, keys),
                    "400 Bad Request",
                    "200 OK",
                )
            })
        }),
        ("a type changed after the proof", {
            let fields = fields.clone();
            Box::new(move |request| {
                replace(
                    proven(request, "200 OK", &fields, keys),
                    "application/json; charset=utf-8",
                    "application/json",
                )
            })
        }),
    ];
    for (case, answer) in cases {
        let service = Service::start(move |request| Some(answer(request)));
        let shell = service.shell(&fx);
        let refused = rank(&shell, "x");
        assert_eq!(refused.status, 502, "{case}: {}", refused.text());
        assert!(
            refused.text().contains("did not prove"),
            "{case}: {}",
            refused.text()
        );
        assert!(!refused.text().contains("test/"), "{case}");
        still_serving(&shell);
    }
}

/// Forwarding is off, and said once without the file's bytes, unless the
/// key file is a private, well formed key; the Library serves either way.
#[cfg(unix)]
#[test]
fn a_model_port_without_a_usable_key_forwards_nothing() {
    use std::os::unix::fs::PermissionsExt as _;
    let fx = fixture();
    let service = Service::start(|_| None);
    let good = key_file(&fx);
    let write = |name: &str, text: &str, mode: u32| {
        let path = fx.home.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    };
    let secret = std::fs::read_to_string(&good).unwrap();
    let link = fx.home.path().join("link.key");
    std::os::unix::fs::symlink(&good, &link).unwrap();
    let cases = [
        (None, "no --model-secret-file"),
        (Some(fx.home.path().join("missing.key")), "cannot be read"),
        (
            Some(write("shared.key", &secret, 0o644)),
            "readable by others",
        ),
        (Some(link), "a link"),
        (
            Some(write("short.key", "abc\n", 0o600)),
            "does not hold a key",
        ),
    ];
    for (key, said) in cases {
        let stderr = fx.home.path().join("stderr");
        let mut command = fx.command();
        command.args([
            "serve",
            "--port",
            "0",
            "--model-port",
            &service.port.to_string(),
        ]);
        if let Some(key) = &key {
            command.arg("--model-secret-file").arg(key);
        }
        let child = command
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let shell = Server::from_child(child);
        let off = rank(&shell, "x");
        assert_eq!(off.status, 503, "{said}: {}", off.text());
        still_serving(&shell);
        drop(shell);
        let warned = std::fs::read_to_string(&stderr).unwrap();
        assert_eq!(warned.matches(said).count(), 1, "{warned}");
        assert!(!warned.contains(secret.trim()), "the key was printed");
    }
    assert!(service.seen().is_empty(), "the service was contacted");
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
    let service = Service::proving("200 OK", json_fields(&fx, keys), keys.to_owned());
    let shell = service.shell(&fx);
    assert_eq!(rank(&shell, "x").status, 502);
    still_serving(&shell);
}
