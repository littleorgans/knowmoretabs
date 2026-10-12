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

/// The key in a private file beside the archive.
fn key_file(fx: &Fixture) -> PathBuf {
    let path = fx.home.path().join("session.key");
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
    let text = std::str::from_utf8(message).ok()?;
    let head = &text[..text.find("\r\n\r\n")?];
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
fn prove_with(key: &[u8], request: &[u8], status: &str, fields: &[String], body: &str) -> Vec<u8> {
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
            &hex(&Sha256::digest(body.as_bytes())),
        ],
    );
    let mut fields = fields.to_vec();
    fields.push(format!("X-KMT-Response-MAC: {proof}"));
    reply(status, &fields, body)
}

/// The service's own answer to `request`.
fn proven(request: &[u8], status: &str, fields: &[String], body: &str) -> Vec<u8> {
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
    let answer = proven(request.as_bytes(), "200 OK", &fields, &text("body"));
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
        (
            "500 Internal Server Error",
            json_fields(&fx, broken),
            broken,
        ),
        ("302 Found", json_fields(&fx, broken), broken),
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
    // Keep an outgoing connection alive: its local port has no listener,
    // and ephemeral binds cannot allocate it to another test. Merely
    // binding without listening times out on macOS instead of refusing.
    // Windows retries a refused loopback connect for about two seconds
    // before it reports the refusal, so the check waits longer than that.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let reserved = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let address = reserved.local_addr().unwrap();
    let port = address.port();
    assert_eq!(
        TcpStream::connect_timeout(&address, Duration::from_secs(10))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::ConnectionRefused,
        "the reserved model port must refuse connections"
    );
    let shell = shell_on(&fx, port);
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
