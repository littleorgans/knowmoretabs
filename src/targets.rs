//! The library pages a network command may touch, in the order it should
//! touch them, and why each of the rest stays home; and the workers that
//! touch them, one host at a time per worker.
//!
//! slice: enrich, content
//! why: The decision of what never leaves the machine is made once, before
//!      any request, and can be read in full with `--dry-run`: forgotten
//!      pages, the private network, search results and URLs that carry a
//!      token stay home, and sign-in screens are recorded without a request.
//!      Every network command plans from these same rules, so a page one of
//!      them refuses is refused by all of them for the same stated reason,
//!      and runs its plan on the same workers, so no command can send a
//!      host more than the shared pacing allows.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use url::Url;

use crate::capture::Log;
use crate::error::Error;
use crate::guard::{self, carries_token, is_search_results};
use crate::library::{self, State};
use crate::model::Snapshot;
use crate::out;
use crate::triage::plural;

/// Hosts fetched at once. Each host still gets one request a second.
pub const WORKERS: usize = 8;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub dry_run: bool,
    pub limit: Option<usize>,
    pub refetch: bool,
}

/// Why a library page is not fetched at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotWeb,
    Forgotten,
    PrivateNetwork,
    SearchResults,
    Token,
    /// A profile, channel or playlist on a site whose documents are posts
    /// and videos.
    NotADocument,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotWeb => "not a web page",
            Self::Forgotten => "forgotten",
            Self::PrivateNetwork => "private network",
            Self::SearchResults => "search results",
            Self::Token => "token in URL",
            Self::NotADocument => "not a document",
        }
    }
}

/// Why a page is on the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    New,
    Refetch,
    /// The last attempt failed in a way worth trying again.
    Retry,
    /// A sign-in, sign-up or verification screen: recorded, never fetched.
    Login,
    /// Read as thin or empty over HTTP before: rendered in a browser, with
    /// no new HTTP read.
    Render,
}

impl Why {
    pub fn label(&self) -> String {
        match self {
            Self::New => "new".to_owned(),
            Self::Refetch => "refetch".to_owned(),
            Self::Retry => "retry after an error".to_owned(),
            Self::Login => "login page: recorded as behind_login, not fetched".to_owned(),
            Self::Render => "render in a browser".to_owned(),
        }
    }
}

#[derive(Debug)]
pub struct Item {
    pub url: String,
    pub host: String,
    pub why: Why,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub todo: Vec<Item>,
    /// On the list but past `--limit`.
    pub more: usize,
    pub not_fetched: Vec<(String, Skip)>,
    pub already_fetched: usize,
}

impl Plan {
    pub fn sites(&self) -> usize {
        self.todo
            .iter()
            .filter(|item| item.why != Why::Login)
            .map(|item| item.host.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    /// A lower bound: the busiest host's pages, one a second.
    pub fn seconds_at_least(&self) -> usize {
        seconds_at_least(
            self.todo
                .iter()
                .filter(|item| item.why != Why::Login)
                .map(|item| (item.host.as_str(), Cost::Paced)),
        )
    }

    pub fn skip_counts(&self) -> BTreeMap<Skip, usize> {
        let mut counts = BTreeMap::new();
        for (_, skip) in &self.not_fetched {
            *counts.entry(*skip).or_default() += 1;
        }
        counts
    }
}

/// The waiting one fetch adds to its host's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    /// A request paced once a second; the first request starts immediately.
    Paced,
    /// Waiting inside a tool call, including the first call; zero for gh.
    Tool(usize),
}

/// The busiest host bounds the run. Only the first paced request is free;
/// a tool's internal waits apply to every fetch, including the first.
pub fn seconds_at_least<'a>(fetches: impl Iterator<Item = (&'a str, Cost)>) -> usize {
    let mut per_host: HashMap<&str, (usize, bool)> = HashMap::new();
    for (host, cost) in fetches {
        let (seconds, paced) = per_host.entry(host).or_default();
        match cost {
            Cost::Paced => {
                *seconds += 1;
                *paced = true;
            }
            Cost::Tool(wait) => *seconds += wait,
        }
    }
    per_host
        .values()
        .map(|(seconds, paced)| seconds.saturating_sub(usize::from(*paced)))
        .max()
        .unwrap_or(0)
}

