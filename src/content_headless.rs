//! The headless tier's part of a `content` run: which pages a browser
//! renders, what a render writes, and what waits for a browser.
//!
//! slice: content
//! why: A page plain HTTP reads as thin, or as an empty shell its scripts
//!      would fill, may have its text once a browser runs them. Such a page
//!      is rendered once in the installed browser, after the run's HTTP
//!      reads, two tabs at a time and one per site, with no second HTTP
//!      read. The rendered text replaces what was kept only when it has more
//!      characters; otherwise the line the page already had is kept, marked
//!      as rendered and saying why, so a render never loses text and never
//!      leaves a page as an error. A render is final until `--refetch`.
//!      Without a browser, or when it fails, those pages wait, unrecorded by
//!      this tier, and the report says how many and why, every run. A signed
//!      in run renders through the same pass in the owner's browser, one
//!      page at a time: every line it writes says `signed_in`, and a page
//!      gets an image only with text read signed in.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use url::Url;

use crate::archive;
use crate::browser::{self, Broken, Browser, Outcome, Readiness};
use crate::capture::Log;
use crate::content::{seconds, spread};
use crate::content_image::Images;
use crate::content_pdf;
use crate::content_store::{Line, Page, Status, Tier};
use crate::error::Error;
use crate::fetch::{Fetcher, Refusal};
use crate::guard;
use crate::image_pick::Found;
use crate::targets;
use crate::triage::plural;

/// Pages rendered at once, each on a site of its own.
pub const TABS: usize = 2;
/// A progress line every this many renders.
const PROGRESS_EVERY: usize = 5;

/// Whether a page's line is one a browser may improve on: read over HTTP
/// as thin, or as an empty shell drawn by scripts or asking for them. An
/// app shell has no scripts to run, a browser shows a PDF without its
/// text, and every other status, and every line a route or a browser
/// read, stands.
pub fn escalates(line: &Line) -> bool {
    line.tier == Some(Tier::Web)
        && line.extractor.as_deref() != Some(content_pdf::EXTRACTOR)
        && match line.status {
            Status::Thin => true,
            Status::EmptyShell => matches!(
                line.reason.as_deref(),
                Some("drawn by scripts" | "needs JavaScript")
            ),
            _ => false,
        }
}

/// One document to render, and the pages it stands for.
#[derive(Debug, Clone)]
pub struct Render {
    /// Where the first page's HTTP read ended, else its address.
    pub url: Url,
    /// The queue it waits in: the host it is loaded from.
    pub host: String,
    /// The line each page has, as recorded.
    pub pages: Vec<Line>,
    /// What its HTTP read named of its image; unread on a later run.
    pub found: Found,
}

impl Render {
    /// The render of the pages whose recorded lines are `pages`, all one
    /// document, loaded from the address `load` names for the first;
    /// `None` when there is none, or nowhere valid to load it.
    pub fn of(pages: Vec<Line>, found: Found, load: fn(&Line) -> &str) -> Option<Self> {
        let url = guard::page_url(load(pages.first()?))?;
        Some(Self {
            host: guard::host_key(&url),
            url,
            pages,
            found,
        })
    }
}

/// Where a headless render loads a page: where its HTTP read ended, else
/// its address.
pub fn ended(line: &Line) -> &str {
    line.final_url.as_deref().unwrap_or(&line.url)
}

/// The pages this run read over HTTP that a browser may improve on: kept
/// for the second pass when there is a browser, counted as waiting when
/// there is not.
#[derive(Debug)]
pub struct Escalated {
    rendering: bool,
    renders: Mutex<Vec<Render>>,
    waiting: AtomicUsize,
}

