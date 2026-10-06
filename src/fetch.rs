//! The only network code in knowmoretabs: one cookieless, guarded GET,
//! with every redirect hop checked before it is requested.
//!
//! slice: enrich, content
//! why: A network command is opt-in because it sends the URLs you visited
//!      to their own sites, so what it sends is kept to the minimum and every
//!      hop is checked. No cookies, no referrer, no proxy from the environment;
//!      each redirect is followed here, not by the client, so that a hop to
//!      this machine, the private network or a login screen is refused before
//!      it is requested; and every name is resolved through a resolver that
//!      refuses private addresses, so a public name pointing inward is caught
//!      at the one moment it matters, the connect. What a response means is
//!      the caller's business; how it was fetched is this module's alone.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

use crate::guard::{
    self, carries_token, is_login_page, is_login_redirect, is_private_host, is_public,
    is_search_results, is_web,
};
use crate::head;

pub const MAX_REDIRECTS: usize = 10;
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// One request per second to any one host.
pub const PACE: Duration = Duration::from_secs(1);
/// What a browser asks for when it wants a page.
pub const ACCEPT_HTML: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5";

/// Says what is asking, in the form sites already know how to read.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; knowmoretabs/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/littleorgans/knowmoretabs)"
);

pub struct Fetcher {
    agent: ureq::Agent,
    pacer: Pacer,
    forgotten: BTreeSet<String>,
    timeout: Duration,
}

/// Why a GET ended without a response to read: a hop the rules refuse, or
/// a request that did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    InvalidUrl,
    /// The URL, or a redirect, is not `http` or `https` with a host.
    NotWeb,
    PrivateNetwork {
        redirected: bool,
    },
    /// The name resolved to this machine or the private network.
    PrivateAddress,
    Forgotten,
    TokenOrSearch,
    /// A sign-in, sign-up or verification screen, where it was met.
    Login(Url),
    InvalidRedirect {
        status: u16,
    },
    TooManyRedirects,
    /// Timeout, connection failure and the like, as a short reason.
    Failed(String),
}

/// The final response, its body not yet read. The request's deadline
/// still covers reading it.
pub struct Response {
    /// Where the redirects ended.
    pub url: Url,
    pub status: u16,
    /// Empty when the server sent none.
    pub content_type: String,
    inner: ureq::http::Response<ureq::Body>,
}

impl Fetcher {
    pub fn new(forgotten: &BTreeSet<String>) -> Self {
        let mut fetcher = Self::with(test_timeout().unwrap_or(TIMEOUT), test_address(), PACE);
        fetcher.forgotten = forgotten
            .iter()
            .filter_map(|raw| {
                let mut url = Url::parse(raw).ok()?;
                url.set_fragment(None);
                Some(url.to_string())
            })
            .collect();
        fetcher
    }

    fn with(timeout: Duration, test_address: Option<SocketAddr>, pace: Duration) -> Self {
        let config = Config::builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .user_agent(USER_AGENT)
            .timeout_global(Some(timeout))
            .build();
        Self {
            agent: ureq::Agent::with_parts(
                config,
                DefaultConnector::new(),
                PublicOnly { test_address },
            ),
            pacer: Pacer::new(pace),
            forgotten: BTreeSet::new(),
            timeout,
        }
    }

    /// Requests one page, following redirects itself so that every hop is
    /// checked and paced first. One deadline covers the hops and the body.
    pub fn get(&self, raw: &str, accept: &str) -> Result<Response, Refusal> {
        let deadline = Instant::now() + self.timeout;
        let Ok(mut url) = Url::parse(raw) else {
            return Err(Refusal::InvalidUrl);
        };
        url.set_fragment(None);
        for hop in 0..=MAX_REDIRECTS {
            if !is_web(&url) {
                return Err(Refusal::NotWeb);
            }
            if is_private_host(&url) {
                return Err(Refusal::PrivateNetwork {
                    redirected: hop > 0,
                });
            }
            if self.forgotten.contains(url.as_str()) {
                return Err(Refusal::Forgotten);
            }
            if carries_token(&url) || is_search_results(&url) {
                return Err(Refusal::TokenOrSearch);
            }
            if is_login_page(&url) || (hop > 0 && is_login_redirect(&url)) {
                return Err(Refusal::Login(url));
            }
            if !self.pacer.wait(&guard::host_key(&url), deadline) {
                return Err(Refusal::Failed("timeout".to_owned()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Refusal::Failed("timeout".to_owned()));
            }
            let response = self
                .agent
                .get(url.as_str())
                .config()
                .accept(accept)
                .timeout_global(Some(remaining))
                .build()
                .call()
                .map_err(|err| refusal(&err))?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok());
            if let (300..=399, Some(location)) = (status, location) {
                url = url
                    .join(location.trim())
                    .map_err(|_| Refusal::InvalidRedirect { status })?;
                url.set_fragment(None);
                continue;
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            return Ok(Response {
                url,
                status,
                content_type,
                inner: response,
            });
        }
        Err(Refusal::TooManyRedirects)
    }
}

impl Response {
    /// The media type alone, lowercase; empty when none was sent.
    pub fn mime(&self) -> String {
        self.content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    }

