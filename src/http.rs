//! The HTTP/1.1 wire `serve` speaks, written directly on `std::net`:
//! request framing, the Host and Origin checks, and responses.
//!
//! slice: triage, platforms
//! why: Any web page the user has open can reach a localhost server, and
//!      `serve` discloses every URL they ever had open and mutates their
//!      library. Owning the HTTP layer keeps the defences (loopback bind in
//!      `server::run`, the Host and Origin checks here) and every input bound
//!      in one place a reviewer can read top to bottom, with no dormant
//!      dependency between them and the socket. No keep-alive: each
//!      connection carries one request and is closed.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

/// Request line plus headers, including the final CRLF CRLF. A browser's own
/// headers fit in a few hundred bytes; the margin is for localhost cookies.
pub(crate) const HEAD_LIMIT: usize = 16 * 1024;
/// A bulk forget of every page in a large archive is well under this.
pub(crate) const BODY_LIMIT: usize = 4 * 1024 * 1024;
/// Browsers open connections they never write to (preconnects); a thread
/// waiting on one must give up rather than hold a socket forever.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long, and how much, to keep reading after the response so the close
/// is orderly (see `respond`).
const LINGER: Duration = Duration::from_secs(2);
const LINGER_LIMIT: usize = 1024 * 1024;

/// Rules 2 and 3: the request must be addressed to this server by its
/// own name, and if it declares where it came from, that must be here.
pub(crate) fn check_headers(head: &Head, port: u16) -> Result<(), Response> {
    if !head
        .header("host")
        .is_some_and(|host| host_allowed(host, port))
    {
        return Err(Response::error(
            Status::Forbidden,
            "the Host header does not name this server",
        ));
    }
    if let Some(origin) = head.header("origin")
        && !origin_allowed(origin, port)
    {
        return Err(Response::error(
            Status::Forbidden,
            "cross-origin requests are not accepted",
        ));
    }
    Ok(())
}

/// Writes the response, then drains what the client is still sending
/// before closing. A rejection is sent before the body is read, and
/// closing a socket with unread input makes the kernel send a reset that
/// can discard the response before the client has read it.
pub(crate) fn respond(stream: &mut TcpStream, response: &Response) {
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

/// Every POST body: JSON by its declared type, so a form post, which a page
/// on another origin can send without a preflight, never reaches a handler.
pub(crate) fn json_request<T: DeserializeOwned>(
    head: &Head,
    body: &[u8],
    expected: &str,
) -> Result<T, Response> {
    if !head.header("content-type").is_some_and(is_json) {
        return Err(Response::error(
            Status::UnsupportedMediaType,
            "send application/json",
        ));
    }
    serde_json::from_slice(body)
        .map_err(|err| Response::error(Status::BadRequest, &format!("expected {expected}: {err}")))
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

pub(crate) struct Head {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) fields: Fields,
}

impl Head {
    pub(crate) fn parse(text: &[u8]) -> Result<Self, Response> {
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
        Ok(Self {
            method: method.to_owned(),
            target: target.to_owned(),
            fields: Fields::parse(lines).ok_or_else(bad)?,
        })
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.fields.get(name)
    }
}

/// The header fields after a request or status line: names lower-cased;
/// values as sent, trimmed.
pub(crate) struct Fields(Vec<(String, String)>);

impl Fields {
    /// `None` for a malformed line or a repeated field that has one meaning.
    fn parse<'a>(lines: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':')?;
            if name.is_empty()
                || !name.bytes().all(is_token)
                || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
            {
                return None;
            }
            let name = name.to_ascii_lowercase();
            // These fields have one meaning per message. Never let the
            // security check and framing silently choose the first value.
            if matches!(
                name.as_str(),
                "host" | "origin" | "content-length" | "content-type"
            ) && fields.iter().any(|(n, _)| n == &name)
            {
                return None;
            }
            fields.push((name, value.trim_matches([' ', '\t']).to_owned()));
        }
        Some(Self(fields))
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
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

/// Why a message could not be framed. A server answers each with its own
/// status; a client gives up on the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    /// Nothing arrived before the peer closed or the deadline passed.
    Closed,
    /// The head or the body stopped short.
    Incomplete,
    /// Over the caller's limit, for the head as read or the body as declared.
    TooLarge,
    /// A transfer encoding instead of a `Content-Length`.
    Chunked,
    /// A `Content-Length` that is not a plain decimal number.
    BadLength,
    /// More bytes arrived with the head than the `Content-Length` declares.
    Overrun,
}

/// The head's bytes and whatever body bytes arrived in the same read.
type HeadAndRest = (Vec<u8>, Vec<u8>);

/// Reads up to the blank line, at most `limit` bytes. Returns the head and
/// whatever body bytes arrived with it.
pub(crate) fn read_head(
    stream: &mut TcpStream,
    deadline: Instant,
    limit: usize,
) -> Result<HeadAndRest, Framing> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if buf.len() == limit {
            return Err(Framing::TooLarge);
        }
        let size = chunk.len().min(limit - buf.len());
        let n = match read_before(stream, &mut chunk[..size], deadline) {
            Ok(n) if n > 0 => n,
            _ if buf.is_empty() => return Err(Framing::Closed),
            _ => return Err(Framing::Incomplete),
        };
        buf.extend_from_slice(&chunk[..n]);
        let search_from = buf.len().saturating_sub(n + 3);
        if let Some(at) = find(&buf[search_from..], b"\r\n\r\n") {
            let end = search_from + at + 4;
            let rest = buf.split_off(end);
            buf.truncate(end - 4);
            return Ok((buf, rest));
        }
    }
}

/// Only `Content-Length` bodies, and only up to `limit`, which is checked
/// before anything is allocated or read. No length is an empty body.
pub(crate) fn read_body(
    stream: &mut TcpStream,
    fields: &Fields,
    mut body: Vec<u8>,
    deadline: Instant,
    limit: usize,
) -> Result<Vec<u8>, Framing> {
    if fields.get("transfer-encoding").is_some() {
        return Err(Framing::Chunked);
    }
    let length = match fields.get("content-length") {
        None => 0,
        Some(value) => {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Framing::BadLength);
            }
            value.parse::<usize>().map_err(|_| Framing::BadLength)?
        }
    };
    if length > limit {
        return Err(Framing::TooLarge);
    }
    if body.len() > length {
        return Err(Framing::Overrun);
    }
    let mut already = body.len();
    body.resize(length, 0);
    while already < length {
        match read_before(stream, &mut body[already..], deadline) {
            Ok(0) | Err(_) => return Err(Framing::Incomplete),
            Ok(n) => already += n,
        }
    }
    Ok(body)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
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
    pub(crate) fn code(self) -> u16 {
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

pub(crate) struct Response {
    pub(crate) status: Status,
    content_type: &'static str,
    body: Vec<u8>,
    allow: Option<&'static str>,
}

impl Response {
    pub(crate) fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: Status::Ok,
            content_type,
            body,
            allow: None,
        }
    }

    pub(crate) fn error(status: Status, message: &str) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::json!({ "error": message })
                .to_string()
                .into_bytes(),
            allow: None,
        }
    }

    pub(crate) fn method_not_allowed(allow: &'static str) -> Self {
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
#[path = "http_tests.rs"]
mod tests;
