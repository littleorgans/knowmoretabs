//! `knowmoretabs content`: which library pages to capture, capturing them,
//! and the report.
//!
//! slice: content
//! why: Content capture sends the URLs you visited to their own sites, so it
//!      plans from the same `targets` rules as `enrich` before any request,
//!      says in full with `--dry-run` what it would send, and records every
//!      page the rules keep home once, with the reason, so the log accounts
//!      for the whole library. A page known by several addresses that differ
//!      only after `#` is fetched once and recorded under each. Hosts run in
//!      parallel while each sees one request a second, and every result is
//!      written as it arrives, so an interrupted run keeps what it captured.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use url::Url;

use crate::archive::Archive;
use crate::capture::Log;
use crate::content_fetch::{self, Capture};
use crate::content_store::{self, Line, Status, Store};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::guard;
use crate::library::{self, State};
use crate::model::Snapshot;
use crate::out;
use crate::targets::{self, Item, Options, Plan, Skip, Why};
use crate::triage::plural;

/// A progress line every this many fetches, on stderr.
const PROGRESS_EVERY: usize = 25;

/// What the run will do beyond the shared plan.
#[derive(Debug, Default)]
struct Work {
    /// One fetch per page, its fragment variants recorded with it.
    fetches: Vec<Fetch>,
    /// Pages recorded without a request: login screens, and pages the rules
    /// keep home that the log does not mention yet.
    unsent: Vec<Line>,
}

#[derive(Debug)]
struct Fetch {
    /// The first library URL of the group; the fetcher drops its fragment.
    url: String,
    host: String,
    /// Every library URL the fetch stands for, with its attempt number.
    pages: Vec<(String, u32)>,
}

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
    let snapshots = only(&loaded.snapshots, urls)?;
    let (plan, work) = plan(&snapshots, &state, &known, options);
    if options.dry_run {
        targets::report_dry_run(&plan, options, json, log);
        return Ok(());
    }
    let started = Instant::now();
    let tally = if work.fetches.is_empty() && work.unsent.is_empty() {
        Tally::default()
    } else {
        run(root, work, &state, log)?
    };
    report(&plan, &tally, started.elapsed(), root, json, log);
    Ok(())
}