/// What a command's log already says about a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Nothing,
    /// An outcome that stands until the owner asks to fetch again.
    Final,
    /// A failure the command retries on its next run.
    Failed,
    /// An outcome a browser may improve on: rendered in one.
    Render,
    /// An outcome an older build stopped short of what this build keeps:
    /// fetched again without `--refetch`.
    Outdated,
}

/// Library pages newest sighting first, excluding prior attempts unless
/// the owner explicitly asks to fetch them again. `recorded` says what the
/// command's log holds for a URL.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    recorded: impl Fn(&str) -> Recorded,
    options: Options,
) -> Plan {
    let library = library::known_urls(snapshots);
    let mut seen = HashSet::new();
    let mut plan = Plan::default();
    let tabs = snapshots.iter().rev().flat_map(|s| s.tabs.iter());
    for tab in tabs {
        let raw = tab.url.as_str();
        if !library.contains(raw) || !seen.insert(raw) {
            continue;
        }
        let parsed = Url::parse(raw).ok().filter(guard::is_web);
        let skip = match &parsed {
            None => Some(Skip::NotWeb),
            Some(_) if state.forgotten.contains(raw) => Some(Skip::Forgotten),
            Some(url) => skip_reason(url),
        };
        if let Some(skip) = skip {
            plan.not_fetched.push((raw.to_owned(), skip));
            continue;
        }
        let Some(url) = parsed else { continue };
        let recorded = recorded(raw);
        if recorded == Recorded::Final && !options.refetch {
            plan.already_fetched += 1;
            continue;
        }
        let why = if guard::is_login_page(&url) {
            Why::Login
        } else {
            match recorded {
                Recorded::Nothing => Why::New,
                Recorded::Final | Recorded::Outdated => Why::Refetch,
                Recorded::Failed => Why::Retry,
                Recorded::Render => Why::Render,
            }
        };
        let item = Item {
            url: raw.to_owned(),
            host: url.host_str().unwrap_or("").to_ascii_lowercase(),
            why,
        };
        plan.todo.push(item);
    }
    if let Some(limit) = options.limit {
        plan.more = plan.todo.len().saturating_sub(limit);
        plan.todo.truncate(limit);
    }
    plan
}

/// Every forgotten page by the address sent for it: a page opened at
/// another fragment of a forgotten page is forgotten too.
pub struct Forgotten(HashSet<Url>);

impl Forgotten {
    pub fn of(state: &State) -> Self {
        Self(
            state
                .forgotten
                .iter()
                .filter_map(|raw| guard::page_url(raw))
                .collect(),
        )
    }

    pub fn covers(&self, raw: &str) -> bool {
        guard::page_url(raw).is_some_and(|url| self.0.contains(&url))
    }
}

/// The rules for a web page on a public-looking host.
fn skip_reason(url: &Url) -> Option<Skip> {
    if guard::is_private_host(url) {
        Some(Skip::PrivateNetwork)
    } else if is_search_results(url) {
        Some(Skip::SearchResults)
    } else if carries_token(url) {
        Some(Skip::Token)
    } else {
        None
    }
}