    /// Whether the body arrives as sent: no `Content-Encoding` but
    /// `identity`. No decoder is compiled in.
    pub fn is_identity(&self) -> bool {
        self.inner
            .headers()
            .get_all("content-encoding")
            .iter()
            .all(|value| {
                value
                    .to_str()
                    .is_ok_and(|v| v.trim().eq_ignore_ascii_case("identity"))
            })
    }

    /// At most `cap` bytes of the body, and no more than the chunk that ends
    /// the head when `stop_at_head`. Fails with a short reason.
    pub fn read(&mut self, cap: usize, stop_at_head: bool) -> Result<Vec<u8>, String> {
        read_head(self.inner.body_mut().as_reader(), cap, stop_at_head)
            .map_err(|err| io_reason(&err))
    }
}

/// Reads until the head ends (when `stop_at_head`), the cap, or the end.
fn read_head(mut body: impl Read, cap: usize, stop_at_head: bool) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut boundary = head::Boundary::default();
    let mut chunk = vec![0u8; 64 * 1024];
    while bytes.len() < cap {
        let want = chunk.len().min(cap - bytes.len());
        let n = match body.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        bytes.extend_from_slice(&chunk[..n]);
        if stop_at_head && boundary.feed(&chunk[..n]) {
            break;
        }
    }
    Ok(bytes)
}

fn refusal(err: &ureq::Error) -> Refusal {
    match err {
        ureq::Error::Io(io)
            if io
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<PrivateAddress>) =>
        {
            Refusal::PrivateAddress
        }
        ureq::Error::Timeout(_) => Refusal::Failed("timeout".to_owned()),
        ureq::Error::HostNotFound => Refusal::Failed("host not found".to_owned()),
        ureq::Error::ConnectionFailed => Refusal::Failed("connection failed".to_owned()),
        ureq::Error::Io(io) => Refusal::Failed(io_reason(io)),
        other => Refusal::Failed(other.to_string().chars().take(160).collect()),
    }
}

fn io_reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::TimedOut => "timeout".to_owned(),
        io::ErrorKind::ConnectionRefused => "connection refused".to_owned(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => {
            "connection reset".to_owned()
        }
        io::ErrorKind::UnexpectedEof => "connection closed early".to_owned(),
        _ => {
            let text = err.to_string();
            if text.to_lowercase().contains("timeout") || text.to_lowercase().contains("timed out")
            {
                "timeout".to_owned()
            } else {
                text.chars().take(160).collect()
            }
        }
    }
}

/// Spaces requests to each host. Every request, redirect hops included,
/// takes the next free slot for its host, whichever worker asks.
struct Pacer {
    interval: Duration,
    next: Mutex<HashMap<String, Instant>>,
}

impl Pacer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            next: Mutex::new(HashMap::new()),
        }
    }

    fn wait(&self, host: &str, deadline: Instant) -> bool {
        let slot = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let slot = next.get(host).map_or(now, |at| (*at).max(now));
            if slot >= deadline {
                return false;
            }
            next.insert(host.to_owned(), slot + self.interval);
            slot
        };
        std::thread::sleep(slot.saturating_duration_since(Instant::now()));
        Instant::now() < deadline
    }
}

/// Resolves as the system does, then refuses unless every address is
/// public. Checking the addresses the connection will actually use closes
/// the gap a check of the name alone leaves: a public name that resolves,
/// or is re-pointed, to this machine or the private network.
#[derive(Debug)]
struct PublicOnly {
    /// Tests point every name at their own server on loopback.
    test_address: Option<SocketAddr>,
}

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if let Some(address) = self.test_address {
            let mut addresses = self.empty();
            addresses.push(address);
            return Ok(addresses);
        }
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if addresses.iter().all(|a| is_public(a.ip())) {
            Ok(addresses)
        } else {
            Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                PrivateAddress,
            )))
        }
    }
}

#[derive(Debug)]
struct PrivateAddress;

impl std::fmt::Display for PrivateAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("resolves to a private network address")
    }
}

impl std::error::Error for PrivateAddress {}

/// Debug builds only, so no shipped binary can be pointed at loopback: the
/// integration tests' server, standing in for every host.
#[cfg(debug_assertions)]
fn test_address() -> Option<SocketAddr> {
    std::env::var("KNOWMORETABS_TEST_RESOLVE")
        .ok()?
        .parse()
        .ok()
}

#[cfg(not(debug_assertions))]
fn test_address() -> Option<SocketAddr> {
    None
}

#[cfg(debug_assertions)]
fn test_timeout() -> Option<Duration> {
    let millis = std::env::var("KNOWMORETABS_TEST_TIMEOUT_MS").ok()?;
    millis.parse().ok().map(Duration::from_millis)
}

