//! `knowmoretabs content`: capturing the planned pages, and the report.
//!
//! slice: content
//! why: Content capture sends the URLs you visited to their own sites, so
//!      it captures exactly what `content_plan` settled before any request,
//!      and `--dry-run` stops there. Hosts run in parallel while each sees
//!      one request a second, and a host whose route allows more (`gh`,
//!      four at a time) is spread over that many lanes. Every result is
//!      written as it arrives, so an interrupted run keeps what it captured,
//!      and the report says how each page ended, how long fetches took,
//!      when GitHub pages were read from the web because `gh` could not,
//!      how many videos wait for yt-dlp, and how each page's image ended.
//!      Unless `--no-images`, each page's image follows its text. Pages
//!      that read thin or empty are rendered in a browser after the HTTP
//!      reads (`content_headless`), and the report counts each page once,
//!      by the run's last line for it. `--signed-in` runs the same render
//!      pass in the owner's own browser over the pages `content_signed_in`
//!      plans, after attaching to it and before writing anything, so a
//!      browser that cannot be reached leaves the archive as it was. A
//!      caller following one page (`add`) is told, as it happens, which tier
//!      reads it, each wait before a retry, and how its text and image
//!      settled, and gets no report: the batch run tells no one. A Retry
//!      (`add --retry`) runs one stage: the text, with an image only for a
//!      page that has none, so a kept image is never fetched again; or only
//!      the retry of a failed image, on the public fetcher, never signed in.
//!      Image Retry neither reads the text log nor opens its file store.
//!      The batch report is `content_report`'s.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::archive::Archive;
use crate::browser::{self, Browser};
use crate::capture::Log;
use crate::content_events::{self, Events, NoOne};
use crate::content_fetch::{self, Capture};
use crate::content_headless::{self, Escalated, Headless, Render};
use crate::content_image::{self, Images};
use crate::content_plan::{self, Work};
use crate::content_report::{Tally, report};
use crate::content_route::Tools;
use crate::content_signed_in;
use crate::content_store::{self, Line, Store, Tier};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::github_api::Readiness;
use crate::library::{self, State};
use crate::model::Snapshot;
use crate::targets::{self, Options, Outcome as _, Plan};
use crate::tools::System;
use crate::ytdlp;

/// A progress line every this many fetches, on stderr.
const PROGRESS_EVERY: usize = 25;

/// What a `content` run was asked for.
#[derive(Clone, Copy)]
pub struct Args<'a> {
    pub options: Options,
    /// Only these library pages; every page when empty.
    pub urls: &'a [String],
    /// Text only: no preview images this run.
    pub no_images: bool,
    /// The browser pages are rendered in: a browser id.
    pub browser: &'a str,
    /// No browser this run: pages that need one wait.
    pub no_browser: bool,
    /// Open the pages a public read could not get in the owner's own
    /// browser, signed in, and nothing else.
    pub signed_in: bool,
    /// Who follows the run, told as it happens instead of a report; no one
    /// for a batch run.
    pub events: Option<&'a Arc<dyn Events>>,
    /// Only this stage, as `add --retry` asks; none for every other run.
    pub retry: Option<Stage>,
}

/// A stage `add --retry` runs again for its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The text as a plain run plans it, and an image only for a page that
    /// has no image line: a kept image is never fetched again.
    Content,
    /// Only the retry of a failed image, from the candidates its line kept,
    /// on the public fetcher.
    Image,
}

impl Args<'_> {
    fn follower(&self) -> &dyn Events {
        self.events.map_or(&NoOne, |events| &**events)
    }

    /// The image work this run does of what `plan` holds.
    fn images(&self, plan: content_image::Plan) -> content_image::Plan {
        if self.retry == Some(Stage::Content) {
            plan.only_new()
        } else {
            plan
        }
    }
}

/// How a run reaches its pages, and who follows it.
struct Reach<'a> {
    fetcher: &'a Fetcher,
    /// How renders get their browser; none renders nothing.
    start: Option<Start>,
    events: &'a dyn Events,
    /// A selected retry stage; image Retry never opens text storage.
    retry: Option<Stage>,
}

/// How a run's renders get their browser.
pub enum Start {
    /// The binary at this path, launched headless when there is a page to
    /// render.
    Launch(PathBuf),
    /// The owner's browser, attached to before the run.
    Attached(Box<Browser>),
}

