//! `knowmoretabs serve`: the library over HTTP/1.1 on 127.0.0.1, written
//! directly on `std::net`.
//!
//! slice: triage
//! why: Any web page the user has open can reach a localhost server, and
//!      this one discloses every URL they ever had open and mutates their
//!      library. Owning the HTTP layer keeps the three defences (loopback
//!      bind, Host check, Origin check) and every input bound in one file a
//!      reviewer can read top to bottom, with no dormant dependency between
//!      them and the socket. Six routes, one client, no keep-alive: each
//!      connection carries one request and is closed.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, Shape};
use crate::triage::{self, Action};
use crate::{assets, export};

/// Request line plus headers, including the final CRLF CRLF. A browser's own
/// headers fit in a few hundred bytes; the margin is for localhost cookies.
const HEAD_LIMIT: usize = 16 * 1024;
/// A bulk forget of every page in a large archive is well under this.
const BODY_LIMIT: usize = 4 * 1024 * 1024;
/// Browsers open connections they never write to (preconnects); a thread
/// waiting on one must give up rather than hold a socket forever.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long, and how much, to keep reading after the response so the close
/// is orderly (see `Server::finish`).
const LINGER: Duration = Duration::from_secs(2);
const LINGER_LIMIT: usize = 1024 * 1024;

pub struct Options {
    pub root: PathBuf,
    pub port: u16,
    pub open: bool,
    pub json: bool,
    pub log: Log,
}

pub fn run(options: &Options) -> Result<(), Error> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, options.port)).map_err(|source| Error::Bind {
            port: options.port,
            source,
        })?;
    let port = listener
        .local_addr()
        .map_err(|source| Error::Bind {
            port: options.port,
            source,
        })?
        .port();
    // A damaged state file is refused here, at the terminal, rather than as
    // a 500 the page can only describe as "HTTP 500".
    library::State::read(&options.root)?;
    let url = format!("http://127.0.0.1:{port}/");
    if options.json {
        println!(
            "{}",
            serde_json::json!({ "serving": { "url": url, "local_only": true } })
        );
    } else if !options.log.quiet {
        println!("Your library is at {url}");
        println!("Local only: it answers this machine and nothing else. Press Ctrl-C to stop.");
    }
    if options.open
        && let Err(err) = open_browser(&url)
    {
        options.log.warn(&format!(
            "could not open a browser ({err}); visit {url} yourself"
        ));
    }
    let server = Arc::new(Server {
        root: options.root.clone(),
        port,
        log: options.log,
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let handler = Arc::clone(&server);
                let spawned = std::thread::Builder::new()
                    .name("connection".to_owned())
                    .spawn(move || handler.serve(stream));
                if let Err(err) = spawned {
                    server.log.warn(&format!("dropped a connection: {err}"));
                }
            }
            Err(err) => server.log.note(&format!("accept failed: {err}")),
        }
    }
    Ok(())
}

fn open_browser(url: &str) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

struct Server {
    root: PathBuf,
    port: u16,
    log: Log,
}

impl Server {
    fn serve(&self, mut stream: TcpStream) {
        let deadline = Instant::now() + IO_TIMEOUT;
        let (head, rest) = match read_head(&mut stream, deadline) {
            Ok(Some(head)) => head,
            // Opened and closed without a request: a preconnect, not an error.
            Ok(None) => return,
            Err(response) => {
                self.finish(&mut stream, "<unreadable>", &response);
                return;
            }
        };
        let (line, response) = match Head::parse(&head) {
            Err(response) => ("<malformed>".to_owned(), response),
            Ok(head) => {
                let line = format!("{} {}", head.method, head.target);
                // Judge Host and Origin before reading any further body bytes;
                // `rest` may contain bytes prefetched with the head.
                let response = match self.check_headers(&head) {
                    Err(response) => response,
                    Ok(()) => match read_body(&mut stream, &head, rest, deadline) {
                        Err(response) => response,
                        Ok(body) => self.route(&head, &body),
                    },
                };
                (line, response)
            }
        };
        self.finish(&mut stream, &line, &response);
    }

