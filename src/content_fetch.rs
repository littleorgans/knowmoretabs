//! What `content` keeps of one page over plain HTTP: the whole body, read
//! through the guarded fetcher, tried again when the failure is passing,
//! and turned into an attempt line and the page's text.
//!
//! slice: content
//! why: The fetcher knows how to ask and nothing of what an answer means;
//!      `content` needs the whole page, not its head, and a different
//!      reading of the answer than `enrich`: a 403 is a block worth a signed
//!      in browser later, a 404 is final, and a 429 or a 503 is a site
//!      asking us to come back. Those passing failures are retried a couple
//!      of times with growing waits, honouring the site's own Retry-After,
//!      and a site that says slow down gets fewer requests for the rest of
//!      the run. The decisions are pure functions so they can be tested
//!      without a network, and the X post route answers by the same rules.
//!      A page a browser rendered is read from its HTML on, as a response is.
//!      A page that is an image is media, its copy the page's image, and a
//!      PDF's text is read as a page's is.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use jiff::Timestamp;
use url::Url;

use crate::content_pdf;
use crate::content_store::{Access, Completeness, Line, Page, Status, Tier};
use crate::extract::{self, Class};
use crate::fetch::{self, Fetcher, Refusal, Response};
use crate::head;
use crate::image_page;
use crate::image_pick::{self, Found, Source};

/// The most of a page's body read; enough for any article, not for a file.
pub const BODY_CAP: usize = 10 * 1024 * 1024;
/// Further tries in one run after a passing failure.
const RETRIES: u32 = 2;
/// The longest a site's Retry-After is waited for in a run.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);
/// Runs ending in `error` before a page is `unavailable`.
pub const RUNS_BEFORE_UNAVAILABLE: u32 = 3;

/// One page's outcome: its line, without the run's attempt number yet, its
/// text when there is text to keep, and what it says about its image.
#[derive(Debug, Clone)]
pub struct Capture {
    pub line: Line,
    pub page: Option<Page>,
    pub images: Found,
}

impl Capture {
    /// An attempt that ended without reading the page.
    pub fn ended(line: Line) -> Self {
        Self {
            line,
            page: None,
            images: Found::Unread,
        }
    }
}

/// A failure that may pass: worth another try after a wait. `T` is how
/// the attempt ends once its retries are spent.
#[derive(Debug, Clone)]
pub struct Passing<T = Capture> {
    failed: Box<T>,
    retry_after: Option<Duration>,
}

impl<T> Passing<T> {
    /// Failure `failed`, to be retried no sooner than `retry_after`.
    pub fn after(failed: T, retry_after: Option<Duration>) -> Self {
        Self {
            failed: Box::new(failed),
            retry_after,
        }
    }
}

impl Passing {
    /// Failure `line`, to be retried no sooner than `retry_after`.
    pub fn new(line: Line, retry_after: Option<Duration>) -> Self {
        Self::after(Capture::ended(line), retry_after)
    }

    #[cfg(test)]
    pub fn line(&self) -> &Line {
        &self.failed.line
    }
}

/// Fetches and reads one page, retrying passing failures, and with
/// `images` the images it names. Never fails: a failure is a capture too.
pub fn capture(fetcher: &Fetcher, raw: &str, images: bool) -> Capture {
    retrying(|| once(fetcher, raw, images))
}

/// Tries `once` until it captures something or its passing failures have
/// had their retries; every route, and every image, retries by these rules.
pub fn retrying<T>(mut once: impl FnMut() -> Result<T, Passing<T>>) -> T {
    let mut retry = 0;
    loop {
        match once() {
            Ok(capture) => return capture,
            Err(passing) => {
                retry += 1;
                let jitter = Duration::from_millis(random() % 1000);
                let Some(wait) = retry_wait(retry, passing.retry_after, jitter) else {
                    return *passing.failed;
                };
                std::thread::sleep(wait);
            }
        }
    }
}

fn once(fetcher: &Fetcher, raw: &str, images: bool) -> Result<Capture, Passing> {
    let response = match fetcher.get(raw, fetch::ACCEPT_HTML) {
        Ok(response) => response,
        Err(refusal) => return refused(raw, Tier::Web, refusal),
    };
    if response.status == 429 {
        fetcher.slow_down(&response.url);
    }
    read(raw, response, images)
}

/// A line for page `raw`, read by `tier` without signing in.
pub fn public_line(raw: &str, tier: Tier, status: Status) -> Line {
    let mut line = Line::new(raw, status);
    line.tier = Some(tier);
    line.access = Some(Access::Public);
    line
}