/// The snapshots narrowed to `urls`, each of which must be a library page;
/// all of them when `urls` is empty.
fn only<'a>(
    snapshots: &'a [Snapshot],
    urls: &[String],
) -> Result<std::borrow::Cow<'a, [Snapshot]>, Error> {
    if urls.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(snapshots));
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
    Ok(std::borrow::Cow::Owned(
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

/// The shared plan, with `content`'s own rules on top: pages that are not
/// documents stay home, `--limit` counts what is left, fragment variants
/// share a fetch, and pages kept home are recorded once.
fn plan(
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    options: Options,
) -> (Plan, Work) {
    let mut plan = targets::plan(
        snapshots,
        state,
        |url| content_store::recorded(known, url),
        Options {
            limit: None,
            ..options
        },
    );
    let (documents, not_documents): (Vec<Item>, Vec<Item>) = std::mem::take(&mut plan.todo)
        .into_iter()
        .partition(|item| Url::parse(&item.url).map_or(true, |url| !is_not_a_document(&url)));
    plan.todo = documents;
    let mut unsent: Vec<Line> = not_documents
        .iter()
        .map(|item| (item.url.clone(), Skip::NotADocument))
        .chain(
            plan.not_fetched
                .iter()
                .filter(|(url, skip)| *skip != Skip::Forgotten && !known.pages.contains_key(url))
                .cloned(),
        )
        .map(|(url, skip)| Line::new(&url, Status::Skipped).with_reason(skip.label()))
        .collect();
    plan.not_fetched.extend(
        not_documents
            .into_iter()
            .map(|item| (item.url, Skip::NotADocument)),
    );
    if let Some(limit) = options.limit {
        plan.more = plan.todo.len().saturating_sub(limit);
        plan.todo.truncate(limit);
    }
    let attempt = |url: &str| content_fetch::next_attempt(known.pages.get(url));
    let mut fetches: Vec<Fetch> = Vec::new();
    let mut by_page: HashMap<String, usize> = HashMap::new();
    for item in &plan.todo {
        if item.why == Why::Login {
            let line =
                Line::new(&item.url, Status::BehindLogin).with_reason("login page, not fetched");
            unsent.push(content_fetch::settle(line, attempt(&item.url)));
            continue;
        }
        let page = without_fragment(&item.url);
        let pair = (item.url.clone(), attempt(&item.url));
        if let Some(&i) = by_page.get(&page) {
            fetches[i].pages.push(pair);
        } else {
            by_page.insert(page, fetches.len());
            fetches.push(Fetch {
                url: item.url.clone(),
                host: item.host.clone(),
                pages: vec![pair],
            });
        }
    }
    (plan, Work { fetches, unsent })
}

fn without_fragment(raw: &str) -> String {
    raw.split_once('#').map_or(raw, |(page, _)| page).to_owned()
}

/// Pages on X and `YouTube` that list documents rather than being one:
/// profiles, timelines, channels and playlists.
fn is_not_a_document(url: &Url) -> bool {
    let host = guard::host_key(url);
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("mobile."))
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host);
    let mut segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty());
    match host {
        "x.com" | "twitter.com" => {
            let (_, second, third) = (segments.next(), segments.next(), segments.next());
            !(second == Some("status")
                && third.is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit())))
        }
        "youtube.com" => match segments.next() {
            Some(first) => {
                first.starts_with('@') || ["channel", "c", "user", "playlist"].contains(&first)
            }
            None => false,
        },
        _ => false,
    }
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
    targets::by_host(
        work.fetches
            .iter()
            .map(|fetch| (fetch.host.as_str(), fetch)),
        |fetch| {
            let started = Instant::now();
            let Capture { line, page } = content_fetch::capture(&fetcher, &fetch.url);
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

fn report(plan: &Plan, tally: &Tally, took: Duration, root: &Path, json: bool, log: Log) {
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
    let _ = writeln!(text, "{}", targets::not_fetched_line(plan));
    if plan.more > 0 {
        let _ = writeln!(text, "{} left for another run", plural(plan.more, "page"));
    }
    out::block(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn profiles_channels_and_playlists_are_not_documents() {
        for raw in [
            "https://x.com/someone",
            "https://x.com/home",
            "https://twitter.com/someone/lists",
            "https://mobile.twitter.com/someone/status/",
            "https://x.com/someone/status/not-a-number",
            "https://www.youtube.com/@someone",
            "https://www.youtube.com/channel/UC000",
            "https://www.youtube.com/c/Someone",
            "https://www.youtube.com/user/someone",
            "https://www.youtube.com/playlist?list=PL000",
        ] {
            assert!(is_not_a_document(&url(raw)), "{raw}");
        }
        for raw in [
            "https://x.com/someone/status/1234567890",
            "https://twitter.com/someone/status/1234567890/photo/1",
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "https://www.youtube.com/shorts/abc",
            "https://www.youtube.com/",
            "https://example.test/someone",
        ] {
            assert!(!is_not_a_document(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn fragments_are_dropped_for_grouping_only() {
        assert_eq!(without_fragment("https://a.test/p#one"), "https://a.test/p");
        assert_eq!(without_fragment("https://a.test/p"), "https://a.test/p");
        assert_eq!(
            without_fragment("https://a.test/p?q=1#"),
            "https://a.test/p?q=1"
        );
    }

    fn snapshot(urls: &[&str]) -> Snapshot {
        let tabs: Vec<serde_json::Value> = urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                serde_json::json!({"tab_id": i, "window": 1, "position": i, "url": url,
                    "title": "", "pinned": false, "active": false, "group": null,
                    "last_active": null, "window_id": 1})
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": "2026-10-07-090000Z", "captured_at": "2026-10-07T09:00:00Z",
            "source": {"browser": null, "profile": null, "profile_display": null, "path": "/x",
                "file": "session.snss", "sha256": "", "bytes": 0, "saved_at": null,
                "session_started_at": null},
            "stats": {"file_version": 3, "command_table": "session", "commands": 0,
                "commands_by_id": {}, "unknown_commands": 0, "unknown_command_ids": [],
                "malformed_commands": 0, "truncated_bytes": 0, "marker_count": 1, "marker_ok": true,
                "windows": 0, "tabs": 0, "groups": 0, "dropped_tabs": 0,
                "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0, "window_closed": 0},
                "navigation_fallbacks": 0, "groups_without_metadata": 0},
            "windows": [], "groups": [], "tabs": tabs,
        }))
        .unwrap()
    }

    fn state(forgotten: &[&str]) -> State {
        serde_json::from_value(serde_json::json!({"schema_version": 1, "forgotten": forgotten}))
            .unwrap()
    }

    fn log(lines: &[(&str, Status, u32)]) -> content_store::Log {
        let mut log = content_store::Log::default();
        for (url, status, attempt) in lines {
            let mut line = Line::new(url, *status);
            line.attempt = *attempt;
            log.pages.insert((*url).to_owned(), line);
        }
        log
    }

    fn unsent(work: &Work) -> Vec<(&str, Status, &str)> {
        work.unsent
            .iter()
            .map(|line| {
                (
                    line.url.as_str(),
                    line.status,
                    line.reason.as_deref().unwrap_or(""),
                )
            })
            .collect()
    }

    #[test]
    fn fragment_variants_share_one_fetch_and_are_recorded_each() {
        let snapshots = [snapshot(&[
            "https://a.test/p#one",
            "https://b.test/",
            "https://a.test/p",
            "https://a.test/p#two",
        ])];
        let known = log(&[("https://a.test/p#two", Status::Error, 1)]);
        let (plan, work) = plan(&snapshots, &state(&[]), &known, Options::default());
        assert_eq!(plan.todo.len(), 4);
        let fetches: Vec<(&str, Vec<(&str, u32)>)> = work
            .fetches
            .iter()
            .map(|f| {
                (
                    f.url.as_str(),
                    f.pages.iter().map(|(u, n)| (u.as_str(), *n)).collect(),
                )
            })
            .collect();
        assert_eq!(
            fetches,
            [
                (
                    "https://a.test/p#one",
                    vec![
                        ("https://a.test/p#one", 1),
                        ("https://a.test/p", 1),
                        ("https://a.test/p#two", 2)
                    ]
                ),
                ("https://b.test/", vec![("https://b.test/", 1)]),
            ]
        );
        assert_eq!(unsent(&work), []);
    }

    #[test]
    fn pages_kept_home_are_recorded_once_and_forgotten_ones_never() {
        let snapshots = [snapshot(&[
            "https://www.google.com/search?q=tide",
            "https://a.test/reset?token=abc123",
            "http://192.168.1.1/admin",
            "https://forgotten.test/",
            "https://x.com/someone",
            "https://www.youtube.com/@someone",
            "https://a.test/login",
            "https://already.test/search?q=x",
            "https://x.com/someone/status/123",
        ])];
        let known = log(&[("https://already.test/search?q=x", Status::Skipped, 1)]);
        let (plan, work) = plan(
            &snapshots,
            &state(&["https://forgotten.test/"]),
            &known,
            Options::default(),
        );
        let mut lines = unsent(&work);
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                (
                    "http://192.168.1.1/admin",
                    Status::Skipped,
                    "private network"
                ),
                (
                    "https://a.test/login",
                    Status::BehindLogin,
                    "login page, not fetched"
                ),
                (
                    "https://a.test/reset?token=abc123",
                    Status::Skipped,
                    "token in URL"
                ),
                (
                    "https://www.google.com/search?q=tide",
                    Status::Skipped,
                    "search results"
                ),
                (
                    "https://www.youtube.com/@someone",
                    Status::Skipped,
                    "not a document"
                ),
                ("https://x.com/someone", Status::Skipped, "not a document"),
            ]
        );
        let fetched: Vec<&str> = work.fetches.iter().map(|f| f.url.as_str()).collect();
        assert_eq!(fetched, ["https://x.com/someone/status/123"]);
        let counts = plan.skip_counts();
        assert_eq!(counts[&Skip::Forgotten], 1);
        assert_eq!(counts[&Skip::NotADocument], 2);
        assert_eq!(
            counts[&Skip::SearchResults],
            2,
            "recorded or not, it is counted"
        );
    }

    #[test]
    fn errors_are_retried_finals_wait_for_refetch_and_the_limit_counts_documents() {
        let snapshots = [snapshot(&[
            "https://x.com/someone",
            "https://a.test/1",
            "https://a.test/2",
            "https://a.test/3",
            "https://a.test/4",
        ])];
        let known = log(&[
            ("https://a.test/1", Status::Error, 2),
            ("https://a.test/2", Status::Ok, 1),
            ("https://a.test/3", Status::Unavailable, 3),
        ]);
        let limited = Options {
            limit: Some(1),
            ..Options::default()
        };
        let (plan, work) = plan(&snapshots, &state(&[]), &known, limited);
        let todo: Vec<(&str, Why)> = plan
            .todo
            .iter()
            .map(|i| (i.url.as_str(), i.why.clone()))
            .collect();
        assert_eq!(todo, [("https://a.test/1", Why::Retry)]);
        assert_eq!((plan.more, plan.already_fetched), (1, 2));
        assert_eq!(work.fetches[0].pages, [("https://a.test/1".to_owned(), 3)]);

        let refetch = Options {
            refetch: true,
            ..Options::default()
        };
        let (plan, work) = super::plan(&snapshots, &state(&[]), &known, refetch);
        let whys: Vec<Why> = plan.todo.iter().map(|i| i.why.clone()).collect();
        assert_eq!(whys, [Why::Retry, Why::Refetch, Why::Refetch, Why::New]);
        let attempts: Vec<u32> = work.fetches.iter().map(|f| f.pages[0].1).collect();
        assert_eq!(attempts, [3, 1, 1, 1]);
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

    #[test]
    fn spreads_are_the_median_and_the_longest() {
        let ms = Duration::from_millis;
        assert_eq!(spread(&[]), None);
        assert_eq!(spread(&[ms(5), ms(1), ms(3)]), Some((ms(3), ms(5))));
    }
}