impl Escalated {
    pub fn new(rendering: bool) -> Self {
        Self {
            rendering,
            renders: Mutex::new(Vec::new()),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Whether the pages one HTTP read recorded as `lines`, all one
    /// document, are rendered after, with what the read `found` of their
    /// image.
    pub fn keep(&self, lines: &[Line], found: &Found) -> bool {
        if !lines.first().is_some_and(escalates) {
            return false;
        }
        if self.rendering
            && let Some(render) = Render::of(lines.to_vec(), found.clone(), ended)
        {
            self.renders
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(render);
            return true;
        }
        self.waiting.fetch_add(lines.len(), Ordering::Relaxed);
        false
    }

    /// The renders kept, and how many pages wait for a browser.
    pub fn into_parts(self) -> (Vec<Render>, usize) {
        (
            self.renders
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            self.waiting.into_inner(),
        )
    }
}

/// The line a page whose recorded line is `baseline` gets after a render
/// by `tier` came to `outcome`, and the text to keep with it. Rendered text
/// replaces the kept text only when it has more characters; otherwise the
/// baseline's text is kept whole, marked as attempted by `tier`. Never an
/// error.
pub fn settle<'a>(baseline: &Line, outcome: &'a Outcome, tier: Tier) -> (Line, Option<&'a Page>) {
    let kept = baseline.chars.unwrap_or(0);
    let not_rendered = |why: &str| {
        (
            copied(baseline, &format!("not rendered: {why}"), tier),
            None,
        )
    };
    match outcome {
        Outcome::Read(capture)
            if capture.page.as_ref().is_some_and(|page| page.chars > kept)
                && !matches!(
                    capture.line.status,
                    Status::Error | Status::Unavailable | Status::Skipped
                ) =>
        {
            let mut line = capture.line.clone();
            baseline.url.clone_into(&mut line.url);
            line.attempt = baseline.attempt;
            (line, capture.page.as_ref())
        }
        Outcome::Read(_) => (copied(baseline, "rendering added no text", tier), None),
        Outcome::SignIn(signed) => {
            let mut line = (**signed).clone();
            baseline.url.clone_into(&mut line.url);
            line.attempt = baseline.attempt;
            line.chars = baseline.chars;
            line.content_sha256.clone_from(&baseline.content_sha256);
            line.extractor.clone_from(&baseline.extractor);
            line.extractor_version
                .clone_from(&baseline.extractor_version);
            line.lang.clone_from(&baseline.lang);
            (line, None)
        }
        Outcome::Refused(refusal) => not_rendered(&refusal.reason()),
        Outcome::NotRendered(why) => not_rendered(why),
    }
}

/// `baseline` as it stands, attempted now by `tier` with its access, its
/// reason saying what the render came to.
fn copied(baseline: &Line, note: &str, tier: Tier) -> Line {
    let mut line = baseline.clone();
    line.tier = Some(tier);
    line.access = Some(tier.access());
    line.attempted_at = archive::now();
    line.reason = Some(match &baseline.reason {
        Some(reason) => format!("{reason}; {note}"),
        None => note.to_owned(),
    });
    line
}

/// What the headless tier did in a run.
#[derive(Debug, Default)]
pub struct Headless {
    /// The browser's name, when there was one to render with.
    pub browser: Option<String>,
    /// How long each render took, its retries included.
    pub renders: Vec<Duration>,
    /// Pages left for a run with a browser.
    pub waiting: usize,
    /// Why they wait; none when the owner asked for no browser.
    pub why: Option<String>,
    /// Connections the relay refused for this machine or the private
    /// network.
    pub refused: usize,
    /// What a signed in run counts beyond a render's; `None` for a
    /// headless run.
    pub signed_in: Option<SignedIn>,
}

/// What a signed in run counts beyond a render's.
#[derive(Debug, Default)]
pub struct SignedIn {
    /// Personal app pages the plan left unopened.
    pub not_opened: usize,
    /// Pages that ended on a personal app, nothing of them kept.
    pub not_kept: usize,
    /// Pages refused to this machine or the private network.
    pub private: usize,
}

impl Headless {
    /// What a signed in run in browser `name` starts from: the personal
    /// app pages its plan left unopened.
    pub fn signed_in(name: String, not_opened: usize) -> Self {
        Self {
            browser: Some(name),
            signed_in: Some(SignedIn {
                not_opened,
                ..SignedIn::default()
            }),
            ..Self::default()
        }
    }

