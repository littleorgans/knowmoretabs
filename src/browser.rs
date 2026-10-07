//! The installed browser, run headless to render the pages plain HTTP
//! read as thin or empty: found, started, asked for one page at a time,
//! and closed with everything it started. Or the owner's running browser,
//! attached to, opening the pages a signed in run reads in tabs of its own.
//!
//! slice: content
//! why: Some pages only show their text once their scripts run, so the
//!      headless tier renders them in the browser the owner already has,
//!      never with the owner's profile: a scratch profile, a private context
//!      per page so pages share no cookies, and every connection through the
//!      relay that keeps the private network out of reach. A page is loaded
//!      from the address its HTTP read ended at, checked and paced as a
//!      request is, given ten seconds to load, then read back once by a fixed
//!      expression; where it ended is checked again, and its status before
//!      its type, as for a response. A failure that may pass is retried by
//!      the same rules; a page that cannot be rendered says why. A browser
//!      that fails is told apart from a page that does, so its pages wait
//!      for a run that has one. Closing is ordered and bounded: the browser
//!      asked to close, its process group ended, the relay stopped, the
//!      scratch profile removed. The owner's own browser is never closed
//!      and its tabs are never touched: each page is a hidden tab this run
//!      creates and attaches to on the one socket the owner allowed, which
//!      the browser ends with that socket however the run ends, and a tab
//!      is the only thing that can speak to its page.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::capture::Log;
use crate::cdp::{self, Command, Socket};
use crate::content_fetch::{self, BODY_CAP, Capture, Passing};
use crate::content_signed_in::Unavailable;
use crate::content_store::{Line, Tier};
use crate::fetch::{self, Fetcher, Refusal};
use crate::guard;
use crate::platform;
use crate::process_tree::{self, Scratch, Tree};
use crate::socks::Relay;
use crate::tools::Probe;
use tab::Tab;

/// Features turned off at launch: the optimization guide, which plausibly
/// sends the hosts it sees, and preconnecting to the search engine.
const DISABLED_FEATURES: &str = "OptimizationHints,OptimizationHintsFetching,\
OptimizationGuideModelDownloading,OptimizationTargetPrediction,PreconnectToSearch";
/// How long a started browser has to say where its `DevTools` socket is.
const READY_WAIT: Duration = Duration::from_secs(10);
/// How long a page has to load, from its navigation, never extended.
const LOAD_WAIT: Duration = Duration::from_secs(10);
/// How long the expression has to read a loaded page back.
const READ_WAIT: Duration = Duration::from_secs(5);
/// How long one call on the browser's own socket may take.
const CALL_WAIT: Duration = Duration::from_secs(10);
/// How long the owner has to allow a signed in run's connection.
const ALLOW_WAIT: Duration = Duration::from_secs(60);
/// How long a closed browser has to exit before its group is ended.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How long the scratch profile is tried for: a crash reporter outside the
/// browser's group can hold it a moment after the browser exits.
const REMOVE_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
/// Navigation failures that may pass, as the browser names them.
const PASSING: [&str; 4] = [
    "ERR_TIMED_OUT",
    "ERR_CONNECTION_RESET",
    "ERR_CONNECTION_CLOSED",
    "ERR_CONNECTION_FAILED",
];

/// Whether pages can be rendered this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// The chosen browser's binary.
    Ready(PathBuf),
    /// Why there is none.
    Missing(String),
    /// The owner said `--no-browser`.
    Off,
}

impl Readiness {
    /// Browser `id`'s binary, looked for on disk and never run; `off` when
    /// the owner asked for no browser.
    pub fn check(probe: &impl Probe, id: &str, off: bool) -> Self {
        if off {
            return Self::Off;
        }
        if platform::browser(id).is_none() {
            return Self::Missing(format!("unknown browser {id:?}"));
        }
        probe.browser(id).map_or_else(
            || Self::Missing(format!("no {id} binary found")),
            Self::Ready,
        )
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Ready(path) => Some(path),
            _ => None,
        }
    }
}

/// The name a browser binary goes by in reports: `Google Chrome`.
pub fn name(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// What rendering a page came to.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Rendered and read: what its HTML says.
    Read(Box<Capture>),
    /// It ended on a sign-in screen; the line says where.
    SignIn(Box<Line>),
    /// Refused by the fetcher's rules, before it was opened or where it
    /// ended: the page keeps what it had.
    Refused(Refusal),
    /// Not rendered, and why: the page keeps what it had.
    NotRendered(String),
}

