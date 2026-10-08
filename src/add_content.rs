//! `add`'s content stage: the page's text and image, captured by the
//! content run narrowed to it, and the lines a caller follows it by.
//!
//! slice: add
//! why: A page added to be read should be readable when `add` returns, so
//!      `add` runs content for that one page: the same routes, render and
//!      signed in reader, and the same rules for what is fetched again, so
//!      a rerun refetches only an error and retries only a failed image.
//!      The run tells `add` which tier reads the page, each wait, and how
//!      its text and image settled, and `add` says each as one line. A
//!      stage the run did not settle is said from what the logs hold, so a
//!      known page reports how it stands without a request. Without a log
//!      line the stage completes as unknown, without inventing a capture.
//!      A browser that cannot be reached is the content stage's outcome,
//!      not the run's failure: the page is in the library either way. A
//!      Retry runs only the stages it names, each only where it failed, so
//!      what succeeded stands untouched: the text first, its image only
//!      when it has none, then a failed image on the public fetcher, which
//!      a text that fails again or a browser out of reach cannot skip.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::capture::Log;
use crate::content::{self, Stage};
use crate::content_events::Events;
use crate::content_report;
use crate::content_signed_in::Unavailable;
use crate::content_store::{self, Line, Tier};
use crate::error::Error;
use crate::image_store;
use crate::out;
use crate::targets::{Options, Outcome as _, Recorded};

/// How the content stage was asked to run.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// The browser renders and signed in reads use: a browser id.
    pub browser: &'a str,
    pub signed_in: bool,
    /// The stages a Retry runs again; empty for a plain add.
    pub retry: &'a [Stage],
}

/// How the text and the image ended, by their status names, and the title
/// the text was kept with.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ended {
    pub content: Option<String>,
    pub image: Option<String>,
    pub title: Option<String>,
}

/// Runs content for `url`, a library page, saying each step as it happens:
/// the whole run for a plain add, or the stages a Retry names, the text's
/// then the image's. A run that fails for another reason than the browser
/// is said on stderr, with what the logs hold after it.
pub fn capture(root: &Path, url: &str, ask: Ask<'_>, json: bool, log: Log) -> Ended {
    let follow = Arc::new(Follow {
        root: root.to_owned(),
        url: url.to_owned(),
        json,
        log,
        seen: Mutex::new(Ended::default()),
        imaging: AtomicBool::new(false),
    });
    let events: Arc<dyn Events> = follow.clone();
    let urls = [url.to_owned()];
    let run = |retry: Option<Stage>, signed_in: bool| {
        let args = content::Args {
            options: Options::default(),
            urls: &urls,
            no_images: false,
            browser: ask.browser,
            no_browser: false,
            signed_in,
            events: Some(&events),
            retry,
        };
        match content::command(root, args, json, log) {
            Ok(()) => {}
            Err(Error::SignedIn(why)) => follow.unreached(why),
            Err(err) => out::problem(&format!("knowmoretabs: {err}")),
        }
    };
    if ask.retry.is_empty() {
        run(None, ask.signed_in);
    } else {
        // The signed in plan opens only a page eligible for it. A log that
        // cannot be read lets the run say why.
        let public = || content_store::read(root).map_or(true, |log| retries_text(&log, url));
        if ask.retry.contains(&Stage::Content) && (ask.signed_in || public()) {
            run(Some(Stage::Content), ask.signed_in);
        }
        if ask.retry.contains(&Stage::Image) && follow.seen().image.is_none() {
            run(Some(Stage::Image), false);
        }
    }
    follow.settle_content();
    follow.settle_image();
    follow.seen().clone()
}

/// Whether a content Retry on the public tiers reads `url` again: only
/// when its text failed or was never recorded. Every other line stands,
/// `unavailable` and what a browser may improve on included.
fn retries_text(log: &content_store::Log, url: &str) -> bool {
    matches!(
        content_store::recorded(log, url),
        Recorded::Nothing | Recorded::Failed
    )
}

/// What `add` hears from its content run, said as it happens.
struct Follow {
    root: PathBuf,
    url: String,
    json: bool,
    log: Log,
    seen: Mutex<Ended>,
    /// The image is being fetched: a wait now is the image's.
    imaging: AtomicBool,
}

