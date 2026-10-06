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
//!      goes to; a GitHub page waits for `gh` only when the plan has one,
//!      and without a signed in `gh` it is planned for the web instead.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use url::Url;

use crate::content_fetch;
use crate::content_route::Route;
use crate::content_store::{self, Line, Status};
use crate::error::Error;
use crate::github_api::Readiness;
use crate::guard;
use crate::library::{self, State};
use crate::model::Snapshot;
use crate::targets::{self, Item, Options, Plan, Skip, Why};
use crate::triage::plural;

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
    /// GitHub documents read from the web because `gh` cannot.
    github_on_the_web: usize,
}

impl Work {
    /// What the report says beyond the counts.
    pub fn notes(&self) -> Vec<String> {
        let fallback = self.github.as_ref().and_then(Readiness::fallback);
        match (fallback, self.github_on_the_web) {
            (Some(why), n) if n > 0 => vec![format!(
                "GitHub API: {why}, used the web page for {}",
                plural(n, "page")
            )],
            _ => Vec::new(),
        }
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

/// The snapshots narrowed to `urls`, each of which must be a library page;
/// all of them when `urls` is empty.
pub fn only<'a>(
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
/// documents stay home, as does every variant of a forgotten page,
/// `--limit` counts what is left, fragment variants share a fetch, and pages
/// that are not documents are recorded once. Pages the shared rules keep
/// home are counted, never recorded, as with `enrich`. `github` says
/// whether `gh` can read GitHub pages; it is asked once, and only when
/// there is one.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    options: Options,
    github: impl Fn() -> Readiness,
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
    let forgotten_urls: HashSet<Url> = state
        .forgotten
        .iter()
        .filter_map(|raw| guard::page_url(raw))
        .collect();
    let (candidates, forgotten_pages): (Vec<Item>, Vec<Item>) = std::mem::take(&mut plan.todo)
        .into_iter()
        .partition(|item| {
            !guard::page_url(&item.url).is_some_and(|url| forgotten_urls.contains(&url))
        });
    plan.not_fetched.extend(
        forgotten_pages
            .into_iter()
            .map(|item| (item.url, Skip::Forgotten)),
    );
    let readiness = OnceCell::new();
    let mut routes = Vec::new();
    let mut on_the_web = Vec::new();
    let mut not_documents = Vec::new();
    for mut item in candidates {
        let Some(route) = Url::parse(&item.url).map_or(Some(Route::Web), |url| Route::of(&url))
        else {
            not_documents.push(item);
            continue;
        };
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
        github_on_the_web: on_the_web.iter().filter(|web| **web).count(),
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
pub(crate) mod tests {
    use super::*;
    use crate::xpost;

    pub(crate) fn snapshot(urls: &[&str]) -> Snapshot {
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

    pub(crate) fn state(forgotten: &[&str]) -> State {
        serde_json::from_value(serde_json::json!({"schema_version": 1, "forgotten": forgotten}))
            .unwrap()
    }

    pub(crate) fn log(lines: &[(&str, Status, u32)]) -> content_store::Log {
        let mut log = content_store::Log::default();
        for (url, status, attempt) in lines {
            let mut line = Line::new(url, *status);
            line.attempt = *attempt;
            log.pages.insert((*url).to_owned(), line);
        }
        log
    }

    pub(crate) fn no_gh() -> Readiness {
        Readiness::Missing
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
        let (plan, work) = plan(&snapshots, &state(&[]), &known, Options::default(), no_gh);
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
    fn one_post_is_fetched_once_from_the_api_host() {
        let snapshots = [snapshot(&[
            "https://x.com/someone/status/42",
            "https://twitter.com/someone/status/42#top",
            "https://x.com/other/status/43",
            "https://a.test/",
        ])];
        let (plan, work) = plan(
            &snapshots,
            &state(&[]),
            &log(&[]),
            Options::default(),
            no_gh,
        );
        let hosts: Vec<&str> = plan.todo.iter().map(|i| i.host.as_str()).collect();
        assert_eq!(
            hosts,
            [xpost::API_HOST, xpost::API_HOST, xpost::API_HOST, "a.test"]
        );
        assert_eq!(plan.sites(), 2);
        let fetches: Vec<(&str, &Route, usize)> = work
            .fetches
            .iter()
            .map(|f| (f.host.as_str(), &f.route, f.pages.len()))
            .collect();
        assert_eq!(
            fetches,
            [
                (xpost::API_HOST, &Route::XPost("42".to_owned()), 2),
                (xpost::API_HOST, &Route::XPost("43".to_owned()), 1),
                ("a.test", &Route::Web, 1),
            ]
        );
    }

    #[test]
    fn github_documents_go_to_gh_in_lanes_or_to_the_web_with_a_note() {
        use crate::github::Target;
        use crate::github_api::{self, Gh};

        let snapshots = [snapshot(&[
            "https://github.com/owner/one",
            "https://github.com/owner/two",
            "https://github.com/Owner/One/#readme",
            "https://github.com/owner/one/issues/3",
            "https://github.com/settings/profile",
            "https://a.test/",
        ])];
        let asked = std::cell::Cell::new(0);
        let ready = || {
            asked.set(asked.get() + 1);
            Readiness::Ready(Gh {
                path: "/usr/bin/gh".into(),
                version: None,
            })
        };
        let (plan, work) = plan(
            &snapshots,
            &state(&[]),
            &log(&[]),
            Options::default(),
            ready,
        );
        assert_eq!(asked.get(), 1, "readiness is asked once");
        let fetches: Vec<(&str, bool, usize)> = work
            .fetches
            .iter()
            .map(|f| {
                (
                    f.host.as_str(),
                    matches!(f.route, Route::Github(_)),
                    f.pages.len(),
                )
            })
            .collect();
        assert_eq!(
            fetches,
            [
                ("api.github.com 1", true, 2),
                ("api.github.com 2", true, 1),
                ("api.github.com 3", true, 1),
                ("github.com", false, 1),
                ("a.test", false, 1),
            ]
        );
        assert_eq!(plan.sites(), 3);
        assert_eq!(work.notes(), Vec::<String>::new());
        assert!(matches!(
            &work.fetches[2].route,
            Route::Github(Target::Issue(_, 3))
        ));

        let (plan, work) = super::plan(
            &snapshots,
            &state(&[]),
            &log(&[]),
            Options::default(),
            no_gh,
        );
        assert!(work.fetches.iter().all(|f| f.route == Route::Web));
        assert_eq!(plan.sites(), 2);
        assert_eq!(
            work.notes(),
            ["GitHub API: gh not found, used the web page for 4 pages"]
        );
        let limited = Options {
            limit: Some(1),
            ..Options::default()
        };
        let (_, work) = super::plan(&snapshots, &state(&[]), &log(&[]), limited, no_gh);
        assert_eq!(
            work.notes(),
            ["GitHub API: gh not found, used the web page for 1 page"]
        );

        let never = || -> Readiness { panic!("no GitHub page, so gh is not asked") };
        let web_only = [snapshot(&["https://a.test/"])];
        let (_, work) = super::plan(&web_only, &state(&[]), &log(&[]), Options::default(), never);
        assert_eq!(work.github, None);
        assert_eq!(github_api::CONCURRENT, 4);
    }

    #[test]
    fn non_document_skips_are_recorded_once_and_private_urls_never() {
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
            no_gh,
        );
        let mut lines = unsent(&work);
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                (
                    "https://a.test/login",
                    Status::BehindLogin,
                    "login page, not fetched"
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
    fn forgotten_fragment_variants_are_excluded_before_fetching() {
        let snapshots = [snapshot(&[
            "https://forgotten.test/page",
            "https://forgotten.test/page#other",
        ])];
        let (plan, work) = plan(
            &snapshots,
            &state(&["https://forgotten.test/page#hidden"]),
            &log(&[]),
            Options::default(),
            no_gh,
        );
        assert!(work.fetches.is_empty());
        assert_eq!(work.unsent, []);
        assert_eq!(plan.skip_counts()[&Skip::Forgotten], 2);
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
        let (plan, work) = plan(&snapshots, &state(&[]), &known, limited, no_gh);
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
        let (plan, work) = super::plan(&snapshots, &state(&[]), &known, refetch, no_gh);
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
}
