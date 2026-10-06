//! `knowmoretabs enrich`: what to fetch, in what order, and the report.
//!
//! slice: enrich
//! why: Enrich sends the URLs you visited to their own sites, so it plans
//!      from the shared `targets` rules before any request and says in full
//!      with `--dry-run` what it would send and what stays home. Hosts run
//!      in parallel on a few workers while each host sees one request a
//!      second, and every result is appended as it arrives so an interrupted
//!      run keeps what it already fetched.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use serde::Serialize;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::jsonl::Appender;
use crate::library::{self, State};
use crate::metadata;
use crate::metadata_fetch;
use crate::metadata_writer::{Line, Outcome};
use crate::out;
use crate::targets::{self, Options, Plan, Recorded, Why};
use crate::triage::plural;

/// A progress line every this many pages, on stderr.
const PROGRESS_EVERY: usize = 25;

/// What a run did.
#[derive(Debug, Default, Serialize)]
struct Tally {
    fetched: usize,
    ok: usize,
    behind_login: usize,
    skipped: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
}

impl Tally {
    fn add(&mut self, line: &Line) {
        self.fetched += 1;
        let reason = || line.reason.clone().unwrap_or_default();
        match line.status {
            Outcome::Ok => self.ok += 1,
            Outcome::BehindLogin => self.behind_login += 1,
            Outcome::Skipped => *self.skipped.entry(reason()).or_default() += 1,
            Outcome::Error => *self.errors.entry(reason()).or_default() += 1,
        }
    }
}

pub fn command(root: &Path, options: Options, json: bool, log: Log) -> Result<(), Error> {
    let archive = Archive::at(root);
    let loaded = library::load(&archive)?;
    let state = State::read(root)?;
    let known = metadata::read(root)?;
    if let Some(note) = known.unreadable_note(&metadata::path(root)) {
        log.warn(&note);
    }
    let plan = targets::plan(
        &loaded.snapshots,
        &state,
        |url| {
            if known.pages.contains_key(url) {
                Recorded::Final
            } else {
                Recorded::Nothing
            }
        },
        options,
    );
    if options.dry_run {
        targets::report_dry_run(&plan, options, json, log, plan.seconds_at_least());
        return Ok(());
    }
    let started = Instant::now();
    let tally = if plan.todo.is_empty() {
        Tally::default()
    } else {
        run(root, &plan, &state, log)?
    };
    let seconds = started.elapsed().as_secs_f64();
    report_run(&plan, &tally, seconds, &metadata::path(root), json, log);
    Ok(())
}

/// Fetches the plan: login pages recorded at once, then each host's pages
/// in order on one of the workers, busiest hosts first so the longest queue
/// starts earliest.
fn run(root: &Path, plan: &Plan, state: &State, log: Log) -> Result<Tally, Error> {
    let appender = Mutex::new(Appender::open(root, metadata::path(root))?);
    let tally = Mutex::new(Tally::default());
    let record = |record: &Line| -> Result<(), Error> {
        appender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .append(record)?;
        tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(record);
        log.note(&format!(
            "{} {}{}",
            status_word(record.status),
            record.url,
            record
                .reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ));
        Ok(())
    };
    let mut pages = Vec::new();
    for item in &plan.todo {
        if item.why == Why::Login {
            record(
                &Line::new(&item.url, Outcome::BehindLogin).with_reason("login page, not fetched"),
            )?;
        } else {
            pages.push((item.host.as_str(), item));
        }
    }
    let fetcher = Fetcher::new(&state.forgotten);
    targets::by_host(
        pages,
        |item| record(&metadata_fetch::fetch(&fetcher, &item.url)),
        |n, total| {
            if n.is_multiple_of(PROGRESS_EVERY) && n < total {
                log.progress(&format!("fetched {n} of {total}"));
            }
        },
    )?;
    Ok(tally.into_inner().unwrap_or_else(PoisonError::into_inner))
}

fn status_word(status: Outcome) -> &'static str {
    match status {
        Outcome::Ok => "ok",
        Outcome::BehindLogin => "behind_login",
        Outcome::Skipped => "skipped",
        Outcome::Error => "error",
    }
}

fn report_run(plan: &Plan, tally: &Tally, seconds: f64, path: &Path, json: bool, log: Log) {
    if json {
        out::json(&serde_json::json!({
            "fetched": tally.fetched,
            "ok": tally.ok,
            "behind_login": tally.behind_login,
            "skipped": tally.skipped,
            "errors": tally.errors,
            "not_fetched": targets::counts_json(plan)["not_fetched"],
            "more": plan.more,
            "seconds": (seconds * 10.0).round() / 10.0,
            "path": path,
        }));
        return;
    }
    if log.quiet {
        return;
    }
    let mut text = String::new();
    if tally.fetched == 0 {
        let _ = writeln!(text, "nothing to fetch");
    } else {
        let skipped: usize = tally.skipped.values().sum();
        let errors: usize = tally.errors.values().sum();
        let _ = writeln!(
            text,
            "enriched {} in {} s: {} ok, {} behind a login, {} skipped, {}; appended to {}",
            plural(tally.fetched, "page"),
            seconds.round(),
            tally.ok,
            tally.behind_login,
            skipped,
            plural(errors, "error"),
            path.display()
        );
        for (label, counts) in [("skipped", &tally.skipped), ("errors", &tally.errors)] {
            if !counts.is_empty() {
                let _ = writeln!(text, "  {label}: {}", targets::breakdown(counts));
            }
        }
    }
    let _ = writeln!(text, "{}", targets::not_fetched_line(plan));
    if plan.more > 0 {
        let _ = writeln!(text, "{} left for another run", plural(plan.more, "page"));
    }
    out::block(&text);
}
