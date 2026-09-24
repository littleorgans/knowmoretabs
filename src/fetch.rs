//! The only network code in knowmoretabs: one cookieless GET per page, read
//! until `</head>`, turned into a metadata record.
//!
//! slice: library
//! why: `enrich` is opt-in because it sends the URLs you visited to their
//!      own sites, so what it sends is kept to the minimum and every hop is
//!      checked. No cookies, no referrer, no proxy from the environment; each
//!      redirect is followed here, not by the client, so that a hop to this
//!      machine, the private network or a login screen is refused before it
//!      is requested; and every name is resolved through a resolver that
//!      refuses private addresses, so a public name pointing inward is caught
//!      at the one moment it matters, the connect.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::{Host, Url};

use crate::github;
use crate::head::{self, Head};
use crate::local;
use crate::metadata_writer::{Line, Outcome};

/// `YouTube`'s meta tags start about 0.7 MB into its pages.
pub const HEAD_CAP: usize = 3 * 1024 * 1024;
/// A repository page is read whole: its README is in the body.
pub const REPO_PAGE_CAP: usize = 4 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 10;
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// One request per second to any one host.
pub const PACE: Duration = Duration::from_secs(1);

/// Says what is asking, in the form sites already know how to read.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; knowmoretabs/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/littleorgans/knowmoretabs)"
);
const ACCEPT: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5";
/// Longest meta value kept; a description is a sentence or two.
const VALUE_CAP: usize = 2000;

pub struct Fetcher {
    agent: ureq::Agent,
    pacer: Pacer,
    forgotten: BTreeSet<String>,
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
            .accept(ACCEPT)
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
        }
    }

    /// Fetches one page and says what came of it. Never fails: a failure is
    /// a record too.
    pub fn fetch(&self, raw: &str) -> Line {
        let Ok(mut url) = Url::parse(raw) else {
            return Line::new(raw, Outcome::Error).with_reason("not a valid URL");
        };
        url.set_fragment(None);
        for hop in 0..=MAX_REDIRECTS {
            if !is_web(&url) {
                return Line::new(raw, Outcome::Skipped)
                    .with_reason("redirected away from the web");
            }
            if is_private_host(&url) {
                return Line::new(raw, Outcome::Skipped).with_reason(if hop == 0 {
                    "private network"
                } else {
                    "redirected to a private network"
                });
            }
            if self.forgotten.contains(url.as_str()) {
                return Line::new(raw, Outcome::Skipped).with_reason("forgotten page, not fetched");
            }
            if carries_token(&url) || is_search_results(&url) {
                return Line::new(raw, Outcome::Skipped)
                    .with_reason("token or search URL, not fetched");
            }
            if is_login_page(&url) || (hop > 0 && is_login_redirect(&url)) {
                let mut record =
                    Line::new(raw, Outcome::BehindLogin).with_reason("redirected to a login page");
                record.final_url = Some(url.to_string());
                return record;
            }
            self.pacer.wait(
                &url.host_str()
                    .unwrap_or("")
                    .trim_end_matches('.')
                    .to_ascii_lowercase(),
            );
            let response = match self.agent.get(url.as_str()).call() {
                Ok(response) => response,
                Err(err) => return failure(raw, &err),
            };
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok());
            if let (300..=399, Some(location)) = (status, location) {
                match url.join(location.trim()) {
                    Ok(next) => {
                        url = next;
                        url.set_fragment(None);
                        continue;
                    }
                    Err(_) => {
                        return Line::new(raw, Outcome::Error)
                            .with_reason(format!("HTTP {status} to an invalid URL"));
                    }
                }
            }
            return read_page(raw, &url, response);
        }
        Line::new(raw, Outcome::Error).with_reason("too many redirects")
    }
}

