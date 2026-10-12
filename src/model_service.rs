//! The Library's search through the model service on this machine: one
//! forwarding rule for one path prefix, to one loopback port.
//!
//! slice: library, triage
//! why: The Library ranks through a model it does not own, and every byte
//!      crossing that boundary is bounded and checked here. The request is
//!      built from constants, so nothing the browser sent but the body
//!      reaches the service, and a reply is used only when it names this
//!      archive, frames cleanly within its bounds, is JSON and carries the
//!      service's proof for this very request (`service_auth`).

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::capture::Log;
use crate::http::{
    self, DeadlineWriter, Framing, HEAD_LIMIT, Head, Response, ResponseHead, Status,
};
use crate::service_auth::{self, Key, Reply, Request, Unusable};

/// Every path the shell forwards starts with this; the rest is the service's.
pub const PREFIX: &str = "/model/";
/// What the service answers below the prefix, by method and path. Anything
/// else under the prefix is refused here, without contacting the service.
const ROUTES: &[(&str, &str)] = &[("POST", "api/rank")];
/// The service names the archive it loaded in this header on every reply
/// (`ARCHIVE_HEADER` in `lab/tagger/tagger/app/server.py`).
const ARCHIVE_HEADER: &str = "x-kmt-archive";
/// One deadline over connect, write and read: the Library searches by
/// keyword rather than wait on a busy or stuck model.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(2);
/// About 80,000 page keys, checked against the declared length before a
/// byte of the body is read.
const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;

/// The model service's loopback port, from `serve --model-port`, and the
/// key that proves each exchange with it.
pub struct ModelService {
    port: u16,
    key: Key,
}

/// Why a forward got no usable answer. Logged by name only, never with a
/// query or a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// `serve` was started without `--model-port`.
    Off,
    /// Nothing accepting the connection: not running, or still loading its
    /// model.
    Refused,
    Timeout,
    /// A declared body over `RESPONSE_LIMIT`.
    TooLarge,
    /// Not a framed JSON reply.
    Malformed,
    /// The reply names another archive, or none, or this one has no identity.
    OtherArchive,
    /// A status the Library has no use for.
    Upstream(u16),
    /// No reply MAC, or not this key's for this request and these bytes:
    /// whatever answered is not the service this launch started.
    Unproven,
    /// The system gave no randomness for a nonce.
    Entropy,
}

impl Failure {
    fn response(self) -> Response {
        let (status, message) = match self {
            Self::Off => (Status::ServiceUnavailable, "no model service is set"),
            Self::Refused => (
                Status::ServiceUnavailable,
                "the model service is not answering",
            ),
            Self::Timeout => (Status::GatewayTimeout, "the model service took too long"),
            Self::TooLarge => (Status::BadGateway, "the model service answered too much"),
            Self::Malformed => (
                Status::BadGateway,
                "the model service answered in a form this server does not take",
            ),
            Self::OtherArchive => (
                Status::BadGateway,
                "the model service is serving another archive",
            ),
            Self::Upstream(_) => (Status::BadGateway, "the model service failed"),
            Self::Unproven => (
                Status::BadGateway,
                "the model service did not prove its answer",
            ),
            Self::Entropy => (
                Status::ServiceUnavailable,
                "the model service cannot be asked now",
            ),
        };
        Response::error(status, message)
    }
}

/// Answers a request whose path starts with `PREFIX`: through the service
/// when the path is one of `ROUTES`, a POST is JSON, the service is set and
/// it answers for this archive.
pub fn forward(
    service: Option<&ModelService>,
    root: &Path,
    head: &Head,
    path: &str,
    body: &[u8],
    log: Log,
) -> Response {
    let below = &path[PREFIX.len()..];
    let Some(&(method, upstream)) = ROUTES
        .iter()
        .find(|&&(method, route)| method == head.method && route == below)
    else {
        return Response::error(Status::NotFound, "no such page");
    };
    if method == "POST"
        && let Err(response) = http::require_json(head)
    {
        return response;
    }
    let answered = match service {
        None => Err(Failure::Off),
        Some(service) => service.exchange(upstream, body, root),
    };
    answered.unwrap_or_else(|failure| {
        log.note(&format!("model service: {failure:?}"));
        failure.response()
    })
}