/// The browser itself failed; the pages left wait for another run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken(pub String);

type Rendered = Result<Outcome, Broken>;

/// What the page gave back by value.
#[derive(Debug, Deserialize)]
struct Read {
    url: String,
    mime: String,
    status: u16,
    html: Option<String>,
}

/// Why a tab ended without a page to read.
#[derive(Debug)]
struct Failed {
    reason: String,
    passing: bool,
}

impl Failed {
    fn stop(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            passing: false,
        }
    }
}

impl From<String> for Failed {
    fn from(reason: String) -> Self {
        let passing = content_fetch::is_passing(&reason);
        Self { reason, passing }
    }
}

/// A browser pages are rendered in: one this run launched, closed when
/// dropped, or the owner's, which dropping only lets go of.
#[derive(Debug)]
pub struct Browser {
    /// The browser's own socket, for contexts, tabs and closing; and when
    /// attached, every page's session too.
    socket: Mutex<Socket>,
    port: u16,
    mode: Mode,
}

/// How the browser is reached.
#[derive(Debug)]
enum Mode {
    /// Started headless by this run.
    Launched(Running),
    /// The owner's running browser, by name: never closed.
    Attached(String),
}

/// What a launch starts, ended in order when dropped.
#[derive(Debug)]
struct Running {
    name: String,
    tree: Tree,
    relay: Relay,
    scratch: Option<Scratch>,
    log: Log,
}