pub fn command(root: &Path, args: Args<'_>, json: bool, log: Log) -> Result<(), Error> {
    let (options, retry) = (args.options, args.retry);
    let archive = Archive::at(root);
    let loaded = library::load(&archive)?;
    let state = State::read(root)?;
    let known = if retry == Some(Stage::Image) {
        content_store::Log::default()
    } else {
        content_store::read(root)?
    };
    if let Some(note) = known.unreadable_note(&content_store::log_path(root)) {
        log.warn(&note);
    }
    let snapshots = only(&loaded.snapshots, args.urls)?;
    if args.signed_in && retry != Some(Stage::Image) {
        return self::signed_in(root, &snapshots, &state, &known, args, json, log);
    }
    // A dry run sends nothing, so it does not let `gh` ask GitHub whether
    // it is signed in, and runs no tool at all: it looks on `PATH`.
    let github = || {
        if options.dry_run {
            Readiness::assumed(&System)
        } else {
            Readiness::check(&System)
        }
    };
    let youtube = || {
        if options.dry_run {
            ytdlp::Readiness::assumed(&System)
        } else {
            ytdlp::Readiness::check(&System)
        }
    };
    // Looked for on disk, never run, so a dry run asks the same.
    let rendering = || browser::Readiness::check(&System, args.browser, args.no_browser);
    let (plan, work) = if retry == Some(Stage::Image) {
        (Plan::default(), Work::default())
    } else {
        content_plan::plan(
            &snapshots, &state, &known, options, github, youtube, rendering,
        )
    };
    content_events::kept_home(&plan, args.follower());
    let images = if args.no_images {
        None
    } else {
        Some(args.images(content_image::Plan::new(
            root, &snapshots, &state, &work, options, log,
        )?))
    };
    let mut notes = work.notes();
    if options.dry_run {
        notes.extend(content_headless::dry_run_notes(
            work.browser.as_ref(),
            work.renders.len(),
            work.browser_waiting,
            work.reads_the_web(),
        ));
        notes.extend(images.as_ref().and_then(content_image::Plan::note));
        targets::report_dry_run(
            &plan,
            options,
            json,
            log,
            work.seconds_at_least(),
            &notes,
            false,
        );
        return Ok(());
    }
    let started = Instant::now();
    let waiting = work.waiting;
    let headless = Headless::new(work.browser.as_ref(), work.browser_waiting);
    let images = images.filter(|images| !images.is_empty());
    let idle = work.fetches.is_empty() && work.renders.is_empty() && work.unsent.is_empty();
    let mut tally = if idle && images.is_none() {
        Tally {
            headless,
            ..Tally::default()
        }
    } else {
        let start = work
            .browser
            .as_ref()
            .and_then(browser::Readiness::path)
            .map(|path| Start::Launch(path.to_owned()));
        let fetcher = content_events::following(Fetcher::new(&state.forgotten), args.events);
        let reach = Reach {
            fetcher: &fetcher,
            start,
            events: args.follower(),
            retry,
        };
        run(root, work, images, headless, reach, log)?
    };
    tally.waiting = waiting;
    if args.events.is_none() {
        report(&plan, &tally, &notes, started.elapsed(), root, json, log);
    }
    Ok(())
}

