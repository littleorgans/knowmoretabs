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
//!      that has it. A page an earlier run read as thin or empty is
//!      rendered in a browser when there is one, without a new HTTP read,
//!      and waits for one when there is not.

use std::cell::{Cell, OnceCell};
use std::collections::HashMap;

use url::Url;

use crate::browser;
use crate::content_fetch;
use crate::content_headless::{self, Render};
use crate::content_route::Route;
use crate::content_store::{self, Line, Status};
use crate::github_api::Readiness;
use crate::image_pick::Found;
use crate::library::State;
use crate::model::Snapshot;
use crate::targets::{self, Forgotten, Item, Options, Plan, Recorded, Skip, Why};
use crate::triage::plural;
use crate::ytdlp;

/// What the run will do beyond the shared plan.
#[derive(Debug, Default)]
pub struct Work {
    /// One fetch per page, its fragment variants recorded with it.
    pub fetches: Vec<Fetch>,
    /// Pages read as thin or empty before, one render per document.
    pub renders: Vec<Render>,
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
    /// Whether pages can be rendered, asked only when the plan has a page
    /// a browser may improve on.
    pub browser: Option<browser::Readiness>,
    /// Pages read as thin or empty before, left for a run with a browser.
    pub browser_waiting: usize,
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

    /// Whether a page is read over HTTP this run, and so may read thin or
    /// empty and need a browser after.
    pub fn reads_the_web(&self) -> bool {
        self.fetches.iter().any(|fetch| fetch.route == Route::Web)
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
/// read videos, and `browser` whether pages can be rendered; each is
/// asked once, and only when there is such a page. Videos waiting for
/// yt-dlp, and pages waiting for a browser, are counted before `--limit`,
/// which counts only what is fetched or rendered.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    options: Options,
    github: impl Fn() -> Readiness,
    youtube: impl Fn() -> ytdlp::Readiness,
    browser: impl Fn() -> browser::Readiness,
) -> (Plan, Work) {
    let rendering = OnceCell::new();
    let browser_waiting = Cell::new(0);
    let mut plan = targets::plan(
        snapshots,
        state,
        |url| match content_store::recorded(known, url) {
            // `--refetch` reads a page over HTTP again, then renders it if
            // it still reads thin or empty.
            Recorded::Final
                if !options.refetch
                    && known
                        .pages
                        .get(url)
                        .is_some_and(content_headless::escalates) =>
            {
                if rendering.get_or_init(&browser).path().is_some() {
                    Recorded::Render
                } else {
                    browser_waiting.set(browser_waiting.get() + 1);
                    Recorded::Final
                }
            }
            recorded => recorded,
        },
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
        if item.why == Why::Render {
            plan.todo.push(item);
            routes.push(Route::Web);
            on_the_web.push(false);
            continue;
        }
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
    let (fetches, renders) = group(&plan.todo, routes, known, &mut unsent);
    let mut work = Work {
        fetches,
        renders,
        unsent,
        github: readiness.into_inner(),
        youtube: videos.into_inner(),
        github_on_the_web: on_the_web.iter().filter(|web| **web).count(),
        waiting,
        browser: None,
        browser_waiting: browser_waiting.get(),
    };
    if work.reads_the_web() {
        rendering.get_or_init(&browser);
    }
    work.browser = rendering.into_inner();
    (plan, work)
}

/// One fetch per document, recorded under each address it was opened at,
/// and one render per document, with the line each address has; login
/// screens go to `unsent`, recorded without a request. A host whose route
/// allows several requests at once is spread over that many lanes.
fn group(
    todo: &[Item],
    routes: Vec<Route>,
    known: &content_store::Log,
    unsent: &mut Vec<Line>,
) -> (Vec<Fetch>, Vec<Render>) {
    let attempt = |url: &str| content_fetch::next_attempt(known.pages.get(url));
    let mut fetches: Vec<Fetch> = Vec::new();
    let mut by_page: HashMap<String, usize> = HashMap::new();
    let mut lanes: HashMap<&str, usize> = HashMap::new();
    let mut renders: Vec<Vec<Line>> = Vec::new();
    let mut rendered: HashMap<String, usize> = HashMap::new();
    for (item, route) in todo.iter().zip(routes) {
        if item.why == Why::Login {
            let line =
                Line::new(&item.url, Status::BehindLogin).with_reason("login page, not fetched");
            unsent.push(content_fetch::settle(line, attempt(&item.url)));
            continue;
        }
        let page = route.key(&item.url);
        if item.why == Why::Render {
            let Some(line) = known.pages.get(&item.url).cloned() else {
                continue;
            };
            if let Some(&i) = rendered.get(&page) {
                renders[i].push(line);
            } else {
                rendered.insert(page, renders.len());
                renders.push(vec![line]);
            }
            continue;
        }
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
    let renders = renders
        .into_iter()
        .filter_map(|pages| Render::of(pages, Found::Unread))
        .collect();
    (fetches, renders)
}

#[cfg(test)]
#[path = "content_plan_tests.rs"]
mod tests;