    /// A page the fetcher's rules refused, which a signed in run counts.
    fn refused_page(&mut self, refusal: &Refusal) {
        let Some(signed_in) = &mut self.signed_in else {
            return;
        };
        match refusal {
            Refusal::PersonalApp => signed_in.not_kept += 1,
            Refusal::PrivateNetwork { .. } | Refusal::PrivateAddress => signed_in.private += 1,
            _ => {}
        }
    }

    /// What a run starts from: the pages the plan left waiting, and why.
    pub fn new(readiness: Option<&Readiness>, waiting: usize) -> Self {
        Self {
            browser: readiness.and_then(Readiness::path).map(browser::name),
            waiting,
            why: match readiness {
                Some(Readiness::Missing(why)) => Some(why.clone()),
                _ => None,
            },
            ..Self::default()
        }
    }

    /// Pages that read thin or empty in this run with no browser to render
    /// them.
    pub fn wait(&mut self, pages: usize) {
        self.waiting += pages;
    }

    /// What the report says, one line each.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let (Some(browser), Some((median, longest))) = (&self.browser, spread(&self.renders)) {
            let (did, how) = match self.signed_in {
                Some(_) => ("opened", " signed in"),
                None => ("rendered", ""),
            };
            lines.push(format!(
                "{did} {} in {browser}{how}; a render took {} s at the median, {} s at the longest",
                plural(self.renders.len(), "page"),
                seconds(median),
                seconds(longest),
            ));
        }
        match &self.signed_in {
            Some(signed_in) => {
                if signed_in.not_opened + signed_in.not_kept > 0 {
                    lines.push(format!(
                        "personal apps: {} not opened, {} not kept",
                        signed_in.not_opened, signed_in.not_kept
                    ));
                }
                if signed_in.private > 0 {
                    lines.push(format!(
                        "refused {} to the private network",
                        plural(signed_in.private, "page")
                    ));
                }
            }
            None if self.refused > 0 => lines.push(format!(
                "refused {} to the private network",
                plural(self.refused, "connection")
            )),
            None => {}
        }
        lines.extend(waiting_note(self.waiting, self.why.as_deref()));
        lines
    }

    pub fn json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "rendered": self.renders.len(),
            "waiting": self.waiting,
            "refused_private": self.refused,
            "render_seconds": spread(&self.renders).map(|(median, longest)| serde_json::json!({
                "median": seconds(median), "longest": seconds(longest),
            })),
        });
        if let Some(signed_in) = &self.signed_in {
            json["signed_in"] = serde_json::json!({
                "personal_not_opened": signed_in.not_opened,
                "personal_not_kept": signed_in.not_kept,
                "refused_private_pages": signed_in.private,
            });
        }
        json
    }
}

/// The note for `waiting` pages left for a browser; `why` they wait, none
/// when the owner asked for no browser.
fn waiting_note(waiting: usize, why: Option<&str>) -> Option<String> {
    let pages = plural(waiting, "page");
    (waiting > 0).then(|| match why {
        Some(why) => format!("{pages} waiting for a browser: {why} (see knowmoretabs doctor)"),
        None => format!("{pages} left for a run with a browser"),
    })
}

/// What `--dry-run` says of the headless tier: the renders planned, that
/// this run's thin or empty pages are rendered after, or what waits.
pub fn dry_run_notes(
    readiness: Option<&Readiness>,
    renders: usize,
    waiting: usize,
    web_fetches: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(name) = readiness.and_then(Readiness::path).map(browser::name) {
        if renders > 0 {
            notes.push(format!(
                "{} rendered in {name}, {TABS} at a time",
                match renders {
                    1 => "1 page is".to_owned(),
                    n => format!("{n} pages are"),
                }
            ));
        }
        if web_fetches {
            notes.push(format!(
                "pages that read thin or empty in this run are rendered afterwards in {name}"
            ));
        }
    }
    let why = match readiness {
        Some(Readiness::Missing(why)) => Some(why.as_str()),
        _ => None,
    };
    notes.extend(waiting_note(waiting, why));
    notes
}