fn read_page(raw: &str, url: &Url, mut response: ureq::http::Response<ureq::Body>) -> Line {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let with_response = |mut record: Line| {
        record.final_url = Some(url.to_string());
        record.http_status = Some(status);
        record
    };
    if status == 401 {
        return with_response(Line::new(raw, Outcome::BehindLogin).with_reason("HTTP 401"));
    }
    if !(200..300).contains(&status) {
        return with_response(Line::new(raw, Outcome::Error).with_reason(format!("HTTP {status}")));
    }
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !mime.is_empty() && !mime.contains("html") {
        return with_response(
            Line::new(raw, Outcome::Skipped).with_reason(format!("not HTML ({mime})")),
        );
    }
    let repo = github::is_repo_page(url);
    let cap = if repo { REPO_PAGE_CAP } else { HEAD_CAP };
    let bytes = match read_head(response.body_mut().as_reader(), cap, !repo) {
        Ok(bytes) => bytes,
        Err(err) => {
            return with_response(Line::new(raw, Outcome::Error).with_reason(io_reason(&err)));
        }
    };
    let text = match head::decode(&bytes, head::charset_param(&content_type).as_deref()) {
        Ok(text) => text,
        Err(label) => {
            return with_response(
                Line::new(raw, Outcome::Error).with_reason(format!("unsupported charset {label}")),
            );
        }
    };
    let found = head::scan(&text);
    if is_sign_in_page(&found) {
        let mut record =
            with_response(Line::new(raw, Outcome::BehindLogin).with_reason("sign-in page"));
        record.title.clone_from(&found.title);
        return record;
    }
    let mut record = with_response(Line::new(raw, Outcome::Ok));
    fill(&mut record, &found, url);
    if repo {
        record.github = github::repo_data(&text);
    }
    let empty = record.title.is_none()
        && record.description.is_none()
        && record.og.is_empty()
        && record.twitter.is_empty()
        && record.github.is_none();
    if empty && bytes.len() >= cap {
        return with_response(
            Line::new(raw, Outcome::Error).with_reason("no </head> in the first 3 MB"),
        );
    }
    record
}

/// Reads until the head ends (when `stop_at_head`), the cap, or the end.
fn read_head(mut body: impl Read, cap: usize, stop_at_head: bool) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    while bytes.len() < cap {
        let want = chunk.len().min(cap - bytes.len());
        let n = match body.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        // Search only what is new, plus room for a tag split across reads.
        let from = bytes.len().saturating_sub(8);
        bytes.extend_from_slice(&chunk[..n]);
        if stop_at_head && head::end_of_head(&bytes[from..]).is_some() {
            break;
        }
    }
    Ok(bytes)
}

fn fill(record: &mut Line, found: &Head, url: &Url) {
    let cap = |value: &str| value.chars().take(VALUE_CAP).collect::<String>();
    record.title = found.title.as_deref().map(cap);
    record.description = found.meta("description").map(|d| cap(&head::collapse(d)));
    record.lang.clone_from(&found.lang);
    record.canonical = found
        .canonical
        .as_deref()
        .and_then(|href| url.join(href).ok())
        .map(|canonical| canonical.to_string());
    for (key, value) in &found.meta {
        let (map, name) = if let Some(name) = key.strip_prefix("og:") {
            (&mut record.og, name)
        } else if let Some(name) = key.strip_prefix("twitter:") {
            (&mut record.twitter, name)
        } else {
            continue;
        };
        if !name.is_empty() && map.len() < 32 {
            map.entry(name.to_owned())
                .or_insert_with(|| cap(&head::collapse(value)));
        }
    }
    record.jsonld_types = jsonld_types(&found.jsonld);
}