impl Follow {
    fn seen(&self) -> MutexGuard<'_, Ended> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn say(&self, line: &Value) {
        if self.json {
            out::json(line);
        }
    }

    /// A stage's result, which a person reads too.
    fn tell(&self, json: &Value, text: &str) {
        if self.json {
            out::json(json);
        } else if !self.log.quiet {
            out::line(text);
        }
    }

    /// The text settled as `line` says.
    fn content_line(&self, line: &Line) {
        let title = kept_title(&self.root, &line.url);
        let status = line.status.word();
        self.tell(&content_done(line, title.as_deref()), &content_text(line));
        let mut seen = self.seen();
        seen.content = Some(status.to_owned());
        seen.title = title;
    }

    /// The owner's browser could not be reached: nothing was read.
    fn unreached(&self, why: Unavailable) {
        let line = json!({"stage": "content", "state": "done", "status": why.name(),
            "tier": Tier::SignedIn});
        self.tell(&line, &format!("content not read signed in: {why}"));
        self.seen().content = Some(why.name().to_owned());
    }

    /// Says how the text stands by the content log, unless the run said it.
    fn settle_content(&self) {
        if self.seen().content.is_some() {
            return;
        }
        let latest = content_store::read(&self.root)
            .ok()
            .and_then(|mut log| log.pages.remove(&self.url));
        let line = latest.unwrap_or_else(|| {
            Line::new(&self.url, content_store::Status::Unknown).with_reason("not_recorded")
        });
        self.content_line(&line);
    }

    /// Says how the image stands by the image log, unless the run said it.
    fn settle_image(&self) {
        if self.seen().image.is_some() {
            return;
        }
        let latest = image_store::read(&self.root)
            .ok()
            .and_then(|mut log| log.pages.remove(&self.url));
        let line = latest
            .unwrap_or_else(|| image_store::Line::new(&self.url, image_store::Status::Unknown));
        self.image_line(&line);
    }

    fn image_line(&self, line: &image_store::Line) {
        self.imaging();
        let status = line.status.word();
        let text = match (&line.reason, line.status) {
            (Some(reason), image_store::Status::None | image_store::Status::Error) => {
                format!("image {status} ({reason})")
            }
            _ => format!("image {status}"),
        };
        self.tell(
            &json!({"stage": "image", "state": "done", "status": status}),
            &text,
        );
        self.seen().image = Some(status.to_owned());
    }
}

impl Events for Follow {
    fn reading(&self, tier: Tier) {
        self.imaging.store(false, Ordering::Relaxed);
        self.say(&json!({"stage": "content", "state": "running", "tier": tier}));
    }

    fn retrying(&self, wait: Duration) {
        let stage = if self.imaging.load(Ordering::Relaxed) {
            "image"
        } else {
            "content"
        };
        self.say(&json!({"stage": stage, "state": "retrying",
            "after_s": content_report::seconds(wait)}));
    }

    fn content(&self, line: &Line) {
        self.content_line(line);
    }

    /// The text is settled before its image is fetched, by the log when
    /// the run did not say it: a page whose text stands gets its failed
    /// image retried.
    fn imaging(&self) {
        self.settle_content();
        if !self.imaging.swap(true, Ordering::Relaxed) {
            self.say(&json!({"stage": "image", "state": "running"}));
        }
    }

    fn image(&self, line: &image_store::Line) {
        self.settle_content();
        self.image_line(line);
    }
}

/// The title page `url`'s kept text carries, when it has one.
fn kept_title(root: &Path, url: &str) -> Option<String> {
    let file = content_store::dir(root).join(content_store::file_name(url));
    let text = std::fs::read_to_string(file).ok()?;
    let (front, _) = content_store::parse(&text)?;
    front
        .get("title")?
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn content_done(line: &Line, title: Option<&str>) -> Value {
    let mut done = json!({"stage": "content", "state": "done", "status": line.status.word(),
        "tier": line.tier});
    if let Some(http) = line.http_status {
        done["http_status"] = http.into();
    }
    if let Some(reason) = &line.reason {
        done["reason"] = reason.as_str().into();
    }
    if let Some(title) = title {
        done["title"] = title.into();
    }
    done
}

fn content_text(line: &Line) -> String {
    let mut text = format!("content {}", line.status.word());
    if let Some(Value::String(tier)) = line.tier.map(|tier| json!(tier)) {
        let _ = write!(text, " by {tier}");
    }
    if let Some(reason) = &line.reason {
        let _ = write!(text, " ({reason})");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_pdf;
    use crate::content_store::Status;
    use crate::content_test::log;

    const URL: &str = "https://a.test/one";

    #[test]
    fn a_public_text_retry_reads_only_an_error_or_no_line() {
        assert!(retries_text(&log(&[]), URL), "no line: a first capture");
        for status in [
            Status::Ok,
            Status::Thin,
            Status::Media,
            Status::EmptyShell,
            Status::BehindLogin,
            Status::Paywalled,
            Status::Blocked,
            Status::NotFound,
            Status::NotHtml,
            Status::Skipped,
            Status::Error,
            Status::Unavailable,
            Status::Unknown,
        ] {
            let ran = retries_text(&log(&[(URL, status, 1)]), URL);
            assert_eq!(ran, status == Status::Error, "{status:?}");
        }
        let mut outdated = content_store::Log::default();
        let mut line = Line::new(URL, Status::NotHtml)
            .with_reason(format!("not HTML ({})", content_pdf::MIME));
        line.tier = Some(Tier::Web);
        outdated.pages.insert(URL.to_owned(), line);
        assert_eq!(
            content_store::recorded(&outdated, URL),
            Recorded::Outdated,
            "the fixture is outdated"
        );
        assert!(!retries_text(&outdated, URL), "an outdated line stands");
    }
}
