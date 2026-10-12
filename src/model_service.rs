//! The Library's search through the model service on this machine: one
//! forwarding rule for one path prefix, to one loopback port.
//!
//! slice: library, triage
//! why: The Library ranks through a model it does not own, and every byte
//!      crossing that boundary is bounded and checked here. The request is
//!      built from constants, so nothing the browser sent but the body
//!      reaches the service, and a reply is used only when it names this
//!      archive, frames cleanly within its bounds and is JSON.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::capture::Log;
use crate::http::{
    self, DeadlineWriter, Framing, HEAD_LIMIT, Head, Response, ResponseHead, Status,
};

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

/// The model service's loopback port, from `serve --model-port`.
pub struct ModelService {
    pub port: u16,
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
    /// One request on its own connection, closed after the reply.
    fn exchange(&self, path: &str, body: &[u8], root: &Path) -> Result<Response, Failure> {
        let archive = archive_identity(root);
        let deadline = Instant::now() + FORWARD_TIMEOUT;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        // A loopback connect that does not complete is a service not
        // accepting, whatever the error: Windows retries a refused connect
        // for about two seconds, so this deadline can expire first.
        let mut stream =
            TcpStream::connect_timeout(&address, FORWARD_TIMEOUT).map_err(|_| Failure::Refused)?;
        let request = format!(
            "POST /{path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.port,
            body.len()
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