    /// Writes the response, then drains what the client is still sending
    /// before closing. A rejection is sent before the body is read, and
    /// closing a socket with unread input makes the kernel send a reset that
    /// can discard the response before the client has read it.
    fn finish(&self, stream: &mut TcpStream, line: &str, response: &Response) {
        self.log.note(&format!("{} {line}", response.status.code()));
        let _ = response.write_to(&mut DeadlineWriter {
            stream,
            deadline: Instant::now() + IO_TIMEOUT,
        });
        let _ = stream.shutdown(Shutdown::Write);
        let deadline = Instant::now() + LINGER;
        let mut sink = [0u8; 4096];
        let mut drained = 0;
        while drained < LINGER_LIMIT {
            let size = sink.len().min(LINGER_LIMIT - drained);
            match read_before(stream, &mut sink[..size], deadline) {
                Ok(0) | Err(_) => break,
                Ok(n) => drained += n,
            }
        }
    }

    /// Rules 2 and 3: the request must be addressed to this server by its
    /// own name, and if it declares where it came from, that must be here.
    fn check_headers(&self, head: &Head) -> Result<(), Response> {
        if !head
            .header("host")
            .is_some_and(|host| host_allowed(host, self.port))
        {
            return Err(Response::error(
                Status::Forbidden,
                "the Host header does not name this server",
            ));
        }
        if let Some(origin) = head.header("origin")
            && !origin_allowed(origin, self.port)
        {
            return Err(Response::error(
                Status::Forbidden,
                "cross-origin requests are not accepted",
            ));
        }
        Ok(())
    }

    fn route(&self, head: &Head, body: &[u8]) -> Response {
        let path = head.target.split('?').next().unwrap_or("");
        match (head.method.as_str(), path) {
            ("GET", "/" | "/index.html") => self.asset("index.html"),
            ("GET", "/app.css" | "/app.js") => self.asset(&path[1..]),
            ("GET", "/api/library") => self.library(),
            ("POST", "/api/forget") => self.triage(head, body, Action::Forget),
            ("POST", "/api/restore") => self.triage(head, body, Action::Restore),
            (_, "/" | "/index.html" | "/app.css" | "/app.js" | "/api/library") => {
                Response::method_not_allowed("GET")
            }
            (_, "/api/forget" | "/api/restore") => Response::method_not_allowed("POST"),
            _ => Response::error(Status::NotFound, "no such page"),
        }
    }

    /// The same files `export` writes; the data element is served empty so
    /// the page knows to fetch.
    fn asset(&self, name: &str) -> Response {
        let loaded = assets::load();
        let Some((_, content)) = loaded.iter().find(|(asset, _)| *asset == name) else {
            return Response::error(Status::NotFound, "no such page");
        };
        match name {
            "index.html" => match export::embed(content, "") {
                Ok(html) => Response::ok("text/html; charset=utf-8", html.into_bytes()),
                Err(err) => self.failure(&err),
            },
            "app.css" => Response::ok("text/css; charset=utf-8", content.as_bytes().to_vec()),
            _ => Response::ok(
                "text/javascript; charset=utf-8",
                content.as_bytes().to_vec(),
            ),
        }
    }

    fn library(&self) -> Response {
        match self.build_library() {
            Ok(json) => Response::ok("application/json", json),
            Err(err) => self.failure(&err),
        }
    }

    /// Rebuilt from disk on every request, under the lock, so a `save` that
    /// ran meanwhile and a forget in flight are both seen whole.
    fn build_library(&self) -> Result<Vec<u8>, Error> {
        let archive = Archive::open(&self.root)?;
        let _lock = archive.lock(|| {})?;
        let loaded = library::load(&archive)?;
        let forgotten = library::forgotten(&self.root)?;
        let library = library::build(&loaded.snapshots, &forgotten, Shape::Serve);
        serde_json::to_vec(&library).map_err(|source| Error::Json {
            path: self.root.join(library::STATE_FILE),
            source,
        })
    }

