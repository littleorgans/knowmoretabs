//! The only network code in knowmoretabs: one cookieless, guarded GET,
//! with every redirect hop checked before it is requested, and the guarded
//! connect behind every connection a rendered page makes.
//!
//! slice: enrich, content
//! why: A network command is opt-in because it sends the URLs you visited
//!      to their own sites, so what it sends is kept to the minimum and every
//!      hop is checked. No cookies, no referrer, no proxy from the environment;
//!      each redirect is followed here, not by the client, so that a hop to
//!      this machine, the private network or a login screen is refused before
//!      it is requested; and every name is resolved through a resolver that
//!      refuses private addresses, so a public name pointing inward is caught
//!      at the one moment it matters, the connect. A browser rendering a
//!      page connects through the same rule, so the headless tier cannot
//!      reach what plain fetching may not. What a response means is the
//!      caller's business; how it was fetched is this module's alone.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read};
use std::net::{Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::{Host, Url};

use crate::guard::{
    self, carries_token, is_login_page, is_login_redirect, is_private_host, is_public,
    is_search_results, is_web,
};
use crate::head;

pub const MAX_REDIRECTS: usize = 10;
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// One request per second to any one host.
pub const PACE: Duration = Duration::from_secs(1);
/// The slowest a host is paced after telling us to slow down, so that one
/// paced request still fits inside [`TIMEOUT`].
pub const MAX_PACE: Duration = Duration::from_secs(8);
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

impl Refusal {
    /// What a log line says about it, the same for every command.
    pub fn reason(&self) -> String {
        match self {
            Self::InvalidUrl => "not a valid URL".to_owned(),
            Self::NotWeb => "redirected away from the web".to_owned(),
            Self::PrivateNetwork { redirected: false } => "private network".to_owned(),
            Self::PrivateNetwork { redirected: true } => {
                "redirected to a private network".to_owned()
            }
            Self::PrivateAddress => "private network address".to_owned(),
            Self::Forgotten => "forgotten page, not fetched".to_owned(),
            Self::TokenOrSearch => "token or search URL, not fetched".to_owned(),
            Self::Login(_) => "redirected to a login page".to_owned(),
            Self::InvalidRedirect { status } => format!("HTTP {status} to an invalid URL"),
            Self::TooManyRedirects => "too many redirects".to_owned(),
            Self::Failed(reason) => reason.clone(),
        }
    }

    /// A rule kept the request home, as opposed to a request that failed.
    pub fn is_rule(&self) -> bool {
        matches!(
            self,
            Self::NotWeb
                | Self::PrivateNetwork { .. }
                | Self::PrivateAddress
                | Self::Forgotten
                | Self::TokenOrSearch
        )
    }
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
            .filter_map(|raw| guard::page_url(raw).map(String::from))
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
        let Some(mut url) = guard::page_url(raw) else {
            return Err(Refusal::InvalidUrl);
        };
        for hop in 0..=MAX_REDIRECTS {
            self.check(&url, hop)?;
            if !self.pace(&url, deadline) {
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

    /// What the rules say about `url` as hop `hop` of a request: 0 for the
    /// address asked for, more for a redirect. A browser rendering a page
    /// asks the same before it navigates and of where the page ended.
    pub fn check(&self, url: &Url, hop: usize) -> Result<(), Refusal> {
        if !is_web(url) {
            return Err(Refusal::NotWeb);
        }
        if is_private_host(url) {
            return Err(Refusal::PrivateNetwork {
                redirected: hop > 0,
            });
        }
        if self.forgotten.contains(url.as_str()) {
            return Err(Refusal::Forgotten);
        }
        if carries_token(url) || is_search_results(url) {
            return Err(Refusal::TokenOrSearch);
        }
        if is_login_page(url) || (hop > 0 && is_login_redirect(url)) {
            return Err(Refusal::Login(url.clone()));
        }
        Ok(())
    }

    /// Waits for the next free slot of `url`'s host, the one pacer every
    /// request shares; false when that slot falls past `deadline`.
    pub fn pace(&self, url: &Url, deadline: Instant) -> bool {
        self.pacer.wait(&guard::host_key(url), deadline)
    }

    /// Halves the request rate to `url`'s host for the rest of the run, down
    /// to one request per [`MAX_PACE`]: what a 429 asks for.
    pub fn slow_down(&self, url: &Url) {
        self.pacer.slow_down(&guard::host_key(url));
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

    /// A response header's value, when it is text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner.headers().get(name).and_then(|v| v.to_str().ok())
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
///
/// The rule: a request is released only once a full interval has passed
/// since its host's last release, each release timed under the lock as it
/// is granted. A booked slot only orders the waiters; one that wakes to
/// find the last release later than it planned for, because it or another
/// waiter woke late, books again. No lock is held across a sleep.
struct Pacer {
    interval: Duration,
    hosts: Mutex<HashMap<String, Pace>>,
}

/// One host's pacing.
struct Pace {
    interval: Duration,
    /// The next slot no waiter has booked.
    next: Instant,
    /// When the last request to this host was released.
    last: Option<Instant>,
}

impl Pacer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    fn wait(&self, host: &str, deadline: Instant) -> bool {
        self.wait_on(host, deadline, Instant::now, std::thread::sleep)
    }

    /// [`Pacer::wait`] on a given clock, so a late wakeup can be simulated.
    fn wait_on(
        &self,
        host: &str,
        deadline: Instant,
        clock: impl Fn() -> Instant,
        mut sleep: impl FnMut(Duration),
    ) -> bool {
        let mut booked = None;
        loop {
            let slot = {
                let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
                let now = clock();
                let pace = self.pace(&mut hosts, host, now);
                let ready = pace.last.map_or(now, |last| last + pace.interval);
                let slot = match booked {
                    Some(slot) if slot > now || ready <= now => slot,
                    _ => {
                        let slot = pace.next.max(now).max(ready);
                        if slot >= deadline {
                            return false;
                        }
                        pace.next = slot + pace.interval;
                        slot
                    }
                };
                if slot <= now {
                    let go = now < deadline;
                    if go {
                        pace.last = Some(now);
                    }
                    return go;
                }
                slot
            };
            booked = Some(slot);
            sleep(slot.saturating_duration_since(clock()));
        }
    }

    fn slow_down(&self, host: &str) {
        let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        let pace = self.pace(&mut hosts, host, Instant::now());
        pace.interval = (pace.interval * 2).min(MAX_PACE.max(self.interval));
    }

    fn pace<'a>(
        &self,
        hosts: &'a mut HashMap<String, Pace>,
        host: &str,
        now: Instant,
    ) -> &'a mut Pace {
        hosts.entry(host.to_owned()).or_insert_with(|| Pace {
            interval: self.interval,
            next: now,
            last: None,
        })
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
        if all_public(addresses.iter()) {
            Ok(addresses)
        } else {
            Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                PrivateAddress,
            )))
        }
    }
}

