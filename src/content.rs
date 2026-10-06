//! `knowmoretabs content`: capturing the planned pages, and the report.
//!
//! slice: content
//! why: Content capture sends the URLs you visited to their own sites, so
//!      it captures exactly what `content_plan` settled before any request,
//!      and `--dry-run` stops there. Hosts run in parallel while each sees
//!      one request a second, and a host whose route allows more (`gh`,
//!      four at a time) is spread over that many lanes. Every result is
//!      written as it arrives, so an interrupted run keeps what it captured,
//!      and the report says how each page ended, how long fetches took, and
//!      when GitHub pages were read from the web because `gh` could not.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::archive::Archive;
use crate::capture::Log;
use crate::content_fetch::{self, Capture};
use crate::content_plan::{self, Work};
use crate::content_store::{self, Line, Status, Store};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::github_api::Readiness;
use crate::library::{self, State};
use crate::out;
use crate::targets::{self, Options, Plan};
use crate::tools::System;
use crate::triage::plural;

/// A progress line every this many fetches, on stderr.
const PROGRESS_EVERY: usize = 25;

/// What a run did.
#[derive(Debug, Default)]
struct Tally {
    statuses: BTreeMap<Status, usize>,
    reasons: BTreeMap<Status, BTreeMap<String, usize>>,
    fetches: usize,
    /// How long each fetch took, retries and extraction included.
    durations: Vec<Duration>,
}

impl Tally {
    fn add(&mut self, line: &Line) {
        *self.statuses.entry(line.status).or_default() += 1;
        if !matches!(line.status, Status::Ok) {
            let reason = line.reason.clone().unwrap_or_default();
            *self
                .reasons
                .entry(line.status)
                .or_default()
                .entry(reason)
                .or_default() += 1;
        }
    }
}

pub fn command(
    root: &Path,
    options: Options,
    urls: &[String],
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
    let snapshots = content_plan::only(&loaded.snapshots, urls)?;
    // A dry run sends nothing, so it does not let `gh` ask GitHub whether
    // it is signed in.
    let github = || {
        if options.dry_run {
            Readiness::assumed(&System)
        } else {
            Readiness::check(&System)
        }
    };
    let (plan, work) = content_plan::plan(&snapshots, &state, &known, options, github);
    if options.dry_run {
        targets::report_dry_run(&plan, options, json, log, work.seconds_at_least());
        return Ok(());
    }
    let started = Instant::now();
    let notes = work.notes();
    let tally = if work.fetches.is_empty() && work.unsent.is_empty() {
        Tally::default()
    } else {
        run(root, work, &state, log)?
    };
    report(&plan, &tally, &notes, started.elapsed(), root, json, log);
    Ok(())
}

/// Records the pages that need no request, then fetches the rest, each
/// host's pages in order on one of the shared workers.
fn run(root: &Path, work: Work, state: &State, log: Log) -> Result<Tally, Error> {
    let store = Mutex::new(Store::open(root)?);
    let tally = Mutex::new(Tally::default());
    let record = |line: Line, page: Option<&content_store::Page>| -> Result<(), Error> {
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
        Ok(())
    };
    for line in work.unsent {
        record(line, None)?;
    }
    let fetcher = Fetcher::new(&state.forgotten);
    let gh = work.github.as_ref().and_then(Readiness::gh);
    targets::by_host(
        work.fetches
            .iter()
            .map(|fetch| (fetch.host.as_str(), fetch)),
        |fetch| {
            let started = Instant::now();
            let Capture { line, page } = fetch.route.capture(&fetcher, gh, &fetch.url);
            {
                let mut tally = tally.lock().unwrap_or_else(PoisonError::into_inner);
                tally.fetches += 1;
                tally.durations.push(started.elapsed());
            }
            for (url, attempt) in &fetch.pages {
                let mut line = line.clone();
                line.url.clone_from(url);
                record(content_fetch::settle(line, *attempt), page.as_ref())?;
            }
            Ok(())
        },
        |n, total| {
            if n.is_multiple_of(PROGRESS_EVERY) && n < total {
                log.progress(&format!("fetched {n} of {total}"));
            }
        },
    )?;
    Ok(tally.into_inner().unwrap_or_else(PoisonError::into_inner))
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
    let recorded: usize = tally.statuses.values().sum();
    let spread = spread(&tally.durations);
    if json {
        let words = |counts: &BTreeMap<Status, usize>| -> BTreeMap<&str, usize> {
            counts
                .iter()
                .map(|(status, n)| (status.word(), *n))
                .collect()
        };
        out::json(&serde_json::json!({
            "recorded": recorded,
            "fetched": tally.fetches,
            "statuses": words(&tally.statuses),
            "reasons": tally.reasons.iter().map(|(status, reasons)| (status.word(), reasons)).collect::<BTreeMap<_, _>>(),
            "deferred": 0,
            "not_fetched": targets::counts_json(plan)["not_fetched"],
            "more": plan.more,
            "notes": notes,
            "seconds": seconds(took),
            "fetch_seconds": spread.map(|(median, longest)| serde_json::json!({
                "median": seconds(median), "longest": seconds(longest),
            })),
            "log": content_store::log_path(root),
            "dir": content_store::dir(root),
        }));
        return;
    }
    if log.quiet {
        return;
    }
    let mut text = String::new();
    if recorded == 0 {
        let _ = writeln!(text, "nothing to capture");
    } else {
        let counts = tally
            .statuses
            .iter()
            .map(|(status, n)| format!("{n} {}", status.word()))
            .collect::<Vec<_>>()
            .join(", ");
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
        for (status, reasons) in &tally.reasons {
            let _ = writeln!(text, "  {}: {}", status.word(), targets::breakdown(reasons));
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
    use crate::content_plan::tests::{log, no_gh, snapshot, state};
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
        let (plan, work) = plan(&snapshots, &state, &log(&[]), Options::default(), no_gh);
        assert_eq!(plan.not_fetched.len(), 6);
        assert_eq!(plan.skip_counts()[&Skip::NotWeb], 2);
        assert!(work.fetches.is_empty());
        let root = tempfile::tempdir().unwrap();
        run(root.path(), work, &state, Log::default()).unwrap();
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
}