/// Every `@type` in the page's JSON-LD, `@graph` and nesting included, in
/// the order met and without the schema.org prefix.
fn jsonld_types(blocks: &[String]) -> Vec<String> {
    fn walk(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                let types = match map.get("@type") {
                    Some(serde_json::Value::Array(items)) => items.iter().collect(),
                    Some(one) => vec![one],
                    None => Vec::new(),
                };
                for name in types.into_iter().filter_map(serde_json::Value::as_str) {
                    let name = ["https://schema.org/", "http://schema.org/"]
                        .iter()
                        .fold(name.trim(), |n, prefix| n.strip_prefix(prefix).unwrap_or(n));
                    if !name.is_empty() && found.len() < 16 && !found.iter().any(|f| f == name) {
                        found.push(name.to_owned());
                    }
                }
                map.values().for_each(|v| walk(v, found));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for block in blocks {
        // Some pages wrap the JSON in an HTML comment or CDATA.
        let text = block
            .trim()
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim_start_matches("//<![CDATA[")
            .trim_end_matches("//]]>");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            walk(&value, &mut found);
        }
    }
    found
}

/// A page that is only a sign-in form: a title that says so and nothing
/// that describes the page itself.
fn is_sign_in_page(found: &Head) -> bool {
    let Some(title) = &found.title else {
        return false;
    };
    let title = title.to_lowercase();
    let says_sign_in = ["sign in", "signin", "sign-in", "log in", "login", "log-in"]
        .iter()
        .any(|phrase| has_word(&title, phrase));
    says_sign_in && found.meta("description").is_none() && found.meta("og:description").is_none()
}

fn has_word(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn failure(raw: &str, err: &ureq::Error) -> Line {
    let (status, reason) = match err {
        ureq::Error::Io(io)
            if io
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<PrivateAddress>) =>
        {
            (Outcome::Skipped, "private network address".to_owned())
        }
        ureq::Error::Timeout(_) => (Outcome::Error, "timeout".to_owned()),
        ureq::Error::HostNotFound => (Outcome::Error, "host not found".to_owned()),
        ureq::Error::ConnectionFailed => (Outcome::Error, "connection failed".to_owned()),
        ureq::Error::Io(io) => (Outcome::Error, io_reason(io)),
        other => (
            Outcome::Error,
            other.to_string().chars().take(160).collect(),
        ),
    };
    Line::new(raw, status).with_reason(reason)
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

pub fn is_web(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host().is_some()
}

/// This machine or the private network, judged from the URL alone: an
/// address literal outside public space, a name that only means something
/// on a local network, or a bare name that a search domain would complete.
pub fn is_private_host(url: &Url) -> bool {
    match url.host() {
        None => true,
        Some(Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        Some(host @ Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            local::is_this_machine(&host)
                || !name.contains('.')
                || [
                    ".local",
                    ".localdomain",
                    ".internal",
                    ".intranet",
                    ".lan",
                    ".home",
                    ".home.arpa",
                    ".corp",
                ]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        }
    }
}

/// Globally routable. Everything else is this machine, the private network
/// or a range nothing public lives in.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let [first, second, ..] = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Only native global unicast; exclude local, reserved and
                // transition ranges that can embed a private IPv4 target.
                || (first & 0xe000) != 0x2000
                || first == 0x2002
                || (first == 0x2001 && second == 0)
                || (first == 0x2001 && second == 0x0db8)
                || (first == 0x0064 && second == 0xff9b))
        }
    }
}

/// A results page says nothing the query in its URL does not.
pub fn is_search_results(url: &Url) -> bool {
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = decoded_path(url).to_ascii_lowercase();
    let has = |key: &str| url.query_pairs().any(|(k, v)| k == key && !v.is_empty());
    let engine = |name: &str| {
        host == name || host.starts_with(&format!("{name}.")) || host.ends_with(&format!(".{name}"))
    };
    if engine("google") && ["/search", "/url", "/imgres", "/webhp"].contains(&path.as_str()) {
        return true;
    }
    if (engine("duckduckgo")
        && (has("q") || path.starts_with("/html") || path.starts_with("/lite")))
        || (engine("baidu") && path == "/s")
        || (engine("amazon") && path == "/s")
    {
        return true;
    }
    // Most sites put their own search at `/search` or `/results` with the
    // query in a parameter: Bing, Kagi, Brave, GitHub, YouTube, Reddit ...
    path.split('/')
        .any(|segment| matches!(segment.to_ascii_lowercase().as_str(), "search" | "results"))
        && url.query().is_some_and(|q| !q.is_empty())
}

