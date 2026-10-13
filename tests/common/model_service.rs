//! A stand-in for the tagger lab's model service on loopback, and its
//! copy of the wire: it proves its replies with the shared vectors'
//! synthetic key and records every request, so the shell's side of the
//! exchange is checked against both. Shared by `serve --model-port` and
//! `search --model-port` tests.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmac::{Hmac, Mac as _};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::Fixture;

pub fn vectors() -> Value {
    serde_json::from_str(include_str!("../fixtures/service_auth.json")).unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// HMAC-SHA256 of the framed fields under `key`, lowercase hex.
pub fn mac(key: &[u8], fields: &[&str]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).unwrap();
    for field in fields {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field.as_bytes());
    }
    hex(&mac.finalize().into_bytes())
}

/// The key `just up` would write, as the shared vectors hold it.
pub fn key() -> Vec<u8> {
    let text = vectors()["key"].as_str().unwrap().to_owned();
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// The key in a private file beside the archive, written once: a shell
/// reads it as it starts.
pub fn key_file(fx: &Fixture) -> PathBuf {
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
pub fn field<'a>(message: &'a [u8], name: &str) -> Option<&'a str> {
    let end = message.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&message[..end]).ok()?;
    head.split("\r\n").skip(1).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The request's method and path.
pub fn request_line(request: &[u8]) -> (String, String) {
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
pub fn prove_with(
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
pub fn proven(request: &[u8], status: &str, fields: &[String], body: impl AsRef<[u8]>) -> Vec<u8> {
    prove_with(&key(), request, status, fields, body)
}

/// Whether the request carries this key's proof of exactly what it sent.
pub fn request_proven(request: &[u8]) -> bool {
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

/// A stand-in model service: records each request's bytes, writes what
/// `answer` returns for it (nothing for `None`), and holds every connection
/// open, so only a declared length can end a reply.
pub struct Service {
    pub port: u16,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Service {
    pub fn start(answer: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + 'static) -> Self {
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
    pub fn replying(reply: Vec<u8>) -> Self {
        Self::start(move |_| Some(reply.clone()))
    }

    /// Answers every request with the same reply, proven for that request.
    pub fn proving(status: &'static str, fields: Vec<String>, body: String) -> Self {
        Self::start(move |request| Some(proven(request, status, &fields, &body)))
    }

    pub fn seen(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }
}

/// One request: the head, then as many body bytes as it declares.
pub fn request(stream: &mut TcpStream) -> Vec<u8> {
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
pub fn reply(status: &str, fields: &[String], body: impl AsRef<[u8]>) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for field in fields {
        head.push_str(field);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    [head.as_bytes(), body.as_ref()].concat()
}

/// The fields of a JSON reply over this archive.
pub fn json_fields(fx: &Fixture, body: &str) -> Vec<String> {
    vec![
        "Content-Type: application/json; charset=utf-8".to_owned(),
        format!("Content-Length: {}", body.len()),
        format!("X-KMT-Archive: {}", identity(fx)),
    ]
}

#[cfg(unix)]
pub fn identity(fx: &Fixture) -> String {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(&fx.root).unwrap();
    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
pub fn identity(_fx: &Fixture) -> String {
    "0:0".to_owned()
}