/// What a GET that ended without a response says about page `raw`.
pub fn refused(raw: &str, tier: Tier, refusal: Refusal) -> Result<Capture, Passing> {
    let reason = refusal.reason();
    let ended = |status: Status| public_line(raw, tier, status).with_reason(reason.clone());
    let line = match refusal {
        Refusal::Login(url) => {
            let mut line = ended(Status::BehindLogin);
            line.final_url = Some(url.to_string());
            line
        }
        refusal if refusal.is_rule() => ended(Status::Skipped),
        Refusal::Failed(_) if is_passing(&reason) => {
            return Err(Passing::new(ended(Status::Error), None));
        }
        _ => ended(Status::Error),
    };
    Ok(Capture::ended(line))
}

/// Connection trouble that may be gone in a few seconds. A name that does
/// not resolve is not.
pub fn is_passing(reason: &str) -> bool {
    matches!(
        reason,
        "timeout" | "connection reset" | "connection failed" | "connection closed early"
    )
}

/// What an HTTP status says about a page, when it says something final or
/// passing; `None` for a success.
pub fn status_outcome(status: u16) -> Option<(Status, bool)> {
    match status {
        200..=299 => None,
        401 => Some((Status::BehindLogin, false)),
        403 => Some((Status::Blocked, false)),
        404 | 410 => Some((Status::NotFound, false)),
        429 | 502..=504 => Some((Status::Error, true)),
        _ => Some((Status::Error, false)),
    }
}

fn read(raw: &str, mut response: Response, images: bool) -> Result<Capture, Passing> {
    let final_url = response.url.to_string();
    let status = response.status;
    let line = |state: Status, reason: Option<String>| {
        let mut line = public_line(raw, Tier::Web, state);
        line.reason = reason;
        line.final_url = Some(final_url.clone());
        line.http_status = Some(status);
        line
    };
    let done = |line: Line| Ok(Capture::ended(line));
    if let Some((state, passing)) = status_outcome(status) {
        let failed = line(state, Some(format!("HTTP {status}")));
        if passing {
            return Err(self::passing(&response, failed));
        }
        return done(failed);
    }
    let mime = response.mime();
    if mime != content_pdf::MIME && !mime.is_empty() && !mime.contains("html") {
        let (state, reason) = not_html(&mime);
        let mut ended = Capture::ended(line(state, Some(reason)));
        ended.images = image_pick::not_html(&mime, &final_url);
        return Ok(ended);
    }
    let bytes = match body(&mut response, |state, reason| line(state, Some(reason))) {
        Ok(bytes) => bytes,
        Err(ended) => return *ended,
    };
    if mime == content_pdf::MIME {
        let images = image_pick::not_html(&mime, &final_url);
        return Ok(match content_pdf::read(&bytes) {
            Ok(page) => {
                let mut read = if page.completeness == Completeness::Full {
                    line(Status::Ok, None)
                } else {
                    line(Status::Thin, Some("short text".to_owned()))
                };
                read.extractor = Some(content_pdf::EXTRACTOR.to_owned());
                read.extractor_version = Some(content_pdf::EXTRACTOR_VERSION.to_owned());
                Capture {
                    line: read,
                    page: Some(page),
                    images,
                }
            }
            Err(reason) => Capture {
                line: line(Status::NotHtml, Some(reason.to_owned())),
                page: None,
                images,
            },
        });
    }
    let html = match head::decode(
        &bytes,
        head::charset_param(&response.content_type).as_deref(),
    ) {
        Ok(html) => html,
        Err(label) => {
            return done(line(
                Status::Error,
                Some(format!("unsupported charset {label}")),
            ));
        }
    };
    Ok(from_html(
        raw,
        Tier::Web,
        &response.url,
        status,
        &html,
        images,
    ))
}

/// How a page of type `mime` that is neither HTML nor a PDF ends: an
/// image is `media`, its copy the page's image; anything else `not_html`.
fn not_html(mime: &str) -> (Status, String) {
    if mime.starts_with("image/") {
        (Status::Media, format!("image ({mime})"))
    } else {
        (Status::NotHtml, format!("not HTML ({mime})"))
    }
}

/// Whether `line` is one an earlier build wrote as not HTML for a type
/// this build reads, an image or a PDF: read again on a plain run.
pub fn outdated(line: &Line) -> bool {
    line.status == Status::NotHtml
        && line.tier == Some(Tier::Web)
        && line
            .reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("not HTML (")?.strip_suffix(')'))
            .is_some_and(|mime| mime == content_pdf::MIME || not_html(mime).0 != Status::NotHtml)
}

