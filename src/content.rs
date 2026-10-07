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
//!      by the run's last line for it.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::archive::Archive;
use crate::browser;
use crate::capture::Log;
use crate::content_fetch::{self, Capture};
use crate::content_headless::{self, Escalated, Headless};
use crate::content_image::{self, Images};
use crate::content_plan::{self, Work};
use crate::content_route::Tools;
use crate::content_store::{self, Line, Status, Store};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::github_api::Readiness;
use crate::image_store;
use crate::library::{self, State};
use crate::model::Snapshot;
use crate::out;
use crate::targets::{self, Counts, Options, Outcome as _, Plan};
use crate::tools::System;
use crate::triage::plural;
use crate::ytdlp;

/// A progress line every this many fetches, on stderr.
const PROGRESS_EVERY: usize = 25;

/// What a run did.
#[derive(Debug, Default)]
struct Tally {
    /// The run's last line for each library page, so a page rendered after
    /// its HTTP read is counted once, as rendered.
    lines: BTreeMap<String, Line>,
    fetches: usize,
    /// How long each fetch took, retries and extraction included.
    durations: Vec<Duration>,
    /// Video pages left for a run with yt-dlp, unrecorded.
    waiting: usize,
    headless: Headless,
    /// How each page's image ended, when images were captured.
    images: Option<Counts<image_store::Status>>,
}

impl Tally {
    /// How many pages ended in each status, and why.
    fn counts(&self) -> Counts<Status> {
        let mut counts = Counts::default();
        for line in self.lines.values() {
            let reason =
                (line.status != Status::Ok).then(|| line.reason.as_deref().unwrap_or_default());
            counts.add(line.status, reason);
        }
        counts
    }
}

/// What a `content` run was asked for.
#[derive(Debug, Clone, Copy)]
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
}

pub fn command(root: &Path, args: Args<'_>, json: bool, log: Log) -> Result<(), Error> {
    let Args {
        options,
        urls,
        no_images,
        browser,
        no_browser,
    } = args;
    let archive = Archive::at(root);
    let loaded = library::load(&archive)?;
    let state = State::read(root)?;
    let known = content_store::read(root)?;
    if let Some(note) = known.unreadable_note(&content_store::log_path(root)) {
        log.warn(&note);
    }
    let snapshots = only(&loaded.snapshots, urls)?;
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
    let rendering = || browser::Readiness::check(&System, browser, no_browser);
    let (plan, work) = content_plan::plan(
        &snapshots, &state, &known, options, github, youtube, rendering,
    );
    let images = if no_images {
        None
    } else {
        Some(content_image::Plan::new(
            root, &snapshots, &state, &work, options, log,
        )?)
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
        targets::report_dry_run(&plan, options, json, log, work.seconds_at_least(), &notes);
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
        run(root, work, &state, images, headless, log)?
    };
    tally.waiting = waiting;
    report(&plan, &tally, &notes, started.elapsed(), root, json, log);
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

/// Records the pages that need no request, then fetches the rest, each
/// host's pages in order on one of the shared workers, each page's image
/// after its text; then renders in a browser the pages that read thin or
/// empty, each page's image after its render; then retries the images
/// that failed before.
fn run(
    root: &Path,
    work: Work,
    state: &State,
    images: Option<content_image::Plan>,
    mut headless: Headless,
    log: Log,
) -> Result<Tally, Error> {
    let store = Mutex::new(Store::open(root)?);
    let images = images
        .map(|plan| Images::open(root, plan, log))
        .transpose()?;
    let tally = Mutex::new(Tally::default());
    let write = |line: Line, page: Option<&content_store::Page>| -> Result<Line, Error> {
        let line = store
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
        write(line, None)?;
    }
    let rendering = work.browser.as_ref().and_then(browser::Readiness::path);
    let escalated = Escalated::new(rendering.is_some());
    let fetcher = Fetcher::new(&state.forgotten);
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
            let started = Instant::now();
            let Capture {
                line,
                page,
                images: found,
            } = fetch
                .route
                .capture(&fetcher, tools, &fetch.url, images.is_some());
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
            images
                .as_ref()
                .map_or(Ok(()), |images| images.after(&fetcher, &lines, &found))
        },
        |n, total| {
            if n.is_multiple_of(PROGRESS_EVERY) && n < total {
                log.progress(&format!("fetched {n} of {total}"));
            }
        },
    )?;
    let (escalated, unrendered) = escalated.into_parts();
    headless.wait(unrendered);
    let mut renders = work.renders;
    renders.extend(escalated);
    if let Some(path) = rendering {
        content_headless::run(
            &renders,
            path,
            &fetcher,
            images.as_ref(),
            write,
            &mut headless,
            log,
        )?;
    }
    if let Some(images) = &images {
        images.retry(&fetcher)?;
    }
    let mut tally = tally.into_inner().unwrap_or_else(PoisonError::into_inner);
    tally.headless = headless;
    tally.images = images.map(Images::counts);
    Ok(tally)
}