/// The second pass: renders `renders` in `browser`, or says why none
/// started, at most [`TABS`] at a time and one per host, or one at a time
/// in the owner's browser, writing each page's line through `write`, then
/// settling its image through `images`. A browser that did not start, or
/// stops answering, leaves its pages waiting.
pub fn run(
    renders: &[Render],
    browser: Result<Browser, String>,
    fetcher: &Fetcher,
    images: Option<&Images>,
    write: impl Fn(Line, Option<&Page>) -> Result<Line, Error> + Sync,
    headless: &mut Headless,
    log: Log,
) -> Result<(), Error> {
    let browser = match browser {
        Ok(browser) => Some(browser),
        Err(why) => {
            log.warn(&format!("the browser did not start: {why}"));
            headless.why = Some(why);
            None
        }
    };
    let tier = browser.as_ref().map_or(Tier::Headless, Browser::tier);
    // The owner's browser is reached on one socket: one page at a time.
    let tabs = if tier == Tier::SignedIn { 1 } else { TABS };
    // Signed in, a page gets an image only with text read signed in.
    let pictured = |kept: bool| kept || tier == Tier::Headless;
    let broken: Mutex<Option<String>> = Mutex::new(headless.why.clone());
    let tally = Mutex::new(&mut *headless);
    let result = targets::by_host(
        tabs,
        renders.iter().map(|render| (render.host.as_str(), render)),
        |render| {
            let started = Instant::now();
            let failed = broken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let rendered = match (&browser, failed) {
                (Some(browser), None) => {
                    browser.render(fetcher, &render.pages[0].url, &render.url, images.is_some())
                }
                (_, why) => Err(Broken(why.unwrap_or_default())),
            };
            let outcome = match rendered {
                Ok(outcome) => outcome,
                Err(Broken(why)) => {
                    broken
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_or_insert(why);
                    tally.lock().unwrap_or_else(PoisonError::into_inner).waiting +=
                        render.pages.len();
                    let pages: &[Line] = if pictured(false) { &render.pages } else { &[] };
                    return images
                        .map_or(Ok(()), |images| images.after(fetcher, pages, &render.found));
                }
            };
            {
                let mut tally = tally.lock().unwrap_or_else(PoisonError::into_inner);
                tally.renders.push(started.elapsed());
                if let Outcome::Refused(refusal) = &outcome {
                    tally.refused_page(refusal);
                }
            }
            let mut written = Vec::with_capacity(render.pages.len());
            for baseline in &render.pages {
                let (line, page) = settle(baseline, &outcome, tier);
                let kept = page.is_some();
                let line = write(line, page)?;
                if pictured(kept) {
                    written.push(line);
                }
            }
            // A sign-in screen's head is not the page's, nor is nothing.
            let found = match &outcome {
                Outcome::Read(capture) if capture.images != Found::Unread => &capture.images,
                _ => &render.found,
            };
            images.map_or(Ok(()), |images| images.after(fetcher, &written, found))
        },
        |n, total| {
            if targets::progress_due(n, total, PROGRESS_EVERY) {
                log.progress(&format!("rendered {n} of {total}"));
            }
        },
    );
    let headless = tally.into_inner().unwrap_or_else(PoisonError::into_inner);
    if let Some(browser) = browser {
        headless.refused = browser.refused();
    }
    if let Some(why) = broken.into_inner().unwrap_or_else(PoisonError::into_inner) {
        headless.why = Some(why);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_fetch::{self, Capture};
    use crate::content_store::{self, Access, Store};
    use crate::content_test::page;

    fn web(url: &str, status: Status, reason: &str) -> Line {
        let mut line = content_fetch::line(url, Tier::Web, status).with_reason(reason);
        line.final_url = Some(format!("{url}?after=redirect"));
        line.http_status = Some(200);
        line.lang = Some("en".to_owned());
        line.extractor = Some("dom_smoothie".to_owned());
        line.extractor_version = Some("0.18.2".to_owned());
        line
    }

    #[test]
    fn only_web_pages_that_read_thin_or_drawn_by_scripts_escalate() {
        let url = "https://a.test/";
        assert!(escalates(&web(url, Status::Thin, "short text")));
        assert!(escalates(&web(url, Status::EmptyShell, "drawn by scripts")));
        assert!(escalates(&web(url, Status::EmptyShell, "needs JavaScript")));
        assert!(!escalates(&web(url, Status::EmptyShell, "app shell")));
        for status in [
            Status::Ok,
            Status::Paywalled,
            Status::BehindLogin,
            Status::Blocked,
            Status::NotFound,
            Status::Error,
        ] {
            assert!(!escalates(&web(url, status, "x")), "{status:?}");
        }
        for tier in [Tier::Headless, Tier::X, Tier::Github, Tier::Youtube] {
            let mut line = web(url, Status::Thin, "short text");
            line.tier = Some(tier);
            assert!(!escalates(&line), "{tier:?}");
        }
        let mut untiered = web(url, Status::Thin, "short text");
        untiered.tier = None;
        assert!(!escalates(&untiered));
        let mut pdf = web(url, Status::Thin, "short text");
        pdf.extractor = Some("lopdf".to_owned());
        pdf.extractor_version = Some("0.45.0".to_owned());
        assert!(!escalates(&pdf), "a browser shows a PDF without its text");
    }

    #[test]
    fn a_render_loads_where_the_first_page_ended() {
        let first = web("https://a.test/p", Status::Thin, "short text");
        let mut alias = first.clone();
        alias.url = "https://a.test/p#part".to_owned();
        let render = Render::of(vec![first, alias], Found::Unread, ended).unwrap();
        assert_eq!(render.url.as_str(), "https://a.test/p?after=redirect");
        assert_eq!(render.host, "a.test");
        assert_eq!(render.pages.len(), 2);
        assert!(Render::of(Vec::new(), Found::Unread, ended).is_none());
    }

    #[test]
    fn more_rendered_text_wins_and_otherwise_the_kept_line_stands_after_a_real_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let url = "https://a.test/p";
        let kept = page("# Kept\n\nShort.\n", 6);
        let baseline = store
            .record(web(url, Status::Thin, "short text"), Some(&kept))
            .unwrap();
        let file = content_store::dir(dir.path()).join(content_store::file_name(url));
        let before = std::fs::read(&file).unwrap();

        let rendered = |chars: usize| {
            let mut line = content_fetch::line(url, Tier::Headless, Status::Ok);
            line.final_url = Some(url.to_owned());
            line.http_status = Some(200);
            Outcome::Read(Box::new(Capture {
                line,
                page: Some(page("# Kept\n\nShort, then rendered.\n", chars)),
                images: Found::Unread,
            }))
        };
        let outcomes = [
            rendered(6),
            Outcome::NotRendered("HTTP 404".to_owned()),
            Outcome::Read(Box::new(Capture::ended(content_fetch::line(
                url,
                Tier::Headless,
                Status::BehindLogin,
            )))),
        ];
        for outcome in &outcomes {
            let (line, page) = settle(&baseline, outcome, Tier::Headless);
            assert!(page.is_none());
            let written = store.record(line, page).unwrap();
            assert_eq!(written.status, Status::Thin);
            assert_eq!(written.tier, Some(Tier::Headless));
            assert_eq!(written.chars, baseline.chars);
            assert_eq!(written.content_sha256, baseline.content_sha256);
            assert_eq!(written.final_url, baseline.final_url);
            assert_eq!(written.attempt, baseline.attempt);
            assert_eq!(std::fs::read(&file).unwrap(), before, "file untouched");
        }
        let (copy, _) = settle(&baseline, &outcomes[1], Tier::Headless);
        assert_eq!(
            copy.reason.as_deref(),
            Some("short text; not rendered: HTTP 404")
        );
        let (copy, _) = settle(&baseline, &outcomes[0], Tier::Headless);
        assert_eq!(
            copy.reason.as_deref(),
            Some("short text; rendering added no text")
        );

        let more = rendered(40);
        let (line, page) = settle(&baseline, &more, Tier::Headless);
        let written = store.record(line, page).unwrap();
        assert_eq!(
            (written.status, written.tier, written.chars),
            (Status::Ok, Some(Tier::Headless), Some(40))
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("tier: \"headless\""));
        assert!(text.ends_with("Short, then rendered.\n"));
    }

    #[test]
    fn a_page_that_ends_on_a_sign_in_screen_keeps_its_text_fields_and_nothing_is_an_error() {
        let mut baseline = web("https://a.test/p", Status::EmptyShell, "drawn by scripts");
        baseline.chars = Some(12);
        baseline.content_sha256 = Some("ab".repeat(32));
        baseline.attempt = 2;
        let mut login =
            content_fetch::line("https://a.test/p", Tier::Headless, Status::BehindLogin)
                .with_reason("redirected to a login page");
        login.final_url = Some("https://a.test/login".to_owned());
        let signed_in = Outcome::SignIn(Box::new(login));
        let (line, kept) = settle(&baseline, &signed_in, Tier::Headless);
        assert!(kept.is_none());
        assert_eq!(line.status, Status::BehindLogin);
        assert_eq!(line.final_url.as_deref(), Some("https://a.test/login"));
        assert_eq!(
            (line.chars, line.content_sha256.as_deref(), line.attempt),
            (Some(12), baseline.content_sha256.as_deref(), 2)
        );
        assert_eq!(line.access, Some(Access::Public));
        for outcome in [
            Outcome::NotRendered("timeout".to_owned()),
            Outcome::Read(Box::new(Capture {
                line: content_fetch::line("https://a.test/p", Tier::Headless, Status::Error),
                page: Some(page("much longer text", 99)),
                images: Found::Unread,
            })),
        ] {
            assert_eq!(
                settle(&baseline, &outcome, Tier::Headless).0.status,
                Status::EmptyShell
            );
        }
    }

    #[test]
    fn a_signed_in_report_says_what_it_opened_and_what_it_left() {
        let mut signed_in = Headless::signed_in("Google Chrome".to_owned(), 18);
        signed_in.renders = vec![Duration::from_millis(2500)];
        signed_in.refused_page(&Refusal::PersonalApp);
        signed_in.refused_page(&Refusal::PrivateAddress);
        signed_in.refused_page(&Refusal::Failed("timeout".to_owned()));
        assert_eq!(
            signed_in.lines(),
            [
                "opened 1 page in Google Chrome signed in; a render took 2.5 s at the median, 2.5 s at the longest",
                "personal apps: 18 not opened, 1 not kept",
                "refused 1 page to the private network",
            ]
        );
        assert_eq!(signed_in.json()["signed_in"]["personal_not_kept"], 1);
        let mut headless = Headless::default();
        headless.refused_page(&Refusal::PersonalApp);
        assert!(headless.signed_in.is_none() && headless.json().get("signed_in").is_none());
    }

    #[test]
    fn waiting_pages_are_counted_with_why() {
        let missing = Readiness::Missing("no chrome binary found".to_owned());
        let mut headless = Headless::new(Some(&missing), 2);
        headless.wait(1);
        assert_eq!(
            headless.lines(),
            ["3 pages waiting for a browser: no chrome binary found (see knowmoretabs doctor)"]
        );
        let off = Headless::new(Some(&Readiness::Off), 1);
        assert_eq!(off.lines(), ["1 page left for a run with a browser"]);
        assert_eq!(Headless::new(None, 0).lines(), Vec::<String>::new());
        let ready = Readiness::Ready("/opt/Google Chrome".into());
        let mut ran = Headless::new(Some(&ready), 0);
        ran.renders = vec![Duration::from_millis(1500), Duration::from_millis(4200)];
        ran.refused = 3;
        assert_eq!(
            ran.lines(),
            [
                "rendered 2 pages in Google Chrome; a render took 4.2 s at the median, 4.2 s at the longest",
                "refused 3 connections to the private network",
            ]
        );
        let json = ran.json();
        assert_eq!(
            (
                json["rendered"].as_u64(),
                json["waiting"].as_u64(),
                json["refused_private"].as_u64()
            ),
            (Some(2), Some(0), Some(3))
        );
        assert_eq!(
            dry_run_notes(Some(&ready), 3, 0, true),
            [
                "3 pages are rendered in Google Chrome, 2 at a time",
                "pages that read thin or empty in this run are rendered afterwards in Google Chrome",
            ]
        );
        assert_eq!(
            dry_run_notes(Some(&missing), 0, 4, true),
            ["4 pages waiting for a browser: no chrome binary found (see knowmoretabs doctor)"]
        );
    }
}