/// What a page's decoded `html` says about page `raw`, read by `tier` at
/// `final_url` with HTTP `status`: its class, its text, and with `images`
/// the images it names. The web tier reads the HTML from a response, the
/// headless tier from a page a browser rendered; the rules are the same.
pub fn from_html(
    raw: &str,
    tier: Tier,
    final_url: &Url,
    status: u16,
    html: &str,
    images: bool,
) -> Capture {
    let found = extract::page(html, Some(final_url.as_str()));
    // A sign-in screen's head describes the screen, not the page.
    let images = if !images || found.class == Class::BehindLogin {
        Found::Unread
    } else {
        let body = if tier == Tier::Headless {
            Source::RenderedImg
        } else {
            Source::BodyImg
        };
        image_page::page(html, found.article.as_deref(), final_url, body)
    };
    let mut captured = public_line(raw, tier, class_status(found.class));
    captured.reason = found.reason.map(str::to_owned);
    captured.final_url = Some(final_url.to_string());
    captured.http_status = Some(status);
    captured.lang.clone_from(&found.lang);
    let page = (!found.markdown.is_empty()).then(|| {
        captured.extractor = Some(found.extractor.to_owned());
        let extractor = if found.extractor == extract::EXTRACTOR {
            captured.extractor_version = Some(extract::EXTRACTOR_VERSION.to_owned());
            format!("{} {}", extract::EXTRACTOR, extract::EXTRACTOR_VERSION)
        } else {
            found.extractor.to_owned()
        };
        Page {
            title: found.title.clone(),
            extractor,
            completeness: if found.class == Class::Ok {
                Completeness::Full
            } else {
                Completeness::Thin
            },
            chars: found.chars,
            captions: None,
            markdown: found.markdown,
        }
    });
    Capture {
        line: captured,
        page,
        images,
    }
}

/// A passing failure `failed`, to be retried no sooner than `response`
/// asks in its Retry-After.
pub fn passing(response: &Response, failed: Line) -> Passing {
    Passing::new(failed, asked_wait(response))
}

/// How long `response` asks us to wait in its Retry-After.
pub fn asked_wait(response: &Response) -> Option<Duration> {
    response
        .header("retry-after")
        .and_then(|value| retry_after(value, Timestamp::now()))
}

/// Up to [`BODY_CAP`] of a response's body, sent as is. Otherwise how the
/// attempt ends: `line` makes its line from a status and a reason.
pub fn body(
    response: &mut Response,
    line: impl Fn(Status, String) -> Line,
) -> Result<Vec<u8>, Box<Result<Capture, Passing>>> {
    let done = |line: Line| Err(Box::new(Ok(Capture::ended(line))));
    if !response.is_identity() {
        return done(line(
            Status::Error,
            "unsupported content encoding".to_owned(),
        ));
    }
    match response.read(BODY_CAP, false) {
        Ok(bytes) => Ok(bytes),
        Err(reason) if is_passing(&reason) => Err(Box::new(Err(Passing::new(
            line(Status::Error, reason),
            None,
        )))),
        Err(reason) => done(line(Status::Error, reason)),
    }
}

fn class_status(class: Class) -> Status {
    match class {
        Class::Ok => Status::Ok,
        Class::Thin => Status::Thin,
        Class::EmptyShell => Status::EmptyShell,
        Class::BehindLogin => Status::BehindLogin,
        Class::Paywalled => Status::Paywalled,
    }
}

/// How long to wait before retry number `retry` (1 or 2): 2 s, then 8 s,
/// or longer when the site asked for longer, plus `jitter`. `None` when the
/// retries are spent or the site asked for more than a run waits.
fn retry_wait(retry: u32, retry_after: Option<Duration>, jitter: Duration) -> Option<Duration> {
    if retry == 0 || retry > RETRIES {
        return None;
    }
    let backoff = Duration::from_secs(2 * 4u64.pow(retry - 1));
    match retry_after {
        Some(asked) if asked > RETRY_AFTER_CAP => None,
        Some(asked) => Some(backoff.max(asked) + jitter),
        None => Some(backoff + jitter),
    }
}

/// `Retry-After` as a wait from `now`: seconds, or an HTTP date. A date in
/// the past is no wait.
pub fn retry_after(value: &str, now: Timestamp) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = jiff::fmt::rfc2822::parse(value).ok()?.timestamp();
    let seconds = at.as_second().saturating_sub(now.as_second());
    Some(Duration::from_secs(u64::try_from(seconds).unwrap_or(0)))
}