#[cfg(not(debug_assertions))]
fn test_timeout() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    use super::*;
    use crate::metadata_fetch::HEAD_CAP;

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_hooks_are_absent() {
        assert_eq!(test_address(), None);
        assert_eq!(test_timeout(), None);
    }

    #[test]
    fn the_pacer_spaces_one_host_and_leaves_others_alone() {
        let pacer = Arc::new(Pacer::new(Duration::from_millis(200)));
        let start = Instant::now();
        pacer.wait("a.test", start + TIMEOUT);
        pacer.wait("b.test", start + TIMEOUT);
        assert!(start.elapsed() < Duration::from_millis(150));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let pacer = Arc::clone(&pacer);
                std::thread::spawn(move || pacer.wait("a.test", start + TIMEOUT))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(400),
            "three requests to one host need two intervals, took {elapsed:?}"
        );
    }

    #[test]
    fn reading_stops_at_the_end_of_the_head_or_the_cap() {
        let head = format!("<head><script>{}</script></HEAD>", "x".repeat(200_000));
        let page = format!("{head}<body>{}</body>", "y".repeat(5_000_000));
        let read = read_head(page.as_bytes(), HEAD_CAP, true).unwrap();
        assert!(
            read.len() >= head.len() && read.len() < head.len() + 64 * 1024,
            "read {} bytes of a {}-byte head",
            read.len(),
            head.len()
        );
        assert_eq!(
            read_head(page.as_bytes(), HEAD_CAP, false).unwrap().len(),
            HEAD_CAP
        );
        let body = "z".repeat(HEAD_CAP + 10);
        assert_eq!(
            read_head(body.as_bytes(), HEAD_CAP, true).unwrap().len(),
            HEAD_CAP
        );
    }

    #[test]
    fn head_boundaries_inside_markup_do_not_truncate_the_response() {
        for prefix in [
            "<script>const x = '</head>';",
            "<!-- </head>",
            "<meta content='</head>",
            "<script>const x = '</scripture></head>';",
        ] {
            let close = if prefix.starts_with("<script") {
                "</script>"
            } else if prefix.starts_with("<!--") {
                "-->"
            } else {
                "'>"
            };
            let page = format!(
                "<head>{prefix}{}{close}<title>Found</title></head>{}",
                "x".repeat(100_000),
                "y".repeat(100_000)
            );
            let bytes = read_head(page.as_bytes(), HEAD_CAP, true).unwrap();
            assert_eq!(
                head::scan(&String::from_utf8(bytes).unwrap())
                    .title
                    .as_deref(),
                Some("Found"),
                "{prefix}"
            );
        }
    }

    /// A server that accepts, reads the request, and says nothing.
    fn silent_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                held.push(stream);
            }
        });
        address
    }

    #[test]
    fn the_caller_chooses_the_accept_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut accept = Vec::new();
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("accept:") {
                    accept.push(value.trim().to_owned());
                }
                line.clear();
            }
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n");
            accept
        });
        let fetcher = Fetcher::with(Duration::from_secs(5), Some(address), PACE);
        let response = fetcher.get("http://json.test/", "application/json").ok();
        assert_eq!(response.map(|r| r.status), Some(204));
        assert_eq!(server.join().unwrap(), ["application/json"]);
    }

    #[test]
    fn a_server_that_never_answers_is_a_timeout() {
        let fetcher = Fetcher::with(Duration::from_millis(300), Some(silent_server()), PACE);
        let start = Instant::now();
        let refusal = fetcher.get("http://slow.test/", ACCEPT_HTML).err();
        assert_eq!(refusal, Some(Refusal::Failed("timeout".to_owned())));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_server_that_stalls_inside_the_head_is_a_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 100000\r\n\r\n<head><title>x",
            );
            std::thread::sleep(Duration::from_secs(3));
        });
        let fetcher = Fetcher::with(Duration::from_millis(400), Some(address), PACE);
        let Ok(mut response) = fetcher.get("http://stall.test/", ACCEPT_HTML) else {
            panic!("the head of the response arrived");
        };
        assert_eq!(response.read(HEAD_CAP, true), Err("timeout".to_owned()));
    }

    #[test]
    fn without_a_test_address_loopback_names_are_refused_at_the_connect() {
        // `localhost` resolves to loopback everywhere; the resolver, not the
        // URL rules, is what is exercised when the URL check is bypassed.
        let fetcher = Fetcher::with(Duration::from_secs(5), None, PACE);
        let config = Config::builder().build();
        let err = PublicOnly { test_address: None }
            .resolve(
                &"http://localhost:9/".parse().unwrap(),
                &config,
                NextTimeout {
                    after: ureq::unversioned::transport::time::Duration::NotHappening,
                    reason: ureq::Timeout::Resolve,
                },
            )
            .unwrap_err();
        assert_eq!(refusal(&err), Refusal::PrivateAddress);
        assert_eq!(
            fetcher.get("http://localhost:9/", ACCEPT_HTML).err(),
            Some(Refusal::PrivateNetwork { redirected: false })
        );
    }
}
