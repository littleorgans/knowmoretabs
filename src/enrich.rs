//! `knowmoretabs enrich`: what to fetch, in what order, and the report.
//!
//! slice: library
//! why: Enrich is the one command that sends anything anywhere, so the
//!      decision of what never leaves is made once, before any request, and
//!      can be read in full with `--dry-run`: forgotten pages, the private
//!      network, search results and URLs that carry a token stay home.
//!      Hosts run in parallel on a few workers while each host sees one
//!      request a second, and every result is appended as it arrives so an
//!      interrupted run keeps what it already fetched.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use serde::Serialize;
use url::Url;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::fetch::{self, Fetcher};
use crate::library::{self, State};
use crate::metadata::{self, Metadata, Status};
use crate::metadata_writer::{Appender, Line, Outcome};
use crate::model::Snapshot;
use crate::out;
use crate::triage::plural;

/// Hosts fetched at once. Each host still gets one request a second.
const WORKERS: usize = 8;
/// A progress line every this many pages, on stderr.
const PROGRESS_EVERY: usize = 25;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub dry_run: bool,
    pub limit: Option<usize>,
    pub refetch: bool,
}

/// Why a library page is not fetched at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Skip {
    NotWeb,
    Forgotten,
    PrivateNetwork,
    SearchResults,
    Token,
}

impl Skip {
    fn label(self) -> &'static str {
        match self {
            Self::NotWeb => "not a web page",
            Self::Forgotten => "forgotten",
            Self::PrivateNetwork => "private network",
            Self::SearchResults => "search results",
            Self::Token => "token in URL",
        }
    }
}

/// Why a page is on the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Why {
    New,
    /// The last attempt failed; its reason.
    Retry(String),
    Refetch,
    /// A sign-in, sign-up or verification screen: recorded, never fetched.
    Login,
}

impl Why {
    fn label(&self) -> String {
        match self {
            Self::New => "new".to_owned(),
            Self::Retry(reason) => format!("retry, last time: {reason}"),
            Self::Refetch => "refetch".to_owned(),
            Self::Login => "login page: recorded as behind_login, not fetched".to_owned(),
        }
    }
}

#[derive(Debug)]
struct Item {
    url: String,
    host: String,
    why: Why,
}

#[derive(Debug, Default)]
struct Plan {
    todo: Vec<Item>,
    /// On the list but past `--limit`.
    more: usize,
    not_fetched: Vec<(String, Skip)>,
    already_fetched: usize,
}