/// Query parameter names that carry a secret or a one-time value. A GET can
/// spend a one-time link, and the value is nobody else's business anyway.
const TOKEN_WORDS: &[&str] = &[
    "token",
    "code",
    "key",
    "apikey",
    "secret",
    "sig",
    "signature",
    "auth",
    "session",
    "sessionid",
    "sid",
    "otp",
    "reset",
    "verify",
    "verification",
    "confirm",
    "confirmation",
    "magic",
    "nonce",
    "state",
    "password",
    "pwd",
    "ticket",
    "jwt",
    "credential",
    "credentials",
];

/// A query parameter whose name says it carries a secret, whose value is a
/// JSON Web Token, or credentials in the URL itself.
pub fn carries_token(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        let words_match = key
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| TOKEN_WORDS.contains(&word));
        words_match
            || ["token", "secret", "password", "signature"]
                .iter()
                .any(|word| key.contains(word))
            || (value.starts_with("eyJ") && value.len() > 30)
    })
}

/// Path segments that name a sign-in, sign-up or verification screen on
/// their own. A GET of a verification link can spend it, so these pages are
/// never fetched.
const LOGIN_SEGMENTS: &[&str] = &[
    "login",
    "log-in",
    "log_in",
    "logon",
    "signin",
    "sign-in",
    "sign_in",
    "signup",
    "sign-up",
    "sign_up",
    "register",
    "sso",
    "saml",
    "oauth",
    "oauth2",
    "authorize",
    "verify",
    "verify-email",
    "confirm-email",
    "reset-password",
    "password-reset",
    "forgot-password",
    "magic-link",
    "2fa",
    "mfa",
    "otp",
    "servicelogin",
];
/// Weaker words that mean a login when a page redirects to them.
const REDIRECT_LOGIN_SEGMENTS: &[&str] = &["auth", "account", "accounts", "session", "sessions"];
const LOGIN_HOST_LABELS: &[&str] = &["login", "signin", "accounts", "auth", "sso", "idp"];

/// A sign-in, sign-up or verification screen in its own right.
pub fn is_login_page(url: &Url) -> bool {
    login_shaped(url, LOGIN_SEGMENTS)
}

fn is_login_redirect(url: &Url) -> bool {
    login_shaped(url, LOGIN_SEGMENTS) || login_shaped(url, REDIRECT_LOGIN_SEGMENTS)
}

fn decoded_path(url: &Url) -> std::borrow::Cow<'_, str> {
    percent_encoding::percent_decode_str(url.path()).decode_utf8_lossy()
}