/// `content --signed-in`: the pages `content_signed_in` plans, opened one
/// at a time in the owner's browser, which is attached to only when there
/// is a page to open, after the dry run and before anything is written.
/// Images follow only text read signed in, and earlier images are not
/// retried.
fn signed_in(
    root: &Path,
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    args: Args<'_>,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let options = args.options;
    let (plan, work, personal) = content_signed_in::plan(snapshots, state, known, options);
    let images = if args.no_images {
        None
    } else {
        Some(
            args.images(
                content_image::Plan::new(root, snapshots, state, &work, options, log)?
                    .without_retries(),
            ),
        )
    };
    let name = content_signed_in::name(args.browser);
    let mut notes = content_signed_in::ineligible(args.urls, known);
    if options.dry_run {
        notes.extend(content_signed_in::dry_run_notes(
            work.renders.len(),
            personal,
            &name,
        ));
        notes.extend(images.as_ref().and_then(content_image::Plan::note));
        let limit = Some(content_signed_in::limit(options));
        targets::report_dry_run(
            &plan,
            Options { limit, ..options },
            json,
            log,
            0,
            &notes,
            true,
        );
        return Ok(());
    }
    let started = Instant::now();
    let headless = Headless::signed_in(name.clone(), personal);
    let tally = if work.renders.is_empty() {
        Tally {
            headless,
            ..Tally::default()
        }
    } else {
        // Said before attaching: Chrome may wait for the owner to allow it.
        args.follower().reading(Tier::SignedIn);
        let browser = content_signed_in::attach(args.browser, name)?;
        let images = images.filter(|images| !images.is_empty());
        let fetcher = content_events::following(Fetcher::signed_in(&state.forgotten), args.events);
        let reach = Reach {
            fetcher: &fetcher,
            start: Some(Start::Attached(Box::new(browser))),
            events: args.follower(),
            retry: args.retry,
        };
        run(root, work, images, headless, reach, log)?
    };
    if args.events.is_none() {
        report(&plan, &tally, &notes, started.elapsed(), root, json, log);
    }
    Ok(())
}

/// The snapshots narrowed to `urls`, each of which must be a library page;
/// all of them when `urls` is empty.
fn only<'a>(snapshots: &'a [Snapshot], urls: &[String]) -> Result<Cow<'a, [Snapshot]>, Error> {
    if urls.is_empty() {
        return Ok(Cow::Borrowed(snapshots));
    }
    let library = library::known_urls(snapshots);
    let unknown: Vec<String> = urls
        .iter()
        .filter(|url| !library.contains(url.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(Error::NotInLibrary(unknown));
    }
    let wanted: HashSet<&str> = urls.iter().map(String::as_str).collect();
    Ok(Cow::Owned(
        snapshots
            .iter()
            .map(|snapshot| {
                let mut snapshot = snapshot.clone();
                snapshot
                    .tabs
                    .retain(|tab| wanted.contains(tab.url.as_str()));
                snapshot
            })
            .collect(),
    ))
}

/// Records the pages that need no request, then fetches the rest through
/// the fetcher, each host's pages in order on one of the shared workers,
/// each page's image after its text; then renders the pages that read thin
/// or empty in the browser `reach` starts, each page's image after its
/// render; then retries the images that failed before. Each page's text is
/// told to whoever follows once it is final for the run.
fn run(
    root: &Path,
    work: Work,
    images: Option<content_image::Plan>,
    mut headless: Headless,
    reach: Reach<'_>,
    log: Log,
) -> Result<Tally, Error> {
    let (fetcher, events) = (reach.fetcher, reach.events);
    let store = (reach.retry != Some(Stage::Image))
        .then(|| Store::open(root))
        .transpose()?
        .map(Mutex::new);
    let images = images
        .map(|plan| Images::open(root, plan, events, log))
        .transpose()?;
    let tally = Mutex::new(Tally::default());
    let write = |line: Line, page: Option<&content_store::Page>| -> Result<Line, Error> {
        let line = store
            .as_ref()
            .expect("text work opens the content store")
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(line, page)?;
        log.note(&format!(
            "{} {}{}",
            line.status.word(),
            line.url,
            line.reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ));
        tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lines
            .insert(line.url.clone(), line.clone());
        Ok(line)
    };
    for line in work.unsent {
        events.content(&write(line, None)?);
    }
    let escalated = Escalated::new(reach.start.is_some());
    let tools = Tools {
        gh: work.github.as_ref().and_then(Readiness::gh),
        ytdlp: work.youtube.as_ref().and_then(ytdlp::Readiness::tool),
    };
    targets::by_host(
        targets::WORKERS,
        work.fetches
            .iter()
            .map(|fetch| (fetch.host.as_str(), fetch)),
        |fetch| {
            events.reading(fetch.route.tier());
            let started = Instant::now();
            let Capture {
                line,
                page,
                images: found,
            } = fetch
                .route
                .capture(fetcher, tools, &fetch.url, images.is_some());
            {
                let mut tally = tally.lock().unwrap_or_else(PoisonError::into_inner);
                tally.fetches += 1;
                tally.durations.push(started.elapsed());
            }
            let mut lines = Vec::with_capacity(fetch.pages.len());
            for (url, attempt) in &fetch.pages {
                let mut line = line.clone();
                line.url.clone_from(url);
                lines.push(write(content_fetch::settle(line, *attempt), page.as_ref())?);
            }
            // A page rendered after gets its image after the render.
            if escalated.keep(&lines, &found) {
                return Ok(());
            }
            for line in &lines {
                events.content(line);
            }
            images
                .as_ref()
                .map_or(Ok(()), |images| images.after(fetcher, &lines, &found))
        },
        |n, total| {
            if targets::progress_due(n, total, PROGRESS_EVERY) {
                log.progress(&format!("fetched {n} of {total}"));
            }
        },
    )?;
    let (escalated, unrendered) = escalated.into_parts();
    headless.wait(unrendered);
    let mut renders = work.renders;
    renders.extend(escalated);
    render(&renders, reach, images.as_ref(), write, &mut headless, log)?;
    if let Some(images) = &images {
        images.retry(fetcher)?;
    }
    let mut tally = tally.into_inner().unwrap_or_else(PoisonError::into_inner);
    tally.headless = headless;
    tally.images = images.map(Images::counts);
    Ok(tally)
}

