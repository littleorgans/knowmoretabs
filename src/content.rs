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
//!      Unless `--no-images`, each page's image follows its text.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::archive::Archive;
use crate::capture::Log;
use crate::content_fetch::{self, Capture};
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
    counts: Counts<Status>,
    fetches: usize,
    /// How long each fetch took, retries and extraction included.
    durations: Vec<Duration>,
    /// Video pages left for a run with yt-dlp, unrecorded.
    waiting: usize,
    /// How each page's image ended, when images were captured.
    images: Option<Counts<image_store::Status>>,
}

impl Tally {
    fn add(&mut self, line: &Line) {
        let reason =
            (line.status != Status::Ok).then(|| line.reason.as_deref().unwrap_or_default());
        self.counts.add(line.status, reason);
    }
}

pub fn command(
    root: &Path,
    options: Options,
    urls: &[String],
    no_images: bool,
    json: bool,
    log: Log,
) -> Result<(), Error> {
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
    let (plan, work) = content_plan::plan(&snapshots, &state, &known, options, github, youtube);
    let images = if no_images {
        None
    } else {
        Some(content_image::Plan::new(
            root, &snapshots, &state, &work, options, log,
        )?)
    };
    let mut notes = work.notes();
    if options.dry_run {
        notes.extend(images.as_ref().and_then(content_image::Plan::note));
        targets::report_dry_run(&plan, options, json, log, work.seconds_at_least(), &notes);
        return Ok(());
    }
    let started = Instant::now();
    let waiting = work.waiting;
    let images = images.filter(|images| !images.is_empty());
    let mut tally = if work.fetches.is_empty() && work.unsent.is_empty() && images.is_none() {
        Tally::default()
    } else {
        run(root, work, &state, images, log)?
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
/// after its text, then retries the images that failed before.
fn run(
    root: &Path,
    work: Work,
    state: &State,
    images: Option<content_image::Plan>,
    log: Log,
) -> Result<Tally, Error> {
    let store = Mutex::new(Store::open(root)?);
    let images = images
        .map(|plan| Images::open(root, plan, log))
        .transpose()?;
    let tally = Mutex::new(Tally::default());
    let record = |line: Line, page: Option<&content_store::Page>| -> Result<Status, Error> {
        let line = store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(line, page)?;
        tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(&line);
        log.note(&format!(
            "{} {}{}",
            line.status.word(),
            line.url,
            line.reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ));
        Ok(line.status)
    };
    for line in work.unsent {
        record(line, None)?;
    }
    let fetcher = Fetcher::new(&state.forgotten);
    let tools = Tools {
        gh: work.github.as_ref().and_then(Readiness::gh),
        ytdlp: work.youtube.as_ref().and_then(ytdlp::Readiness::tool),
    };
    targets::by_host(
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
            let mut settled = Vec::with_capacity(fetch.pages.len());
            for (url, attempt) in &fetch.pages {
                let mut line = line.clone();
                line.url.clone_from(url);
                let status = record(content_fetch::settle(line, *attempt), page.as_ref())?;
                settled.push((url.as_str(), status));
            }
            match &images {
                Some(images) => images.after(&fetcher, &settled, &found),
                None => Ok(()),
            }
        },
        |n, total| {
            if n.is_multiple_of(PROGRESS_EVERY) && n < total {
                log.progress(&format!("fetched {n} of {total}"));
            }
        },
    )?;
    if let Some(images) = &images {
        images.retry(&fetcher)?;
    }
    let mut tally = tally.into_inner().unwrap_or_else(PoisonError::into_inner);
    tally.images = images.map(Images::counts);
    Ok(tally)
}

fn seconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10.0).round() / 10.0
}

/// The median and longest fetch.
fn spread(durations: &[Duration]) -> Option<(Duration, Duration)> {
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
    let recorded = tally.counts.total();
    let spread = spread(&tally.durations);
    if json {
        let (statuses, reasons) = tally.counts.json();
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
        let counts = tally.counts.summary();
        let _ = writeln!(
            text,
            "recorded {} in {} s, {}: {counts}",
            plural(recorded, "page"),
            took.as_secs_f64().round(),
            match tally.fetches {
                1 => "1 fetch".to_owned(),
                n => format!("{n} fetches"),
            },
        );
        for line in tally.counts.reason_lines() {
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
    use crate::content_test::{log, no_gh, no_ytdlp, snapshot, state};
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
        );
        assert_eq!(plan.not_fetched.len(), 6);
        assert_eq!(plan.skip_counts()[&Skip::NotWeb], 2);
        assert!(work.fetches.is_empty());
        let root = tempfile::tempdir().unwrap();
        run(root.path(), work, &state, None, Log::default()).unwrap();
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