impl Browser {
    /// Starts the browser at `path` headless, on a scratch profile and the
    /// relay, and connects to it. Fails with one line of reason.
    pub fn launch(path: &Path, log: Log) -> Result<Self, String> {
        let name = name(path);
        let relay = Relay::start().map_err(|err| format!("no relay: {}", err.kind()))?;
        let scratch = process_tree::scratch("knowmoretabs-browser-")
            .map_err(|err| format!("no scratch profile: {}", err.kind()))?;
        let profile = scratch.path().to_owned();
        let tree = process_tree::spawn(
            Process::new(path)
                .args(flags(&profile, relay.port()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .map_err(|err| format!("{name} could not start: {}", err.kind()))?;
        let mut running = Running {
            name,
            tree,
            relay,
            scratch: Some(scratch),
            log,
        };
        let deadline = Instant::now() + READY_WAIT;
        let (port, route) = running.endpoint(&profile, deadline)?;
        let socket = Socket::connect(port, &route, deadline)
            .map_err(|why| format!("{} did not answer: {why}", running.name))?;
        Ok(Self {
            socket: Mutex::new(socket),
            port,
            mode: Mode::Launched(running),
        })
    }

    /// Attaches to the owner's running browser, `name`, whose `DevTools`
    /// socket is `route` on loopback `port`: connected and asked its
    /// version under one deadline, long enough for the owner to allow the
    /// connection. Nothing is started.
    pub fn attach(port: u16, route: &str, name: String) -> Result<Self, Unavailable> {
        let deadline = Instant::now() + ALLOW_WAIT;
        let stream = cdp::reach(port, deadline).map_err(|_| Unavailable::NotRunning)?;
        let mut socket =
            Socket::over(stream, route, deadline).map_err(|_| Unavailable::NotAllowed)?;
        socket
            .call(&Command::GetVersion, None, deadline)
            .map_err(|_| Unavailable::NotAllowed)?;
        Ok(Self {
            socket: Mutex::new(socket),
            port,
            mode: Mode::Attached(name),
        })
    }

    /// What the browser says it is: `HeadlessChrome/155.0.8059.39`.
    pub fn version(&self) -> Result<String, String> {
        let reply = self.call(&Command::GetVersion)?;
        Ok(reply["product"].as_str().unwrap_or("unknown").to_owned())
    }

    /// The tier its pages are recorded under.
    pub fn tier(&self) -> Tier {
        match self.mode {
            Mode::Launched(_) => Tier::Headless,
            Mode::Attached(_) => Tier::SignedIn,
        }
    }

    /// The connections the relay refused for this machine or the private
    /// network; the owner's browser has no relay.
    pub fn refused(&self) -> usize {
        match &self.mode {
            Mode::Launched(running) => running.relay.refused(),
            Mode::Attached(_) => 0,
        }
    }

    fn name(&self) -> &str {
        match &self.mode {
            Mode::Launched(running) => &running.name,
            Mode::Attached(name) => name,
        }
    }

    /// Renders page `raw` from `url`, the address its HTTP read ended at,
    /// and reads it, with `images` the images it names. Failures that may
    /// pass are tried again; a failure of the browser itself is returned
    /// at once.
    pub fn render(&self, fetcher: &Fetcher, raw: &str, url: &Url, images: bool) -> Rendered {
        content_fetch::retrying(|| self.once(fetcher, raw, url, images))
    }

    fn once(
        &self,
        fetcher: &Fetcher,
        raw: &str,
        url: &Url,
        images: bool,
    ) -> Result<Rendered, Passing<Rendered>> {
        if let Err(refusal) = fetcher.check(url, 0) {
            return Ok(Ok(Outcome::Refused(refusal)));
        }
        if !fetcher.pace(url, Instant::now() + fetch::TIMEOUT) {
            return Err(passing("timeout".to_owned()));
        }
        let mut tab = match Tab::open(self) {
            Ok(tab) => tab,
            Err(why) => return Ok(Err(Broken(why))),
        };
        let loaded = tab.load(url);
        let closed = tab.close();
        match (loaded, closed) {
            // A tab that failed in a browser that no longer answers: the
            // browser is what failed.
            (Err(_), Err(why)) => Ok(Err(Broken(why))),
            (Err(failed), Ok(())) if failed.passing => Err(passing(failed.reason)),
            (Err(failed), Ok(())) => Ok(Ok(Outcome::NotRendered(failed.reason))),
            (Ok(read), _) => after(fetcher, self.tier(), raw, url, read, images),
        }
    }

    fn call(&self, command: &Command) -> Result<Value, String> {
        self.socket()
            .call(command, None, Instant::now() + CALL_WAIT)
            .map_err(|why| format!("{} stopped answering: {why}", self.name()))
    }

    /// The browser's own socket.
    fn socket(&self) -> MutexGuard<'_, Socket> {
        self.socket.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Browser {
    /// A launched browser is asked to close and given a moment to exit;
    /// the owner's is only let go of, its socket closed.
    fn drop(&mut self) {
        if matches!(self.mode, Mode::Attached(_)) {
            return;
        }
        let _ = self.call(&Command::Close);
        let Mode::Launched(running) = &mut self.mode else {
            return;
        };
        let deadline = Instant::now() + CLOSE_WAIT;
        while Instant::now() < deadline {
            match running.tree.try_wait() {
                Ok(None) => std::thread::sleep(POLL),
                Ok(Some(_)) | Err(_) => break,
            }
        }
    }
}

impl Running {
    /// The port and route of the browser's `DevTools` socket, from the file
    /// it writes in its profile once it listens.
    fn endpoint(&mut self, profile: &Path, deadline: Instant) -> Result<(u16, String), String> {
        let file = profile.join(PORT_FILE);
        loop {
            if let Some(endpoint) = fs::read_to_string(&file).ok().and_then(|t| endpoint(&t)) {
                return Ok(endpoint);
            }
            if let Ok(Some(status)) = self.tree.try_wait() {
                return Err(format!(
                    "{} exited before it was ready ({status})",
                    self.name
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} was not ready after {} s",
                    self.name,
                    READY_WAIT.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.tree.retire();
        let open = self.relay.stop();
        if open > 0 {
            self.log.warn(&format!(
                "{open} relay connections were still open after {} closed",
                self.name
            ));
        }
        let Some(scratch) = self.scratch.take() else {
            return;
        };
        let path = scratch.path().to_owned();
        let deadline = Instant::now() + REMOVE_WAIT;
        while fs::remove_dir_all(&path).is_err() && path.exists() && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        drop(scratch);
        if path.exists() {
            self.log.warn(&format!(
                "could not remove {}'s scratch profile at {}",
                self.name,
                path.display()
            ));
        }
    }
}

/// How the browser is started: headless, on `profile`, never the owner's
/// own, every connection through the relay on `relay`, loopback included,
/// WebRTC kept off UDP outside the proxy, and the features that call home
/// about pages off.
fn flags(profile: &Path, relay: u16) -> Vec<OsString> {
    let mut user_data = OsString::from("--user-data-dir=");
    user_data.push(profile);
    vec![
        "--headless".into(),
        "--remote-debugging-port=0".into(),
        user_data,
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--use-mock-keychain".into(),
        "--disable-background-networking".into(),
        format!("--proxy-server=socks5://127.0.0.1:{relay}").into(),
        "--proxy-bypass-list=<-loopback>".into(),
        "--webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        format!("--disable-features={DISABLED_FEATURES}").into(),
        "about:blank".into(),
    ]
}

/// The file a browser with remote debugging on writes in its user-data
/// directory while it listens.
pub(crate) const PORT_FILE: &str = "DevToolsActivePort";

/// `DevToolsActivePort`: a nonzero port, then the browser's socket route.
pub(crate) fn endpoint(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok().filter(|&p| p != 0)?;
    let route = lines.next()?.trim();
    route
        .starts_with("/devtools/browser/")
        .then(|| (port, route.to_owned()))
}

/// How far the navigated frame's current document has loaded: the
/// navigation's own document, until a new one starts in that frame.
#[derive(Debug)]
struct Lifecycle {
    frame: String,
    loader: String,
    load: bool,
    idle: bool,
}

impl Lifecycle {
    fn new(frame: String, loader: String) -> Self {
        Self {
            frame,
            loader,
            load: false,
            idle: false,
        }
    }

    fn see(&mut self, event: &cdp::Lifecycle) {
        if event.frame != self.frame {
            return;
        }
        if event.name == "init" && event.loader != self.loader {
            event.loader.clone_into(&mut self.loader);
            self.load = false;
            self.idle = false;
        } else if event.loader == self.loader {
            match event.name.as_str() {
                "load" => self.load = true,
                "networkIdle" => self.idle = true,
                _ => {}
            }
        }
    }

    /// Hears events from `next` until the document has loaded and the
    /// network is idle, or `deadline` passes, one deadline whatever the
    /// page does; whether it loaded.
    fn wait(
        &mut self,
        mut next: impl FnMut(Instant) -> Result<Option<cdp::Lifecycle>, String>,
        deadline: Instant,
    ) -> Result<bool, String> {
        while !(self.load && self.idle) {
            match next(deadline)? {
                Some(event) => self.see(&event),
                None => break,
            }
        }
        Ok(self.load)
    }
}

/// A string field of a reply, empty when absent.
fn text(reply: &Value, key: &str) -> String {
    reply[key].as_str().unwrap_or_default().to_owned()
}

/// A failure worth another try, which ends the page unrendered if it
/// keeps failing.
fn passing(reason: String) -> Passing<Rendered> {
    Passing::after(Ok(Outcome::NotRendered(reason)), None)
}

/// What a page read back says, in the order a response is read: where it
/// ended, its status, its type, its size, then its HTML, which `tier` read.
fn after(
    fetcher: &Fetcher,
    tier: Tier,
    raw: &str,
    navigated: &Url,
    read: Read,
    images: bool,
) -> Result<Rendered, Passing<Rendered>> {
    let not_rendered = |why: String| Ok(Ok(Outcome::NotRendered(why)));
    let Some(url) = guard::page_url(&read.url) else {
        return not_rendered(Refusal::InvalidUrl.reason());
    };
    let hop = usize::from(url != *navigated);
    match fetcher.check(&url, hop) {
        Ok(()) => {}
        // A sign-in screen, said as a request that met one says it.
        Err(login @ Refusal::Login(_)) => {
            let reason = login.reason();
            return match content_fetch::refused(raw, tier, login) {
                Ok(capture) => Ok(Ok(Outcome::SignIn(Box::new(capture.line)))),
                Err(_) => not_rendered(reason),
            };
        }
        Err(refusal) => return Ok(Ok(Outcome::Refused(refusal))),
    }
    if read.status == 0 {
        return not_rendered("no HTTP status".to_owned());
    }
    match content_fetch::status_outcome(read.status) {
        None => {}
        Some((_, true)) => {
            if read.status == 429 {
                fetcher.slow_down(&url);
            }
            return Err(passing(format!("HTTP {}", read.status)));
        }
        Some(_) => return not_rendered(format!("HTTP {}", read.status)),
    }
    if !read.mime.contains("html") {
        return not_rendered(format!("not HTML ({})", read.mime));
    }
    let Some(html) = read.html.filter(|html| html.len() <= BODY_CAP) else {
        return not_rendered("page too large".to_owned());
    };
    Ok(Ok(Outcome::Read(Box::new(content_fetch::from_html(
        raw,
        tier,
        &url,
        read.status,
        &html,
        images,
    )))))
}

#[path = "browser_tab.rs"]
mod tab;

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