/// Works through `items`, each host's in order on one of `workers` threads
/// and hosts side by side, busiest host first so the longest queue
/// starts earliest. The first error `work` returns stops every worker and is
/// returned; `progress` hears how many items of how many are done.
pub fn by_host<'a, T: Send>(
    workers: usize,
    items: impl IntoIterator<Item = (&'a str, T)>,
    work: impl Fn(&T) -> Result<(), Error> + Sync,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<(), Error> {
    let mut hosts: Vec<(&str, Vec<T>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (host, item) in items {
        let i = *index.entry(host).or_insert_with(|| {
            hosts.push((host, Vec::new()));
            hosts.len() - 1
        });
        hosts[i].1.push(item);
    }
    hosts.sort_by_key(|(_, items)| std::cmp::Reverse(items.len()));
    let total: usize = hosts.iter().map(|(_, items)| items.len()).sum();
    let queue = Mutex::new(hosts.into_iter().collect::<VecDeque<_>>());
    let done = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let failed: Mutex<Option<Error>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some((_, items)) = next else { break };
                    for item in &items {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Err(err) = work(item) {
                            stop.store(true, Ordering::Relaxed);
                            failed
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .get_or_insert(err);
                            return;
                        }
                        progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    }
                }
            });
        }
    });
    match failed.into_inner().unwrap_or_else(PoisonError::into_inner) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Whether a progress line is due after `n` of `total` items: every
/// `every`, and the last so a run ends on "N of N". A run shorter than
/// `every` stays quiet.
pub fn progress_due(n: usize, total: usize, every: usize) -> bool {
    (n.is_multiple_of(every) || n == total) && total >= every
}

/// What `--dry-run` says: every page that would be fetched and why, every
/// page that would not and why, the counts, and `notes` when there are any.
pub fn report_dry_run(
    plan: &Plan,
    options: Options,
    json: bool,
    log: Log,
    seconds_at_least: usize,
    notes: &[String],
) {
    if json {
        let mut report = serde_json::json!({
            "dry_run": true,
            "fetch": plan.todo.iter().map(|item| serde_json::json!({
                "url": item.url, "why": item.why.label(),
            })).collect::<Vec<_>>(),
            "not_fetched": plan.not_fetched.iter().map(|(url, skip)| serde_json::json!({
                "url": url, "reason": skip.label(),
            })).collect::<Vec<_>>(),
            "counts": counts_json(plan),
            "more": plan.more,
            "sites": plan.sites(),
            "seconds_at_least": seconds_at_least,
        });
        if !notes.is_empty() {
            report["notes"] = serde_json::json!(notes);
        }
        out::json(&report);
        return;
    }
    if log.quiet {
        return;
    }
    let mut text = String::new();
    if plan.todo.is_empty() {
        let _ = writeln!(text, "would fetch nothing");
    } else {
        let _ = writeln!(
            text,
            "would fetch {}{} from {}{}:",
            plural(plan.todo.len(), "page"),
            match options.limit {
                Some(limit) if plan.more > 0 =>
                    format!(" of {} (--limit {limit})", plan.todo.len() + plan.more),
                _ => String::new(),
            },
            plural(plan.sites(), "site"),
            match seconds_at_least {
                0 => String::new(),
                n => format!(", at least {n} s at one request a second per site"),
            }
        );
        for item in &plan.todo {
            let _ = writeln!(text, "  {}  ({})", item.url, item.why.label());
        }
    }
    if !plan.not_fetched.is_empty() {
        let _ = writeln!(
            text,
            "would not fetch {}:",
            plural(plan.not_fetched.len(), "page")
        );
        for (url, skip) in &plan.not_fetched {
            let _ = writeln!(text, "  {url}  ({})", skip.label());
        }
    }
    for note in notes {
        let _ = writeln!(text, "{note}");
    }
    let _ = writeln!(text, "{}", not_fetched_line(plan));
    out::block(&text);
}

pub fn counts_json(plan: &Plan) -> serde_json::Value {
    let mut not_fetched: BTreeMap<&str, usize> = plan
        .skip_counts()
        .into_iter()
        .map(|(skip, n)| (skip.label(), n))
        .collect();
    not_fetched.insert("already fetched", plan.already_fetched);
    serde_json::json!({
        "fetch": plan.todo.len(),
        "not_fetched": not_fetched,
    })
}

