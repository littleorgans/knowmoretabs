//! `knowmoretabs serve` through the real binary and a raw TCP client: a
//! synthetic archive, the server on an ephemeral port, and a reply parser
//! that sees exactly what went over the wire.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::process::{Child, Stdio};
use std::time::Duration;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{Fixture, write_snapshot};

pub const A: &str = "https://a.test/one";
pub const B: &str = "https://b.test/two";
pub const HIDDEN: &str = "https://hidden.test/page";

pub fn archive(fx: &Fixture) {
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

/// The state file, `library.json`, as written.
pub fn state(fx: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fx.root.join("library.json")).unwrap()).unwrap()
}

/// The served library's entry for `url`.
pub fn page<'a>(library: &'a Value, url: &str) -> &'a Value {
    library["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["url"] == url)
        .unwrap_or_else(|| panic!("{url} not in pages"))
}

pub fn without_generated_at(mut library: Value) -> Value {
    library.as_object_mut().unwrap().remove("generated_at");
    library
}

/// The name a page's text and image are kept under, and its image served
/// by: the hex SHA-256 of its URL.
pub fn kept_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(url.as_bytes()))
}

/// Appends `line` to the log `pages/<log>`.
pub fn append_line(fx: &Fixture, log: &str, line: &Value) {
    let pages = fx.root.join("pages");
    fs::create_dir_all(&pages).unwrap();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pages.join(log))
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// A content line for `url` and, given a body, its file as `content` writes
/// it: front matter, then the markdown.
pub fn kept_text(fx: &Fixture, url: &str, line: &Value, body: Option<&str>) {
    append_line(fx, "content.jsonl", line);
    if let Some(body) = body {
        let dir = fx.root.join("pages").join("content");
        fs::create_dir_all(&dir).unwrap();
        let front = format!(
            "---\nschema_version: 1\nurl: {}\nfetched_at: \"2026-10-07T09:00:00Z\"\n---\n",
            json!(url)
        );
        fs::write(dir.join(format!("{}.md", kept_name(url))), front + body).unwrap();
    }
}

/// Appends an image line for `url` and, given bytes, writes its file.
pub fn kept_image(fx: &Fixture, url: &str, status: &str, jpeg: Option<&[u8]>) {
    let images = fx.root.join("pages").join("images");
    fs::create_dir_all(&images).unwrap();
    append_line(
        fx,
        "images.jsonl",
        &json!({"schema_version": 1, "url": url, "attempted_at": "2026-10-07T09:00:00Z", "status": status}),
    );
    if let Some(jpeg) = jpeg {
        fs::write(images.join(format!("{}.jpg", kept_name(url))), jpeg).unwrap();
    }
}

/// The binary serving on an ephemeral port, killed on drop.
pub struct Server {
    pub child: Child,
    pub port: u16,
    pub url: String,
}

impl Server {
    pub fn start(fx: &Fixture) -> Self {
        Self::start_with(fx, &[])
    }

    /// With more `serve` arguments after the ephemeral port.
    pub fn start_with(fx: &Fixture, args: &[&str]) -> Self {
        Self::from_child(
            fx.command()
                .args(["serve", "--port", "0"])
                .args(args)
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
    pub fn from_child(mut child: Child) -> Self {
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

    pub fn origin(&self) -> String {
        self.url.trim_end_matches('/').to_owned()
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// A raw request to the server; `headers` are sent verbatim after the
    /// request line, so a test controls Host, Origin and framing exactly.
    pub fn raw(&self, method: &str, path: &str, headers: &[String], body: &[u8]) -> Reply {
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

    pub fn get(&self, path: &str) -> Reply {
        self.raw("GET", path, &[format!("Host: {}", self.host())], b"")
    }

    /// `GET /api/content` for `url`, percent encoded.
    pub fn content(&self, url: &str) -> Reply {
        self.get(&format!(
            "/api/content?url={}",
            utf8_percent_encode(url, NON_ALPHANUMERIC)
        ))
    }

    /// The content route's reply for `url`, which must be a JSON 200.
    pub fn served(&self, url: &str) -> Value {
        let reply = self.content(url);
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(reply.header("content-type"), Some("application/json"));
        reply.json()
    }

    pub fn post(&self, path: &str, body: &Value) -> Reply {
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

    pub fn library(&self) -> Value {
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

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn parse(bytes: &[u8]) -> Self {
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

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }
}
