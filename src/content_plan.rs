//! Which library pages `content` captures, and how: the shared plan with
//! content's own rules on top, each page's route, and one fetch per
//! document.
//!
//! slice: content
//! why: Content capture sends the URLs you visited to their own sites, so
//!      what it will send is settled before any request, and `--dry-run`
//!      shows exactly that. Pages that are not documents are recorded once
//!      with the reason, and pages the shared rules keep home are counted
//!      and stay out of the store. A document known by several addresses
//!      is fetched once and recorded under each. Each page's route is
//!      chosen from its address, so the plan names the host every request
//!      goes to. A tool is asked about only when the plan has a page for
//!      it: without a signed in `gh` a GitHub page is planned for the web
//!      instead, and without yt-dlp a video waits, unrecorded, for a run
//!      that has it.

use std::cell::OnceCell;
use std::collections::HashMap;

use url::Url;

use crate::content_fetch;
use crate::content_route::Route;
use crate::content_store::{self, Line, Status};
use crate::github_api::Readiness;
use crate::library::State;
use crate::model::Snapshot;
use crate::targets::{self, Forgotten, Item, Options, Plan, Skip, Why};
use crate::triage::plural;
use crate::ytdlp;

/// What the run will do beyond the shared plan.
#[derive(Debug, Default)]
pub struct Work {
    /// One fetch per page, its fragment variants recorded with it.
    pub fetches: Vec<Fetch>,
    /// Pages recorded without a request: login screens, and pages that are
    /// not documents.
    pub unsent: Vec<Line>,
    /// Whether `gh` can read GitHub pages, asked only when the plan has
    /// one.
    pub github: Option<Readiness>,
    /// Whether yt-dlp can read videos, asked only when the plan has one.
    pub youtube: Option<ytdlp::Readiness>,
    /// GitHub documents read from the web because `gh` cannot.
    github_on_the_web: usize,
    /// Video pages left for a run with yt-dlp, unrecorded.
    pub waiting: usize,
}

impl Work {
    /// The pacing lower bound for a dry run.
    pub fn seconds_at_least(&self) -> usize {
        targets::seconds_at_least(
            self.fetches
                .iter()
                .map(|fetch| (fetch.host.as_str(), fetch.route.cost())),
        )
    }

    /// What the report says beyond the counts.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        let fallback = self.github.as_ref().and_then(Readiness::fallback);
        if let Some(why) = fallback
            && self.github_on_the_web > 0
        {
            notes.push(format!(
                "GitHub API: {why}, used the web page for {}",
                plural(self.github_on_the_web, "page")
            ));
        }
        if self.waiting > 0 {
            notes.push(ytdlp::waiting_note(self.waiting));
        }
        notes
    }
}

#[derive(Debug)]
pub struct Fetch {
    /// The first library URL of the group; the fetcher drops its fragment.
    pub url: String,
    /// The queue it waits in: its host, or one of its host's lanes.
    pub host: String,
    pub route: Route,
    /// Every library URL the fetch stands for, with its attempt number.
    pub pages: Vec<(String, u32)>,
}

/// The shared plan, with `content`'s own rules on top: pages that are not
/// documents stay home, as does every variant of a forgotten page,
/// `--limit` counts what is left, fragment variants share a fetch, and pages
/// that are not documents are recorded once. Pages the shared rules keep
/// home are counted, never recorded, as with `enrich`. `github` says
/// whether `gh` can read GitHub pages and `youtube` whether yt-dlp can
/// read videos; each is asked once, and only when there is such a page.
/// Videos waiting for yt-dlp are counted before `--limit`, which counts
/// only what is fetched.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    options: Options,
    github: impl Fn() -> Readiness,
    youtube: impl Fn() -> ytdlp::Readiness,
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
    let forgotten = Forgotten::of(state);
    let (candidates, forgotten_pages): (Vec<Item>, Vec<Item>) = std::mem::take(&mut plan.todo)
        .into_iter()
        .partition(|item| !forgotten.covers(&item.url));
    plan.not_fetched.extend(
        forgotten_pages
            .into_iter()
            .map(|item| (item.url, Skip::Forgotten)),
    );
    let readiness = OnceCell::new();
    let videos = OnceCell::new();
    let mut waiting = 0;
    let mut routes = Vec::new();
    let mut on_the_web = Vec::new();
    let mut not_documents = Vec::new();
    for mut item in candidates {
        let Some(route) = Url::parse(&item.url).map_or(Some(Route::Web), |url| Route::of(&url))
        else {
            not_documents.push(item);
            continue;
        };
        if matches!(route, Route::Youtube(_)) && videos.get_or_init(&youtube).tool().is_none() {
            waiting += 1;
            continue;
        }
        let without_gh =
            matches!(route, Route::Github(_)) && readiness.get_or_init(&github).gh().is_none();
        let route = if without_gh { Route::Web } else { route };
        if let Some(host) = route.host() {
            host.clone_into(&mut item.host);
        }
        plan.todo.push(item);
        routes.push(route);
        on_the_web.push(without_gh);
    }
    let mut unsent: Vec<Line> = not_documents
        .iter()
        .map(|item| Line::new(&item.url, Status::Skipped).with_reason(Skip::NotADocument.label()))
        .collect();
    plan.not_fetched.extend(
        not_documents
            .into_iter()
            .map(|item| (item.url, Skip::NotADocument)),
    );
    if let Some(limit) = options.limit {
        plan.more = plan.todo.len().saturating_sub(limit);
        plan.todo.truncate(limit);
        routes.truncate(limit);
        on_the_web.truncate(limit);
    }
    let fetches = group(&plan.todo, routes, known, &mut unsent);
    let work = Work {
        fetches,
        unsent,
        github: readiness.into_inner(),
        youtube: videos.into_inner(),
        github_on_the_web: on_the_web.iter().filter(|web| **web).count(),
        waiting,
    };
    (plan, work)
}

/// One fetch per document, recorded under each address it was opened at;
/// login screens go to `unsent`, recorded without a request. A host whose
/// route allows several requests at once is spread over that many lanes.
fn group(
    todo: &[Item],
    routes: Vec<Route>,
    known: &content_store::Log,
    unsent: &mut Vec<Line>,
) -> Vec<Fetch> {
    let attempt = |url: &str| content_fetch::next_attempt(known.pages.get(url));
    let mut fetches: Vec<Fetch> = Vec::new();
    let mut by_page: HashMap<String, usize> = HashMap::new();
    let mut lanes: HashMap<&str, usize> = HashMap::new();
    for (item, route) in todo.iter().zip(routes) {
        if item.why == Why::Login {
            let line =
                Line::new(&item.url, Status::BehindLogin).with_reason("login page, not fetched");
            unsent.push(content_fetch::settle(line, attempt(&item.url)));
            continue;
        }
        let page = route.key(&item.url);
        let pair = (item.url.clone(), attempt(&item.url));
        if let Some(&i) = by_page.get(&page) {
            fetches[i].pages.push(pair);
        } else {
            by_page.insert(page, fetches.len());
            let host = match route.lanes() {
                1 => item.host.clone(),
                n => {
                    let used = lanes.entry(item.host.as_str()).or_default();
                    *used += 1;
                    format!("{} {}", item.host, *used % n)
                }
            };
            fetches.push(Fetch {
                url: item.url.clone(),
                host,
                route,
                pages: vec![pair],
            });
        }
    }
    fetches
}

#[cfg(test)]
#[path = "content_plan_tests.rs"]
mod tests;