/// `not fetched: 412 already fetched, 40 forgotten, ...`
pub fn not_fetched_line(plan: &Plan) -> String {
    let mut parts = vec![format!("{} already fetched", plan.already_fetched)];
    parts.extend(
        plan.skip_counts()
            .into_iter()
            .map(|(skip, n)| format!("{n} {}", skip.label())),
    );
    let mut line = format!("not fetched: {}", parts.join(", "));
    if plan.already_fetched > 0 {
        line.push_str("; --refetch fetches pages again");
    }
    line
}

/// A log's closed set of outcomes, as its reports name them.
pub trait Outcome: Copy + Ord {
    fn word(self) -> &'static str;
}

/// How many pages ended in each outcome, and why those that did not end
/// well ended as they did.
#[derive(Debug)]
pub struct Counts<S> {
    statuses: BTreeMap<S, usize>,
    reasons: BTreeMap<S, BTreeMap<String, usize>>,
}

impl<S> Default for Counts<S> {
    fn default() -> Self {
        Self {
            statuses: BTreeMap::new(),
            reasons: BTreeMap::new(),
        }
    }
}

impl<S: Outcome> Counts<S> {
    /// One page ended in `status`, for `reason` when it did not end well.
    pub fn add(&mut self, status: S, reason: Option<&str>) {
        *self.statuses.entry(status).or_default() += 1;
        if let Some(reason) = reason {
            *self
                .reasons
                .entry(status)
                .or_default()
                .entry(reason.to_owned())
                .or_default() += 1;
        }
    }

    pub fn total(&self) -> usize {
        self.statuses.values().sum()
    }

    /// `3 ok, 1 thin`.
    pub fn summary(&self) -> String {
        self.statuses
            .iter()
            .map(|(status, n)| format!("{n} {}", status.word()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `  thin: 2 short text`, one line per outcome that has reasons.
    pub fn reason_lines(&self) -> Vec<String> {
        self.reasons
            .iter()
            .map(|(status, reasons)| format!("  {}: {}", status.word(), breakdown(reasons)))
            .collect()
    }

    /// The counts by outcome, and the reasons by outcome.
    pub fn json(&self) -> (serde_json::Value, serde_json::Value) {
        let statuses: BTreeMap<&str, usize> = self
            .statuses
            .iter()
            .map(|(status, n)| (status.word(), *n))
            .collect();
        let reasons: BTreeMap<&str, &BTreeMap<String, usize>> = self
            .reasons
            .iter()
            .map(|(status, reasons)| (status.word(), reasons))
            .collect();
        (serde_json::json!(statuses), serde_json::json!(reasons))
    }
}

/// `3 timeout, 1 HTTP 404`, most common first.
pub fn breakdown(counts: &BTreeMap<String, usize>) -> String {
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    sorted
        .iter()
        .map(|(reason, n)| format!("{n} {reason}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_waits_are_included_even_for_the_first_fetch() {
        let waits = crate::ytdlp::SECONDS_AT_LEAST;
        assert_eq!(
            seconds_at_least([("video", Cost::Tool(waits))].into_iter()),
            waits
        );
        assert_eq!(
            seconds_at_least(
                [("video", Cost::Tool(waits)), ("video", Cost::Tool(waits))].into_iter()
            ),
            2 * waits
        );
        assert_eq!(
            seconds_at_least([("video", Cost::Tool(waits)), ("video", Cost::Paced)].into_iter()),
            waits
        );
        assert_eq!(
            seconds_at_least(
                [
                    ("video", Cost::Tool(waits)),
                    ("web", Cost::Paced),
                    ("web", Cost::Paced)
                ]
                .into_iter()
            ),
            waits
        );
    }

    #[test]
    fn progress_ends_on_the_last_item() {
        assert!(progress_due(185, 186, 5));
        assert!(progress_due(186, 186, 5));
        assert!(!progress_due(184, 186, 5));
        assert!(!progress_due(4, 4, 5));
    }

    #[test]
    fn breakdowns_put_the_common_reason_first() {
        let counts: BTreeMap<String, usize> =
            [("HTTP 404".into(), 1), ("timeout".into(), 3)].into();
        assert_eq!(breakdown(&counts), "3 timeout, 1 HTTP 404");
    }
}