    fn triage(&self, head: &Head, body: &[u8], action: Action) -> Response {
        if !head.header("content-type").is_some_and(is_json) {
            return Response::error(Status::UnsupportedMediaType, "send application/json");
        }
        let request: TriageRequest = match serde_json::from_slice(body) {
            Ok(request) => request,
            Err(err) => {
                return Response::error(
                    Status::BadRequest,
                    &format!("expected {{\"urls\":[...]}}: {err}"),
                );
            }
        };
        match triage::apply(&self.root, &request.urls, action, false, self.log) {
            Ok(outcome) => {
                let body = serde_json::json!({
                    "urls": outcome.changed,
                    "counts": {
                        "changed": outcome.changed.len(),
                        "unchanged": outcome.unchanged.len(),
                        "unknown": outcome.unknown.len(),
                        "forgotten": outcome.forgotten,
                    },
                    // The key the stand-in server answered with, kept so the
                    // documented contract stays true.
                    action.key(): outcome.changed,
                });
                Response::ok("application/json", body.to_string().into_bytes())
            }
            Err(err) => self.failure(&err),
        }
    }

    fn failure(&self, err: &Error) -> Response {
        self.log.warn(&err.to_string());
        Response::error(Status::InternalServerError, &err.to_string())
    }
}

#[derive(Deserialize)]
struct TriageRequest {
    urls: Vec<String>,
}

fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/json")
}

/// `127.0.0.1:<port>` or `localhost:<port>`, the port omitted only when it
/// is the one a browser omits. Anything else is a name this server was
/// never bound under, and a page that reached it by such a name is using
/// DNS to launder its origin.
fn host_allowed(host: &str, port: u16) -> bool {
    let (name, given_port) = match host.rsplit_once(':') {
        Some((name, given_port)) => (name, Some(given_port)),
        None => (host, None),
    };
    let name_ok = name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost");
    let port_ok = match given_port {
        Some(given) => {
            !given.is_empty()
                && given.bytes().all(|b| b.is_ascii_digit())
                && given.parse::<u16>() == Ok(port)
        }
        None => port == 80,
    };
    name_ok && port_ok
}

fn origin_allowed(origin: &str, port: u16) -> bool {
    origin
        .strip_prefix("http://")
        .is_some_and(|host| host_allowed(host, port))
}

struct Head {
    method: String,
    target: String,
    /// Names lower-cased; values as sent, trimmed.
    headers: Vec<(String, String)>,
}

impl Head {
    fn parse(text: &[u8]) -> Result<Self, Response> {
        let bad = || Response::error(Status::BadRequest, "malformed request");
        let text = std::str::from_utf8(text).map_err(|_| bad())?;
        let mut lines = text.split("\r\n");
        let mut request_line = lines.next().unwrap_or("").split(' ');
        let (Some(method), Some(target), Some(version), None) = (
            request_line.next(),
            request_line.next(),
            request_line.next(),
            request_line.next(),
        ) else {
            return Err(bad());
        };
        if method.is_empty()
            || !method.bytes().all(is_token)
            || !target.starts_with('/')
            || target.bytes().any(|b| b.is_ascii_control() || b == b' ')
            || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        {
            return Err(bad());
        }
        let mut headers = Vec::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').ok_or_else(bad)?;
            if name.is_empty()
                || !name.bytes().all(is_token)
                || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
            {
                return Err(bad());
            }
            let name = name.to_ascii_lowercase();
            // These fields have one meaning per request. Never let the
            // security check and framing silently choose the first value.
            if matches!(
                name.as_str(),
                "host" | "origin" | "content-length" | "content-type"
            ) && headers.iter().any(|(n, _)| n == &name)
            {
                return Err(bad());
            }
            headers.push((name, value.trim_matches([' ', '\t']).to_owned()));
        }
        Ok(Self {
            method: method.to_owned(),
            target: target.to_owned(),
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

// HTTP tokens are ASCII; accepting separators or controls creates competing
// interpretations of methods and header names.
fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "connection deadline expired"))
}