/// Seconds to a tenth.
pub fn seconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10.0).round() / 10.0
}

/// The median and the longest of `durations`.
pub fn spread(durations: &[Duration]) -> Option<(Duration, Duration)> {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    Some((*sorted.get(sorted.len() / 2)?, *sorted.last()?))
}

fn report(
    plan: &Plan,
    tally: &Tally,
    notes: &[String],
    took: Duration,
    root: &Path,
    json: bool,
    log: Log,
) {
    let counts = tally.counts();
    let recorded = counts.total();
    let spread = spread(&tally.durations);
    if json {
        let (statuses, reasons) = counts.json();
        let mut report = serde_json::json!({
            "recorded": recorded,
            "fetched": tally.fetches,
            "statuses": statuses,
            "reasons": reasons,
            "deferred": tally.waiting,
            "not_fetched": targets::counts_json(plan)["not_fetched"],
            "more": plan.more,
            "notes": notes,
            "seconds": seconds(took),
            "fetch_seconds": spread.map(|(median, longest)| serde_json::json!({
                "median": seconds(median), "longest": seconds(longest),
            })),
            "headless": tally.headless.json(),
            "log": content_store::log_path(root),
            "dir": content_store::dir(root),
        });
        if let Some(images) = &tally.images {
            report["images"] = content_image::report_json(images, root);
        }
        out::json(&report);
        return;
    }
    if log.quiet {
        return;
    }
    let mut text = String::new();
    if recorded == 0 {
        let _ = writeln!(text, "nothing to capture");
    } else {
        let _ = writeln!(
            text,
            "recorded {} in {} s, {}: {}",
            plural(recorded, "page"),
            took.as_secs_f64().round(),
            match tally.fetches {
                1 => "1 fetch".to_owned(),
                n => format!("{n} fetches"),
            },
            counts.summary(),
        );
        for line in counts.reason_lines() {
            let _ = writeln!(text, "{line}");
        }
        if let Some((median, longest)) = spread {
            let _ = writeln!(
                text,
                "  a fetch took {} s at the median, {} s at the longest",
                seconds(median),
                seconds(longest)
            );
        }
        let _ = writeln!(
            text,
            "text in {}, one line per attempt in {}",
            content_store::dir(root).display(),
            content_store::log_path(root).display()
        );
    }
    if let Some(images) = &tally.images {
        text.push_str(&content_image::report(images, root));
    }
    for line in tally.headless.lines() {
        let _ = writeln!(text, "{line}");
    }
    for note in notes {
        let _ = writeln!(text, "{note}");
    }
    let _ = writeln!(text, "{}", targets::not_fetched_line(plan));
    if plan.more > 0 {
        let _ = writeln!(text, "{} left for another run", plural(plan.more, "page"));
    }
    out::block(&text);
}

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
        run(
            root.path(),
            work,
            &state,
            None,
            Headless::default(),
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
    fn each_page_is_counted_once_by_its_last_line_in_the_run() {
        let mut tally = Tally::default();
        for (url, status) in [
            ("https://a.test/p", Status::Thin),
            ("https://a.test/p#part", Status::Thin),
            ("https://b.test/", Status::Ok),
            ("https://a.test/p", Status::Ok),
            ("https://a.test/p#part", Status::Ok),
        ] {
            tally.lines.insert(url.to_owned(), Line::new(url, status));
        }
        let counts = tally.counts();
        assert_eq!(counts.total(), 3, "one per page, aliases apart");
        assert_eq!(
            counts.summary(),
            "3 ok",
            "the render replaced the HTTP read"
        );
    }

    #[test]
    fn spreads_are_the_median_and_the_longest() {
        let ms = Duration::from_millis;
        assert_eq!(spread(&[]), None);
        assert_eq!(spread(&[ms(5), ms(1), ms(3)]), Some((ms(3), ms(5))));
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