/// A log line whose page is tried again on the next run while it says
/// `error`, until [`RUNS_BEFORE_UNAVAILABLE`] runs have.
pub trait Attempted {
    fn is_error(&self) -> bool;
    fn attempt(&self) -> u32;
    fn set_attempt(&mut self, attempt: u32);
    fn reason(&self) -> Option<&str>;
    /// The page is given up on: `unavailable`, saying `reason`.
    fn give_up(&mut self, reason: String);
}

impl Attempted for Line {
    fn is_error(&self) -> bool {
        self.status == Status::Error
    }

    fn attempt(&self) -> u32 {
        self.attempt
    }

    fn set_attempt(&mut self, attempt: u32) {
        self.attempt = attempt;
    }

    fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    fn give_up(&mut self, reason: String) {
        self.status = Status::Unavailable;
        self.reason = Some(reason);
    }
}

/// A page's line for this run: its attempt number, and `unavailable` once
/// the runs ending in `error` reach [`RUNS_BEFORE_UNAVAILABLE`].
pub fn settle<L: Attempted>(mut line: L, attempt: u32) -> L {
    line.set_attempt(attempt);
    if line.is_error() && attempt >= RUNS_BEFORE_UNAVAILABLE {
        let reason = format!(
            "{}; failed on {attempt} runs",
            line.reason().unwrap_or_default()
        );
        line.give_up(reason);
    }
    line
}

/// The attempt number for a page whose latest line is `previous`: one more
/// after an `error`, else a fresh start.
pub fn next_attempt<L: Attempted>(previous: Option<&L>) -> u32 {
    match previous {
        Some(line) if line.is_error() => line.attempt().saturating_add(1),
        _ => 1,
    }
}