// Socket timeouts alone restart on every syscall. Recompute the remaining
// budget so a peer cannot keep a thread alive by dribbling bytes.
fn read_before(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

struct DeadlineWriter<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl Write for DeadlineWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream
            .set_write_timeout(Some(remaining(self.deadline)?))?;
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// The head's bytes and whatever body bytes arrived in the same read.
type HeadAndRest = (Vec<u8>, Vec<u8>);

/// Reads up to the blank line. Returns the head and whatever body bytes
/// arrived with it, or `None` when the client sent nothing at all.
fn read_head(stream: &mut TcpStream, deadline: Instant) -> Result<Option<HeadAndRest>, Response> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if buf.len() == HEAD_LIMIT {
            return Err(Response::error(
                Status::RequestHeaderFieldsTooLarge,
                "request head too large",
            ));
        }
        let size = chunk.len().min(HEAD_LIMIT - buf.len());
        let n = match read_before(stream, &mut chunk[..size], deadline) {
            Ok(0) => {
                return if buf.is_empty() {
                    Ok(None)
                } else {
                    Err(Response::error(Status::BadRequest, "incomplete request"))
                };
            }
            Ok(n) => n,
            Err(_) if buf.is_empty() => return Ok(None),
            Err(_) => return Err(Response::error(Status::BadRequest, "incomplete request")),
        };
        buf.extend_from_slice(&chunk[..n]);
        let search_from = buf.len().saturating_sub(n + 3);
        if let Some(at) = find(&buf[search_from..], b"\r\n\r\n") {
            let end = search_from + at + 4;
            let rest = buf.split_off(end);
            buf.truncate(end - 4);
            return Ok(Some((buf, rest)));
        }
    }
}

/// Only `Content-Length` bodies, and only up to the cap, which is checked
/// before anything is allocated or read.
fn read_body(
    stream: &mut TcpStream,
    head: &Head,
    mut body: Vec<u8>,
    deadline: Instant,
) -> Result<Vec<u8>, Response> {
    if head.header("transfer-encoding").is_some() {
        return Err(Response::error(
            Status::LengthRequired,
            "send a Content-Length, not a transfer encoding",
        ));
    }
    let length = match head.header("content-length") {
        None => 0,
        Some(value) => {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Response::error(Status::BadRequest, "bad Content-Length"));
            }
            value
                .parse::<usize>()
                .map_err(|_| Response::error(Status::BadRequest, "bad Content-Length"))?
        }
    };
    if length > BODY_LIMIT {
        return Err(Response::error(
            Status::ContentTooLarge,
            "request body too large",
        ));
    }
    if body.len() > length {
        return Err(Response::error(
            Status::BadRequest,
            "body longer than Content-Length",
        ));
    }
    let mut already = body.len();
    body.resize(length, 0);
    while already < length {
        match read_before(stream, &mut body[already..], deadline) {
            Ok(0) | Err(_) => {
                return Err(Response::error(Status::BadRequest, "incomplete body"));
            }
            Ok(n) => already += n,
        }
    }
    Ok(body)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    LengthRequired,
    ContentTooLarge,
    UnsupportedMediaType,
    RequestHeaderFieldsTooLarge,
    InternalServerError,
}

impl Status {
    fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::BadRequest => 400,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::LengthRequired => 411,
            Self::ContentTooLarge => 413,
            Self::UnsupportedMediaType => 415,
            Self::RequestHeaderFieldsTooLarge => 431,
            Self::InternalServerError => 500,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::BadRequest => "Bad Request",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not Found",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::LengthRequired => "Length Required",
            Self::ContentTooLarge => "Content Too Large",
            Self::UnsupportedMediaType => "Unsupported Media Type",
            Self::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
            Self::InternalServerError => "Internal Server Error",
        }
    }
}

struct Response {
    status: Status,
    content_type: &'static str,
    body: Vec<u8>,
    allow: Option<&'static str>,
}