fn login_shaped(url: &Url, words: &[&str]) -> bool {
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let first_label = host.split('.').next().unwrap_or("");
    if host.contains('.') && LOGIN_HOST_LABELS.contains(&first_label) {
        return true;
    }
    decoded_path(url).split('/').any(|segment| {
        let segment = segment.to_ascii_lowercase();
        let stem = segment.split('.').next().unwrap_or("");
        words.contains(&stem)
    })
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

    fn wait(&self, host: &str) {
        let slot = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let slot = next.get(host).map_or(now, |at| (*at).max(now));
            next.insert(host.to_owned(), slot + self.interval);
            slot
        };
        std::thread::sleep(slot.saturating_duration_since(Instant::now()));
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
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    use std::sync::Arc;

    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn private_hosts_are_known_from_the_url() {
        for raw in [
            "http://192.168.1.1/",
            "http://10.0.0.8:8080/",
            "http://172.16.4.4/",
            "http://100.64.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://127.0.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fec0::1]/",
            "http://[feff::1]/",
            "http://[2001::7f00:1]/",
            "http://[::127.0.0.1]/",
            "http://[2002:7f00:1::]/",
            "http://0.1.2.3/",
            "http://2130706433/",
            "http://0177.0.0.1/",
            "http://127.0.0.1./",
            "http://224.0.0.1/",
            "http://255.255.255.255/",
            "http://[ff02::1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[fe80::1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://0x7f.1/",
            "http://printer.local/",
            "http://nas.home.arpa/",
            "http://build.internal/",
            "http://intranet/",
            "http://app.localhost/",
        ] {
            assert!(is_private_host(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
            "https://local.example.com/",
            "https://localhost.example.test/",
        ] {
            assert!(!is_private_host(&url(raw)), "{raw}");
        }
        assert!(is_public(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_public(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn login_screens_are_known_by_their_own_url_and_redirects_by_more() {
        for raw in [
            "https://example.com/login",
            "https://example.com/users/sign_in",
            "https://example.com/accounts/login/?next=/x",
            "https://example.com/login.php",
            "https://example.com/oauth/authorize?client_id=x",
            "https://example.com/verify-email/abc",
            "https://accounts.example.com/ServiceLogin",
            "https://login.example.com/",
        ] {
            assert!(is_login_page(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/blog/how-login-works",
            "https://github.com/owner/auth",
            "https://example.com/account",
            "https://login/",
        ] {
            assert!(!is_login_page(&url(raw)), "{raw}");
        }
        assert!(is_login_redirect(&url(
            "https://example.com/account?return=/"
        )));
        assert!(is_login_redirect(&url("https://example.com/auth/start")));
        assert!(!is_login_redirect(&url("https://example.com/docs/")));
    }

    #[test]
    fn a_sign_in_title_without_a_description_is_a_sign_in_page() {
        let page = |html: &str| is_sign_in_page(&head::scan(html));
        assert!(page("<title>Sign in - Example</title>"));
        assert!(page("<title>Login | App</title>"));
        assert!(!page(
            "<title>Sign in</title><meta name=description content=\"A real page\">"
        ));
        assert!(!page(
            "<title>Why login forms fail</title><meta property=og:description content=x>"
        ));
        assert!(!page("<title>Blogin' about loginess</title>"));
    }

    #[test]
    fn json_ld_types_are_collected_through_graphs_and_arrays() {
        let blocks = vec![
            r#"{"@context":"https://schema.org","@graph":[{"@type":"WebPage"},{"@type":["Article","https://schema.org/NewsArticle"],"author":{"@type":"Person"}}]}"#.to_owned(),
            "<!--{\"@type\":\"WebPage\"}-->".to_owned(),
            "not json".to_owned(),
        ];
        assert_eq!(
            jsonld_types(&blocks),
            ["WebPage", "Article", "NewsArticle", "Person"]
        );
    }

    #[test]
    fn the_pacer_spaces_one_host_and_leaves_others_alone() {
        let pacer = Arc::new(Pacer::new(Duration::from_millis(200)));
        let start = Instant::now();
        pacer.wait("a.test");
        pacer.wait("b.test");
        assert!(start.elapsed() < Duration::from_millis(150));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let pacer = Arc::clone(&pacer);
                std::thread::spawn(move || pacer.wait("a.test"))
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
    fn a_server_that_never_answers_is_a_timeout() {
        let fetcher = Fetcher::with(Duration::from_millis(300), Some(silent_server()), PACE);
        let start = Instant::now();
        let record = fetcher.fetch("http://slow.test/");
        assert_eq!(record.status, Outcome::Error);
        assert_eq!(record.reason.as_deref(), Some("timeout"));
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
        let record = fetcher.fetch("http://stall.test/");
        assert_eq!(
            (record.status, record.reason.as_deref()),
            (Outcome::Error, Some("timeout"))
        );
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
        assert_eq!(failure("http://x.test/", &err).status, Outcome::Skipped);
        assert_eq!(
            fetcher.fetch("http://localhost:9/").reason.as_deref(),
            Some("private network")
        );
    }
}