/// The second pass, when there are pages and `reach` has a browser to
/// start: renders `renders`, each page's line written through `write` and
/// told to whoever follows.
fn render(
    renders: &[Render],
    reach: Reach<'_>,
    images: Option<&Images>,
    write: impl Fn(Line, Option<&content_store::Page>) -> Result<Line, Error> + Sync,
    headless: &mut Headless,
    log: Log,
) -> Result<(), Error> {
    let Some(start) = reach.start.filter(|_| !renders.is_empty()) else {
        return Ok(());
    };
    // An attached browser was told of before it was reached.
    let browser = match start {
        Start::Launch(path) => {
            reach.events.reading(Tier::Headless);
            Browser::launch(&path, log)
        }
        Start::Attached(browser) => Ok(*browser),
    };
    content_headless::run(
        renders,
        browser,
        reach.fetcher,
        images,
        |line, page| {
            let line = write(line, page)?;
            reach.events.content(&line);
            Ok(line)
        },
        headless,
        log,
    )
}

#[cfg(test)]
#[path = "content_events_tests.rs"]
mod events_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_plan::plan;
    use crate::content_test::{log, no_browser, no_gh, no_ytdlp, snapshot, state};
    use crate::targets::Skip;

    #[test]
    fn shared_skips_never_write_urls_to_the_content_store() {
        let snapshots = [snapshot(&[
            "https://www.google.com/search?q=tide",
            "https://a.test/reset?token=abc123",
            "http://192.168.1.1/admin",
            "https://forgotten.test/",
            "chrome://settings/",
            "data:text/plain,hello",
        ])];
        let state = state(&["https://forgotten.test/"]);
        let (plan, work) = plan(
            &snapshots,
            &state,
            &log(&[]),
            Options::default(),
            no_gh,
            no_ytdlp,
            no_browser,
        );
        assert_eq!(plan.not_fetched.len(), 6);
        assert_eq!(plan.skip_counts()[&Skip::NotWeb], 2);
        assert!(work.fetches.is_empty());
        let root = tempfile::tempdir().unwrap();
        let reach = Reach {
            fetcher: &Fetcher::new(&state.forgotten),
            start: None,
            events: &NoOne,
            retry: None,
        };
        run(
            root.path(),
            work,
            None,
            Headless::default(),
            reach,
            Log::default(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(content_store::log_path(root.path())).unwrap(),
            [] as [u8; 0]
        );
        assert_eq!(
            std::fs::read_dir(content_store::dir(root.path()))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn urls_narrow_the_library_and_must_belong_to_it() {
        let snapshots = [snapshot(&["https://a.test/", "https://b.test/"])];
        let narrowed = only(&snapshots, &["https://b.test/".to_owned()]).unwrap();
        let urls: Vec<&str> = narrowed[0].tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://b.test/"]);
        assert_eq!(only(&snapshots, &[]).unwrap().len(), 1);
        let Err(Error::NotInLibrary(unknown)) = only(
            &snapshots,
            &["https://c.test/".to_owned(), "https://a.test/".to_owned()],
        ) else {
            panic!("an unknown URL is refused");
        };
        assert_eq!(unknown, ["https://c.test/"]);
    }
}