impl Plan {
    fn sites(&self) -> usize {
        self.todo
            .iter()
            .filter(|item| item.why != Why::Login)
            .map(|item| item.host.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    /// A lower bound: the busiest host's pages, one a second.
    fn seconds_at_least(&self) -> usize {
        let mut per_host: HashMap<&str, usize> = HashMap::new();
        for item in self.todo.iter().filter(|item| item.why != Why::Login) {
            *per_host.entry(&item.host).or_default() += 1;
        }
        per_host.values().max().map_or(0, |n| n.saturating_sub(1))
    }

    fn skip_counts(&self) -> BTreeMap<Skip, usize> {
        let mut counts = BTreeMap::new();
        for (_, skip) in &self.not_fetched {
            *counts.entry(*skip).or_default() += 1;
        }
        counts
    }
}

/// Library pages newest sighting first, then the rules, then what is
/// already known. New pages come before retries, so that `--limit` spends
/// its budget on pages never tried.
fn plan(snapshots: &[Snapshot], state: &State, known: &Metadata, options: Options) -> Plan {
    let library = library::known_urls(snapshots);
    let mut seen = HashSet::new();
    let mut plan = Plan::default();
    let (mut fresh, mut retries) = (Vec::new(), Vec::new());
    let tabs = snapshots.iter().rev().flat_map(|s| s.tabs.iter());
    for tab in tabs {
        let raw = tab.url.as_str();
        if !library.contains(raw) || !seen.insert(raw) {
            continue;
        }
        let parsed = Url::parse(raw).ok().filter(fetch::is_web);
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
        let why = match known.pages.get(raw) {
            Some(record) if !options.refetch && record.status != Status::Error => {
                plan.already_fetched += 1;
                continue;
            }
            _ if fetch::is_login_page(&url) => Why::Login,
            Some(_) if options.refetch => Why::Refetch,
            Some(record) => Why::Retry(record.reason.clone().unwrap_or_default()),
            None => Why::New,
        };
        let item = Item {
            url: raw.to_owned(),
            host: url.host_str().unwrap_or("").to_ascii_lowercase(),
            why,
        };
        if matches!(item.why, Why::Retry(_)) {
            retries.push(item);
        } else {
            fresh.push(item);
        }
    }
    plan.todo = fresh;
    plan.todo.append(&mut retries);
    if let Some(limit) = options.limit {
        plan.more = plan.todo.len().saturating_sub(limit);
        plan.todo.truncate(limit);
    }
    plan
}

/// The rules for a web page on a public-looking host.
fn skip_reason(url: &Url) -> Option<Skip> {
    if fetch::is_private_host(url) {
        Some(Skip::PrivateNetwork)
    } else if is_search_results(url) {
        Some(Skip::SearchResults)
    } else if carries_token(url) {
        Some(Skip::Token)
    } else {
        None
    }
}

/// A results page says nothing the query in its URL does not.
fn is_search_results(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = url.path().to_ascii_lowercase();
    let has = |key: &str| url.query_pairs().any(|(k, v)| k == key && !v.is_empty());
    let engine = |name: &str| {
        host == name || host.starts_with(&format!("{name}.")) || host.ends_with(&format!(".{name}"))
    };
    if engine("google") && ["/search", "/url", "/imgres", "/webhp"].contains(&path.as_str()) {
        return true;
    }
    if (engine("duckduckgo")
        && (has("q") || path.starts_with("/html") || path.starts_with("/lite")))
        || (engine("baidu") && path == "/s")
        || (engine("amazon") && path == "/s")
    {
        return true;
    }
    // Most sites put their own search at `/search` or `/results` with the
    // query in a parameter: Bing, Kagi, Brave, GitHub, YouTube, Reddit ...
    url.path_segments()
        .into_iter()
        .flatten()
        .any(|segment| matches!(segment.to_ascii_lowercase().as_str(), "search" | "results"))
        && url.query().is_some_and(|q| !q.is_empty())
}

/// Query parameter names that carry a secret or a one-time value. A GET can
/// spend a one-time link, and the value is nobody else's business anyway.
const TOKEN_WORDS: &[&str] = &[
    "token",
    "code",
    "key",
    "apikey",
    "secret",
    "sig",
    "signature",
    "auth",
    "session",
    "sessionid",
    "sid",
    "otp",
    "reset",
    "verify",
    "verification",
    "confirm",
    "confirmation",
    "magic",
    "nonce",
    "state",
    "password",
    "pwd",
    "ticket",
    "jwt",
    "credential",
    "credentials",
];

/// A query parameter whose name says it carries a secret, whose value is a
/// JSON Web Token, or credentials in the URL itself.
fn carries_token(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        let words_match = key
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| TOKEN_WORDS.contains(&word));
        words_match
            || ["token", "secret", "password", "signature"]
                .iter()
                .any(|word| key.contains(word))
            || (value.starts_with("eyJ") && value.len() > 30)
    })
}

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
    if known.unreadable > 0 {
        log.warn(&format!(
            "{} of {} could not be read and {} ignored",
            plural(known.unreadable, "line"),
            metadata::path(root).display(),
            if known.unreadable == 1 { "was" } else { "were" }
        ));
    }
    let plan = plan(&loaded.snapshots, &state, &known, options);
    if options.dry_run {
        report_dry_run(&plan, options, json, log);
        return Ok(());
    }
    let started = Instant::now();
    let tally = if plan.todo.is_empty() {
        Tally::default()
    } else {
        run(root, &plan, log)?
    };
    let seconds = started.elapsed().as_secs_f64();
    report_run(&plan, &tally, seconds, &metadata::path(root), json, log);
    Ok(())
}

