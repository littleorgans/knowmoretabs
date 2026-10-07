//! The installed browser, run headless to render the pages plain HTTP
//! read as thin or empty: found, started, asked for one page at a time,
//! and closed with everything it started.
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
//!      scratch profile removed.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::capture::Log;
use crate::cdp::{self, Command, Socket};
use crate::content_fetch::{self, BODY_CAP, Capture, Passing};
use crate::content_store::{Line, Tier};
use crate::fetch::{self, Fetcher, Refusal};
use crate::guard;
use crate::platform;
use crate::process_tree::{self, Scratch, Tree};
use crate::socks::Relay;
use crate::tools::Probe;

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

/// A running headless browser. Dropping it closes it.
#[derive(Debug)]
pub struct Browser {
    /// The browser's own socket, for contexts, tabs and closing.
    socket: Mutex<Socket>,
    port: u16,
    running: Running,
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
            running,
        })
    }

    /// What the browser says it is: `HeadlessChrome/155.0.8059.39`.
    pub fn version(&self) -> Result<String, String> {
        let reply = self.call(&Command::GetVersion)?;
        Ok(reply["product"].as_str().unwrap_or("unknown").to_owned())
    }

    /// The connections the relay refused for this machine or the private
    /// network.
    pub fn refused(&self) -> usize {
        self.running.relay.refused()
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
            return Ok(Ok(Outcome::NotRendered(refusal.reason())));
        }
        if !fetcher.pace(url, Instant::now() + fetch::TIMEOUT) {
            return Err(passing("timeout".to_owned()));
        }
        let context = match self.call(&Command::CreateBrowserContext) {
            Ok(reply) => text(&reply, "browserContextId"),
            Err(why) => return Ok(Err(Broken(why))),
        };
        let dispose = Command::DisposeBrowserContext {
            context: context.clone(),
        };
        let target = match self.call(&Command::CreateTarget { context }) {
            Ok(reply) => text(&reply, "targetId"),
            Err(why) => {
                let _ = self.call(&dispose);
                return Ok(Err(Broken(why)));
            }
        };
        let loaded = self.load(&target, url);
        let closed = self
            .call(&Command::CloseTarget { target })
            .and_then(|_| self.call(&dispose));
        match (loaded, closed) {
            // A tab that failed in a browser that no longer answers: the
            // browser is what failed.
            (Err(_), Err(why)) => Ok(Err(Broken(why))),
            (Err(failed), Ok(_)) if failed.passing => Err(passing(failed.reason)),
            (Err(failed), Ok(_)) => Ok(Ok(Outcome::NotRendered(failed.reason))),
            (Ok(read), _) => after(fetcher, raw, url, read, images),
        }
    }

    /// Opens the tab's own socket, loads `url` in it and reads the page
    /// back once it has loaded and the network is idle, or at the deadline
    /// when it has loaded but never gone idle.
    fn load(&self, target: &str, url: &Url) -> Result<Read, Failed> {
        let deadline = Instant::now() + LOAD_WAIT;
        let mut page = Socket::connect(self.port, &format!("/devtools/page/{target}"), deadline)?;
        page.call(&Command::PageEnable, deadline)?;
        page.call(&Command::LifecycleEvents, deadline)?;
        // What the blank tab reported is not the page's.
        page.forget_events();
        let deadline = Instant::now() + LOAD_WAIT;
        let navigated = page.call(
            &Command::Navigate {
                url: url.to_string(),
            },
            deadline,
        )?;
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            let name = error.trim_start_matches("net::");
            return Err(Failed {
                reason: name.to_owned(),
                passing: PASSING.contains(&name),
            });
        }
        let mut lifecycle =
            Lifecycle::new(text(&navigated, "frameId"), text(&navigated, "loaderId"));
        if !lifecycle.wait(|until| page.event(until), deadline)? {
            return Err(Failed::from("timeout".to_owned()));
        }
        let reply = page.call(&Command::Extract, Instant::now() + READ_WAIT)?;
        if reply.get("exceptionDetails").is_some() {
            return Err(Failed::stop("the page could not be read"));
        }
        serde_json::from_value(reply["result"]["value"].clone())
            .map_err(|_| Failed::stop("the page could not be read"))
    }

    fn call(&self, command: &Command) -> Result<Value, String> {
        self.socket
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .call(command, Instant::now() + CALL_WAIT)
            .map_err(|why| format!("{} stopped answering: {why}", self.running.name))
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.call(&Command::Close);
        let deadline = Instant::now() + CLOSE_WAIT;
        while Instant::now() < deadline {
            match self.running.tree.try_wait() {
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
        let file = profile.join("DevToolsActivePort");
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

/// `DevToolsActivePort`: a nonzero port, then the browser's socket route.
fn endpoint(text: &str) -> Option<(u16, String)> {
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
/// ended, its status, its type, its size, then its HTML.
fn after(
    fetcher: &Fetcher,
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
            return match content_fetch::refused(raw, Tier::Headless, login) {
                Ok(capture) => Ok(Ok(Outcome::SignIn(Box::new(capture.line)))),
                Err(_) => not_rendered(reason),
            };
        }
        Err(refusal) => return not_rendered(refusal.reason()),
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
        Tier::Headless,
        &url,
        read.status,
        &html,
        images,
    )))))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::content_store::Status;
    use crate::tools::Table;

    #[test]
    fn readiness_is_the_chosen_binary_unless_the_owner_said_no() {
        let mut table = Table::default();
        table
            .browsers
            .insert(platform::CHROME, PathBuf::from("/opt/Google Chrome"));
        assert_eq!(
            Readiness::check(&table, platform::CHROME, false),
            Readiness::Ready(PathBuf::from("/opt/Google Chrome"))
        );
        assert_eq!(
            Readiness::check(&table, platform::CHROME, true),
            Readiness::Off
        );
        assert_eq!(
            Readiness::check(&table, platform::BRAVE, false),
            Readiness::Missing("no brave binary found".to_owned())
        );
        assert_eq!(
            Readiness::check(&table, "arc", false),
            Readiness::Missing("unknown browser \"arc\"".to_owned())
        );
        assert_eq!(name(Path::new("/opt/Google Chrome")), "Google Chrome");
    }

    #[test]
    fn the_launch_uses_a_scratch_profile_the_relay_and_no_udp_around_it() {
        let flags: Vec<String> = flags(Path::new("/scratch/knowmoretabs-browser-x"), 4321)
            .into_iter()
            .map(|flag| flag.into_string().unwrap())
            .collect();
        for expected in [
            "--headless",
            "--remote-debugging-port=0",
            "--user-data-dir=/scratch/knowmoretabs-browser-x",
            "--disable-background-networking",
            "--proxy-server=socks5://127.0.0.1:4321",
            "--proxy-bypass-list=<-loopback>",
            "--webrtc-ip-handling-policy=disable_non_proxied_udp",
        ] {
            assert!(flags.iter().any(|flag| flag == expected), "{expected}");
        }
        let features = flags
            .iter()
            .find_map(|flag| flag.strip_prefix("--disable-features="))
            .unwrap();
        assert!(features.split(',').any(|f| f == "PreconnectToSearch"));
        assert!(features.split(',').any(|f| f == "OptimizationHints"));
        assert!(
            !flags.iter().any(|flag| flag.starts_with("--profile")),
            "never the owner's profile"
        );
        assert_eq!(flags.last().map(String::as_str), Some("about:blank"));
    }

    #[test]
    fn the_endpoint_file_needs_a_port_and_the_browser_route() {
        assert_eq!(
            endpoint("9222\n/devtools/browser/abc-123\n"),
            Some((9222, "/devtools/browser/abc-123".to_owned()))
        );
        for bad in [
            "",
            "9222\n",
            "0\n/devtools/browser/x",
            "70000\n/devtools/browser/x",
            "9222\n/devtools/page/x",
        ] {
            assert_eq!(endpoint(bad), None, "{bad:?}");
        }
    }

    fn event(frame: &str, loader: &str, name: &str) -> cdp::Lifecycle {
        cdp::Lifecycle {
            frame: frame.to_owned(),
            loader: loader.to_owned(),
            name: name.to_owned(),
        }
    }

    /// Waits on `events` in order, then on nothing: the deadline. Returns
    /// whether the page loaded, and every deadline asked for.
    fn heard(events: Vec<cdp::Lifecycle>) -> (bool, Vec<Instant>) {
        let mut lifecycle = Lifecycle::new("F".to_owned(), "L1".to_owned());
        let mut events = events.into_iter();
        let mut asked = Vec::new();
        let deadline = Instant::now() + LOAD_WAIT;
        let loaded = lifecycle
            .wait(
                |until| {
                    asked.push(until);
                    Ok(events.next())
                },
                deadline,
            )
            .unwrap();
        assert!(asked.iter().all(|&until| until == deadline), "never reset");
        (loaded, asked)
    }

    #[test]
    fn loading_is_heard_for_the_navigations_frame_and_document_only() {
        let (loaded, asked) = heard(vec![
            event("F", "BLANK", "networkIdle"),
            event("CHILD", "L1", "load"),
            event("F", "L1", "load"),
            event("F", "L1", "networkIdle"),
            event("F", "L1", "unreached"),
        ]);
        assert!(loaded);
        assert_eq!(asked.len(), 4, "done at load and idle");

        let (loaded, _) = heard(vec![
            event("F", "L1", "load"),
            event("F", "L2", "init"),
            event("F", "L1", "networkIdle"),
        ]);
        assert!(!loaded, "a script navigation starts a new document");

        let (loaded, asked) = heard(vec![event("F", "L1", "load")]);
        assert!(loaded, "loaded but never idle is read at the deadline");
        assert_eq!(asked.len(), 2);
    }

    fn read(url: &str, mime: &str, status: u16, html: Option<String>) -> Read {
        Read {
            url: url.to_owned(),
            mime: mime.to_owned(),
            status,
            html,
        }
    }

    /// Why a page read back as `read` is not rendered, `passing` when it
    /// would be tried again, or the status it was read with.
    fn verdict(read: Read) -> String {
        let fetcher = Fetcher::new(&BTreeSet::new());
        let navigated = Url::parse("https://a.test/page").unwrap();
        match after(&fetcher, "https://a.test/page", &navigated, read, false) {
            Ok(Ok(Outcome::NotRendered(why))) => why,
            Ok(Ok(Outcome::Read(capture))) => format!("read {:?}", capture.line.status),
            Ok(Ok(Outcome::SignIn(line))) => format!("sign-in {:?}", line.status),
            Ok(Err(Broken(why))) => format!("broken {why}"),
            Err(_) => "passing".to_owned(),
        }
    }

    #[test]
    fn a_read_page_is_checked_where_it_ended_then_status_then_type_then_size() {
        let html = || Some(format!("<main><p>{}</p></main>", "Text. ".repeat(400)));
        let page = "https://a.test/page";
        assert_eq!(verdict(read(page, "text/html", 200, html())), "read Ok");
        assert_eq!(
            verdict(read("https://a.test/login", "text/html", 200, html())),
            "sign-in BehindLogin"
        );
        assert_eq!(
            verdict(read(
                "chrome-error://chromewebdata/",
                "text/html",
                200,
                html()
            )),
            "redirected away from the web"
        );
        assert_eq!(
            verdict(read("http://192.168.1.1/", "text/html", 200, html())),
            "redirected to a private network"
        );
        assert_eq!(
            verdict(read(page, "application/pdf", 404, None)),
            "HTTP 404",
            "status before type"
        );
        assert_eq!(verdict(read(page, "text/html", 503, html())), "passing");
        assert_eq!(
            verdict(read(page, "text/html", 0, html())),
            "no HTTP status"
        );
        assert_eq!(
            verdict(read(page, "application/pdf", 200, None)),
            "not HTML (application/pdf)"
        );
        assert_eq!(
            verdict(read(page, "text/html", 200, None)),
            "page too large"
        );
        // Under the cap in UTF-16 units, as the expression counts, and over
        // it in UTF-8 bytes, as Rust decodes it.
        let wide = "é".repeat(BODY_CAP / 2 + 1);
        assert!(wide.encode_utf16().count() <= BODY_CAP && wide.len() > BODY_CAP);
        assert_eq!(
            verdict(read(page, "text/html", 200, Some(wide))),
            "page too large"
        );
    }

    #[test]
    fn a_page_that_ends_on_a_sign_in_screen_says_where() {
        let fetcher = Fetcher::new(&BTreeSet::new());
        let navigated = Url::parse("https://a.test/page").unwrap();
        let landed = read("https://a.test/account?next=/page", "text/html", 200, None);
        let Ok(Ok(Outcome::SignIn(line))) =
            after(&fetcher, "https://a.test/page", &navigated, landed, false)
        else {
            panic!("a sign-in screen is read as one");
        };
        assert_eq!(line.status, Status::BehindLogin);
        assert_eq!(line.tier, Some(Tier::Headless));
        assert_eq!(
            line.final_url.as_deref(),
            Some("https://a.test/account?next=/page")
        );
    }
}