/// The one rule for what a name resolved to, whoever connects: every
/// address public, so one inward address among public ones is refused.
fn all_public<'a>(addresses: impl IntoIterator<Item = &'a SocketAddr>) -> bool {
    addresses.into_iter().all(|address| is_public(address.ip()))
}

/// Opens a connection to `host` on `port` for a caller that speaks its own
/// protocol over it, the headless tier's relay, under the fetcher's rules
/// for where a connection may go: the name rule, then one resolution,
/// refused unless every address is public, then a connect to the first
/// checked address that answers before `deadline`. The checked address is
/// the one connected to, so a name is never resolved twice. `test_address`
/// stands in for every host, as it does for the fetcher.
pub fn connect_public(
    host: &str,
    port: u16,
    deadline: Instant,
    test_address: Option<SocketAddr>,
) -> Result<TcpStream, Refusal> {
    let bracketed = if host.parse::<Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let url =
        Url::parse(&format!("http://{bracketed}:{port}/")).map_err(|_| Refusal::InvalidUrl)?;
    if is_private_host(&url) {
        return Err(Refusal::PrivateNetwork { redirected: false });
    }
    let addresses = match test_address {
        Some(address) => vec![address],
        None => resolve_public(&url, port)?,
    };
    let mut failure = io::Error::from(io::ErrorKind::TimedOut);
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(stream) => return Ok(stream),
            Err(err) => failure = err,
        }
    }
    Err(Refusal::Failed(io_reason(&failure)))
}

/// The addresses of `url`'s host, refused unless every one is public.
fn resolve_public(url: &Url, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
    let not_found = || Refusal::Failed("host not found".to_owned());
    let addresses: Vec<SocketAddr> = match url.host() {
        Some(Host::Domain(name)) => (name, port)
            .to_socket_addrs()
            .map_err(|_| not_found())?
            .collect(),
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
        None => return Err(Refusal::InvalidUrl),
    };
    if addresses.is_empty() {
        return Err(not_found());
    }
    if !all_public(&addresses) {
        return Err(Refusal::PrivateAddress);
    }
    Ok(addresses)
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
pub fn test_address() -> Option<SocketAddr> {
    std::env::var("KNOWMORETABS_TEST_RESOLVE")
        .ok()?
        .parse()
        .ok()
}

#[cfg(not(debug_assertions))]
pub fn test_address() -> Option<SocketAddr> {
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
#[path = "fetch_tests.rs"]
mod tests;