/// Fetches the plan: login pages recorded at once, then each host's pages
/// in order on one of the workers, busiest hosts first so the longest queue
/// starts earliest.
fn run(root: &Path, plan: &Plan, log: Log) -> Result<Tally, Error> {
    let appender = Mutex::new(Appender::open(root)?);
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
    let mut hosts: Vec<(&str, Vec<&Item>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for item in &plan.todo {
        if item.why == Why::Login {
            record(
                &Line::new(&item.url, Outcome::BehindLogin).with_reason("login page, not fetched"),
            )?;
            continue;
        }
        let i = *index.entry(&item.host).or_insert_with(|| {
            hosts.push((&item.host, Vec::new()));
            hosts.len() - 1
        });
        hosts[i].1.push(item);
    }
    hosts.sort_by_key(|(_, items)| std::cmp::Reverse(items.len()));
    let total: usize = hosts.iter().map(|(_, items)| items.len()).sum();
    let queue = Mutex::new(hosts.into_iter().collect::<VecDeque<_>>());
    let fetcher = Fetcher::new();
    let done = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let failed: Mutex<Option<Error>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some((_, items)) = next else { break };
                    for item in items {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Err(err) = record(&fetcher.fetch(&item.url)) {
                            stop.store(true, Ordering::Relaxed);
                            failed
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .get_or_insert(err);
                            return;
                        }
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n.is_multiple_of(PROGRESS_EVERY) && n < total {
                            log.progress(&format!("fetched {n} of {total}"));
                        }
                    }
                }
            });
        }
    });
    if let Some(err) = failed.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(err);
    }
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

fn report_dry_run(plan: &Plan, options: Options, json: bool, log: Log) {
    if json {
        out::json(&serde_json::json!({
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
            "seconds_at_least": plan.seconds_at_least(),
        }));
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
            match plan.seconds_at_least() {
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
    let _ = writeln!(text, "{}", not_fetched_line(plan));
    out::block(&text);
}

fn counts_json(plan: &Plan) -> serde_json::Value {
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
fn not_fetched_line(plan: &Plan) -> String {
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

fn report_run(plan: &Plan, tally: &Tally, seconds: f64, path: &Path, json: bool, log: Log) {
    if json {
        out::json(&serde_json::json!({
            "fetched": tally.fetched,
            "ok": tally.ok,
            "behind_login": tally.behind_login,
            "skipped": tally.skipped,
            "errors": tally.errors,
            "not_fetched": counts_json(plan)["not_fetched"],
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
                let _ = writeln!(text, "  {label}: {}", breakdown(counts));
            }
        }
    }
    let _ = writeln!(text, "{}", not_fetched_line(plan));
    if plan.more > 0 {
        let _ = writeln!(text, "{} left for another run", plural(plan.more, "page"));
    }
    out::block(&text);
}

/// `3 timeout, 1 HTTP 404`, most common first.
fn breakdown(counts: &BTreeMap<String, usize>) -> String {
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

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn search_results_pages_are_known_by_engine_and_by_shape() {
        for raw in [
            "https://www.google.com/search?q=rust",
            "https://www.google.co.uk/search?q=rust&tbm=isch",
            "https://www.google.com/url?q=https://example.com",
            "https://duckduckgo.com/?q=rust",
            "https://www.bing.com/search?q=rust",
            "https://kagi.com/search?q=rust",
            "https://search.brave.com/search?q=rust",
            "https://github.com/search?q=parser&type=repositories",
            "https://www.youtube.com/results?search_query=rust",
            "https://www.baidu.com/s?wd=rust",
            "https://www.amazon.co.uk/s?k=kettle",
        ] {
            assert!(is_search_results(&url(raw)), "{raw}");
        }
        for raw in [
            "https://www.google.com/maps/place/x",
            "https://duckduckgo.com/about",
            "https://example.com/search",
            "https://example.com/research?q=x",
            "https://example.com/blog/how-search-works",
        ] {
            assert!(!is_search_results(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn token_parameters_are_known_by_name_value_and_credentials() {
        for raw in [
            "https://example.com/a?token=abc",
            "https://example.com/a?access_token=abc",
            "https://example.com/a?X-Amz-Signature=abc",
            "https://example.com/cb?code=abc&state=xyz",
            "https://example.com/a?resetPasswordToken=1",
            "https://example.com/a?api-key=1",
            "https://example.com/a?t=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
            "https://user:pass@example.com/",
        ] {
            assert!(carries_token(&url(raw)), "{raw}");
        }
        for raw in [
            "https://example.com/a?author=bob",
            "https://example.com/a?keywords=rust&monkey=1",
            "https://example.com/a?page=2&sort=new",
            "https://example.com/a?estate=house&design=x",
            "https://example.com/zip?postcode=AB1",
        ] {
            assert!(!carries_token(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn breakdowns_put_the_common_reason_first() {
        let counts: BTreeMap<String, usize> =
            [("HTTP 404".into(), 1), ("timeout".into(), 3)].into();
        assert_eq!(breakdown(&counts), "3 timeout, 1 HTTP 404");
    }
}