impl Response {
    fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: Status::Ok,
            content_type,
            body,
            allow: None,
        }
    }

    fn error(status: Status, message: &str) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::json!({ "error": message })
                .to_string()
                .into_bytes(),
            allow: None,
        }
    }

    fn method_not_allowed(allow: &'static str) -> Self {
        Self {
            allow: Some(allow),
            ..Self::error(Status::MethodNotAllowed, "method not allowed")
        }
    }

    /// No CORS headers, ever: `Access-Control-Allow-Origin` would hand the
    /// library to exactly the pages the Origin check turns away.
    fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Connection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
            self.status.code(),
            self.status.reason(),
            self.content_type,
            self.body.len()
        );
        if let Some(allow) = self.allow {
            let _ = write!(out, "Allow: {allow}\r\n");
        }
        out.push_str("\r\n");
        stream.write_all(out.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

#[cfg(test)]
mod tests {
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

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    #[test]
    fn review_linger_has_a_total_deadline_even_when_bytes_keep_arriving() {
        let (mut socket, mut client) = socket_pair();
        let root = tempfile::tempdir().unwrap();
        let server = Server {
            root: root.path().into(),
            port: 0,
            log: Log::default(),
        };
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let start = Instant::now();
            server.finish(
                &mut socket,
                "test",
                &Response::error(Status::BadRequest, "test"),
            );
            done.send(start.elapsed()).unwrap();
        });
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client.read_to_end(&mut Vec::new()).unwrap();
        let dribbler = std::thread::spawn(move || {
            for _ in 0..35 {
                if client.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let elapsed = finished
            .recv_timeout(Duration::from_secs(3))
            .expect("drain deadline");
        assert!(elapsed >= Duration::from_millis(1800));
        assert!(elapsed < Duration::from_secs(3));
        worker.join().unwrap();
        dribbler.join().unwrap();
    }

    #[test]
    fn review_linger_never_reads_more_than_its_byte_limit() {
        let (mut socket, mut client) = socket_pair();
        let root = tempfile::tempdir().unwrap();
        let server = Server {
            root: root.path().into(),
            port: 0,
            log: Log::default(),
        };
        let writer = std::thread::spawn(move || {
            // An unaligned first read used to make the last read overshoot.
            client.write_all(b"x").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            client.write_all(&vec![b'x'; LINGER_LIMIT + 1024]).unwrap();
            client.shutdown(Shutdown::Write).unwrap();
            client.read_to_end(&mut Vec::new()).unwrap();
        });
        server.finish(
            &mut socket,
            "test",
            &Response::error(Status::BadRequest, "test"),
        );
        let mut left = Vec::new();
        socket.read_to_end(&mut left).unwrap();
        assert!(left.len() <= LINGER_LIMIT, "drained {} bytes", left.len());
        assert!(left.len() >= 1025, "drained only {} bytes", left.len());
        writer.join().unwrap();
    }

    #[test]
    fn review_head_and_body_share_one_deadline() {
        let (mut socket, mut client) = socket_pair();
        let deadline = Instant::now() + Duration::from_millis(300);
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            client
                .write_all(b"GET / HTTP/1.1\r\nContent-Length: 1\r\n\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(250));
            let _ = client.write_all(b"x");
        });
        let (head, rest) = read_head(&mut socket, deadline).ok().unwrap().unwrap();
        let head = Head::parse(&head).ok().unwrap();
        let err = read_body(&mut socket, &head, rest, deadline).unwrap_err();
        assert_eq!(err.status, Status::BadRequest);
        writer.join().unwrap();
    }

    #[test]
    fn review_response_write_has_a_total_deadline() {
        let (mut socket, _client) = socket_pair();
        let mut writer = DeadlineWriter {
            stream: &mut socket,
            deadline: Instant::now() + Duration::from_millis(100),
        };
        let start = Instant::now();
        let err = writer.write_all(&vec![b'x'; 32 * 1024 * 1024]).unwrap_err();
        assert!(matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