/// Enough randomness to spread retries apart, with no dependency.
fn random() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(u64::from(std::process::id()));
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JITTER: Duration = Duration::from_millis(300);

    #[test]
    fn retries_wait_two_then_eight_seconds_then_stop() {
        assert_eq!(
            retry_wait(1, None, JITTER),
            Some(Duration::from_millis(2300))
        );
        assert_eq!(
            retry_wait(2, None, JITTER),
            Some(Duration::from_millis(8300))
        );
        assert_eq!(retry_wait(3, None, JITTER), None);
        assert_eq!(retry_wait(0, None, JITTER), None);
    }

    #[test]
    fn retry_after_is_honoured_up_to_a_minute() {
        let secs = Duration::from_secs;
        assert_eq!(
            retry_wait(1, Some(secs(30)), JITTER),
            Some(secs(30) + JITTER)
        );
        assert_eq!(retry_wait(2, Some(secs(1)), JITTER), Some(secs(8) + JITTER));
        assert_eq!(
            retry_wait(1, Some(secs(60)), JITTER),
            Some(secs(60) + JITTER)
        );
        assert_eq!(retry_wait(1, Some(secs(61)), JITTER), None);
    }

    #[test]
    fn retry_after_reads_seconds_and_http_dates() {
        let now: Timestamp = "2026-10-07T09:00:00Z".parse().unwrap();
        assert_eq!(retry_after(" 120 ", now), Some(Duration::from_secs(120)));
        assert_eq!(
            retry_after("Wed, 07 Oct 2026 09:00:30 GMT", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_after("Wed, 07 Oct 2026 08:00:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(retry_after("soon", now), None);
        assert_eq!(retry_after("-5", now), None);
    }

    #[test]
    fn http_statuses_map_to_the_vocabulary() {
        assert_eq!(status_outcome(200), None);
        assert_eq!(status_outcome(401), Some((Status::BehindLogin, false)));
        assert_eq!(status_outcome(403), Some((Status::Blocked, false)));
        assert_eq!(status_outcome(404), Some((Status::NotFound, false)));
        assert_eq!(status_outcome(410), Some((Status::NotFound, false)));
        for passing in [429, 502, 503, 504] {
            assert_eq!(
                status_outcome(passing),
                Some((Status::Error, true)),
                "{passing}"
            );
        }
        for failed in [400, 500, 501, 418] {
            assert_eq!(
                status_outcome(failed),
                Some((Status::Error, false)),
                "{failed}"
            );
        }
    }

    #[test]
    fn refusals_keep_rules_home_and_retry_only_passing_failures() {
        let status = |refusal| match refused("https://a.test/", Tier::Web, refusal) {
            Ok(capture) => (capture.line.status, false),
            Err(passing) => (passing.line().status, true),
        };
        assert_eq!(status(Refusal::PrivateAddress), (Status::Skipped, false));
        assert_eq!(status(Refusal::TokenOrSearch), (Status::Skipped, false));
        assert_eq!(status(Refusal::TooManyRedirects), (Status::Error, false));
        assert_eq!(
            status(Refusal::Failed("timeout".into())),
            (Status::Error, true)
        );
        assert_eq!(
            status(Refusal::Failed("connection reset".into())),
            (Status::Error, true)
        );
        assert_eq!(
            status(Refusal::Failed("host not found".into())),
            (Status::Error, false)
        );
        let login = refused(
            "https://a.test/",
            Tier::Web,
            Refusal::Login(url::Url::parse("https://a.test/login").unwrap()),
        )
        .unwrap()
        .line;
        assert_eq!(login.status, Status::BehindLogin);
        assert_eq!(login.final_url.as_deref(), Some("https://a.test/login"));
    }

    #[test]
    fn rendered_html_is_read_by_the_same_rules_and_names_its_images_as_rendered() {
        let html = format!(
            r#"<html lang="en"><head><title>Rendered</title></head><body><main><p>{}</p>
            <img src="/lead.jpg" width="1200" height="800"></main></body></html>"#,
            "Words a script drew. ".repeat(100)
        );
        let url = Url::parse("https://a.test/after").unwrap();
        let rendered = from_html("https://a.test/", Tier::Headless, &url, 200, &html, true);
        let line = &rendered.line;
        assert_eq!(
            (line.status, line.tier, line.access, line.http_status),
            (
                Status::Ok,
                Some(Tier::Headless),
                Some(Access::Public),
                Some(200)
            )
        );
        assert_eq!(line.url, "https://a.test/");
        assert_eq!(line.final_url.as_deref(), Some("https://a.test/after"));
        assert_eq!(line.lang.as_deref(), Some("en"));
        assert!(
            rendered
                .page
                .is_some_and(|page| page.chars >= extract::ENOUGH)
        );
        let Found::Candidates(found) = rendered.images else {
            panic!("a rendered page names its images");
        };
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].url.as_str(), found[0].source),
            ("https://a.test/lead.jpg", Source::RenderedImg)
        );
        let served = from_html("https://a.test/", Tier::Web, &url, 200, &html, true);
        assert_eq!(served.line.tier, Some(Tier::Web));
        assert!(
            matches!(served.images, Found::Candidates(found) if found[0].source == Source::BodyImg)
        );
    }

    #[test]
    fn an_image_is_media_and_other_files_are_not_html() {
        for mime in ["image/jpeg", "image/png"] {
            assert_eq!(not_html(mime), (Status::Media, format!("image ({mime})")));
        }
        for mime in ["application/x-sh", "video/mp4", "text/plain"] {
            assert_eq!(
                not_html(mime),
                (Status::NotHtml, format!("not HTML ({mime})"))
            );
        }
    }

    #[test]
    fn not_html_lines_of_types_this_build_reads_are_outdated() {
        let line = |status, reason: &str| {
            public_line("https://a.test/", Tier::Web, status).with_reason(reason)
        };
        for mime in ["image/jpeg", "image/png", "application/pdf"] {
            let earlier = line(Status::NotHtml, &format!("not HTML ({mime})"));
            assert!(outdated(&earlier), "{mime}");
            let mut rendered = earlier.clone();
            rendered.tier = Some(Tier::Headless);
            assert!(!outdated(&rendered), "{mime}");
        }
        for (status, reason) in [
            not_html("application/x-sh"),
            not_html("image/jpeg"),
            (Status::NotHtml, "unreadable PDF".to_owned()),
            (Status::NotHtml, "PDF without text".to_owned()),
            (Status::Ok, "not HTML (image/jpeg)".to_owned()),
        ] {
            assert!(!outdated(&line(status, &reason)), "{status:?} {reason}");
        }
    }

    #[test]
    fn a_page_failing_on_three_runs_becomes_unavailable() {
        let error = Line::new("https://a.test/", Status::Error).with_reason("HTTP 503");
        assert_eq!(next_attempt::<Line>(None), 1);
        let first = settle(error.clone(), next_attempt::<Line>(None));
        assert_eq!((first.status, first.attempt), (Status::Error, 1));
        let second = settle(error.clone(), next_attempt(Some(&first)));
        assert_eq!((second.status, second.attempt), (Status::Error, 2));
        let third = settle(error.clone(), next_attempt(Some(&second)));
        assert_eq!((third.status, third.attempt), (Status::Unavailable, 3));
        assert_eq!(third.reason.as_deref(), Some("HTTP 503; failed on 3 runs"));
        assert_eq!(next_attempt(Some(&third)), 1, "a refetch starts again");
        let ok = settle(Line::new("https://a.test/", Status::Ok), 3);
        assert_eq!((ok.status, ok.attempt), (Status::Ok, 3));
    }
}