impl ModelService {
    /// The service on `port` when `key_file` holds a usable key. Otherwise
    /// nothing is forwarded, said once here without the file's bytes, and
    /// the Library searches by keyword.
    pub fn new(port: u16, key_file: Option<&Path>, log: Log) -> Option<Self> {
        let why = match key_file.map(Key::read) {
            Some(Ok(key)) => return Some(Self { port, key }),
            None => "no --model-secret-file was given",
            Some(Err(Unusable::Unreadable)) => "its key file cannot be read",
            Some(Err(Unusable::Unsafe)) => {
                "its key file is a link, someone else's, or readable by others"
            }
            Some(Err(Unusable::Malformed)) => "its key file does not hold a key",
        };
        log.warn(&format!(
            "the model service is off: {why}; search is by keyword"
        ));
        None
    }

    /// One request on its own connection, closed after the reply.
    fn exchange(&self, path: &str, body: &[u8], root: &Path) -> Result<Response, Failure> {
        let archive = archive_identity(root);
        let nonce = service_auth::nonce().ok_or(Failure::Entropy)?;
        let time = service_auth::now();
        let (method, content_type, target) = ("POST", "application/json", format!("/{path}"));
        let mac = self.key.sign(&Request {
            nonce: &nonce,
            time: &time,
            method,
            path: &target,
            content_type,
            body,
        });
        let deadline = Instant::now() + FORWARD_TIMEOUT;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        // A loopback connect that does not complete is a service not
        // accepting, whatever the error: Windows retries a refused connect
        // for about two seconds, so this deadline can expire first.
        let mut stream =
            TcpStream::connect_timeout(&address, FORWARD_TIMEOUT).map_err(|_| Failure::Refused)?;
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\n{}: {nonce}\r\n{}: {time}\r\n{}: {mac}\r\n\
             Connection: close\r\n\r\n",
            self.port,
            body.len(),
            service_auth::NONCE,
            service_auth::TIME,
            service_auth::REQUEST_MAC,
        );
        let mut writer = DeadlineWriter {
            stream: &mut stream,
            deadline,
        };
        writer
            .write_all(request.as_bytes())
            .and_then(|()| writer.write_all(body))
            .map_err(|_| late(deadline, Failure::Refused))?;
        let (head, rest) = http::read_head(&mut stream, deadline, HEAD_LIMIT)
            .map_err(|_| late(deadline, Failure::Malformed))?;
        let reply = ResponseHead::parse(&head).ok_or(Failure::Malformed)?;
        // Judged on the reply, so a service that is not running or not
        // answering is told apart from one serving another archive.
        if archive.is_none() || reply.fields.get(ARCHIVE_HEADER) != archive.as_deref() {
            return Err(Failure::OtherArchive);
        }
        let status = match reply.code {
            200 => Status::Ok,
            400 => Status::BadRequest,
            503 => Status::ServiceUnavailable,
            code => return Err(Failure::Upstream(code)),
        };
        if !reply.fields.get("content-type").is_some_and(http::is_json)
            || reply.fields.get("content-length").is_none()
        {
            return Err(Failure::Malformed);
        }
        let body = http::read_body(&mut stream, &reply.fields, rest, deadline, RESPONSE_LIMIT)
            .map_err(|framing| match framing {
                Framing::TooLarge => Failure::TooLarge,
                _ => late(deadline, Failure::Malformed),
            })?;
        let proven = Reply {
            nonce: &nonce,
            method,
            path: &target,
            status: reply.code,
            content_type: reply.fields.get("content-type").unwrap_or(""),
            archive: reply.fields.get(ARCHIVE_HEADER).unwrap_or(""),
            body: &body,
        };
        if !reply
            .fields
            .get(service_auth::RESPONSE_MAC)
            .is_some_and(|mac| self.key.verify(&proven, mac))
        {
            return Err(Failure::Unproven);
        }
        Ok(Response::json(status, body))
    }
}

/// A failure the deadline explains is a timeout, whatever else it looked like.
fn late(deadline: Instant, otherwise: Failure) -> Failure {
    if Instant::now() >= deadline {
        Failure::Timeout
    } else {
        otherwise
    }
}

/// `<device>:<inode>` of the archive directory, read on every forward so it
/// is never stale. A path compare would miss the same directory spelled
/// another way on a case insensitive volume.
#[cfg(unix)]
fn archive_identity(root: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(root).ok()?;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

/// No stable directory identity here, so no reply is ever taken as this
/// archive's and search stays keyword only.
#[cfg(not(unix))]
fn archive_identity(_root: &Path) -> Option<String> {
    None
}
