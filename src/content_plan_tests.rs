//! Regression checks for `content_plan`.
//!
//! slice: content
//! why: Synthetic snapshots and attempt logs settle which pages are planned, routed and grouped without any request or tool run.

use super::*;
use crate::browser;
use crate::content_store::Tier;
use crate::content_test::{log, no_browser, no_gh, no_ytdlp, snapshot, state, unsent};
use crate::xpost;

/// The plan for `snapshots` with nothing known or forgotten, no `gh` and
/// no browser.
fn fresh(
    snapshots: &[Snapshot],
    options: Options,
    youtube: impl Fn() -> ytdlp::Readiness,
) -> (Plan, Work) {
    plan(
        snapshots,
        &state(&[]),
        &log(&[]),
        options,
        no_gh,
        youtube,
        no_browser,
    )
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
    let (plan, work) = plan(
        &snapshots,
        &state(&[]),
        &known,
        Options::default(),
        no_gh,
        no_ytdlp,
        no_browser,
    );
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
        no_ytdlp,
        no_browser,
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
fn estimates_count_paced_fetches_and_exclude_gh() {
    use crate::github_api::Gh;

    let snapshots = [snapshot(&[
        "https://github.com/owner/one",
        "https://github.com/owner/two",
        "https://github.com/owner/three",
        "https://github.com/owner/four",
        "https://github.com/owner/five",
        "https://a.test/one",
        "https://a.test/one#alias",
        "https://a.test/two",
        "https://x.com/someone/status/42",
        "https://twitter.com/someone/status/42#alias",
    ])];
    let ready = || {
        Readiness::Ready(Gh {
            path: "/usr/bin/gh".into(),
            version: None,
        })
    };
    let (_, work) = plan(
        &snapshots,
        &state(&[]),
        &log(&[]),
        Options::default(),
        ready,
        no_ytdlp,
        no_browser,
    );
    assert_eq!(
        work.seconds_at_least(),
        1,
        "only the two distinct web fetches are paced"
    );
    let (_, web) = plan(
        &snapshots,
        &state(&[]),
        &log(&[]),
        Options::default(),
        no_gh,
        no_ytdlp,
        no_browser,
    );
    assert_eq!(
        web.seconds_at_least(),
        4,
        "five GitHub web fetches are paced"
    );
    let (_, gh_only) = plan(
        &[snapshot(&[
            "https://github.com/owner/one",
            "https://github.com/owner/two",
            "https://github.com/owner/three",
            "https://github.com/owner/four",
            "https://github.com/owner/five",
        ])],
        &state(&[]),
        &log(&[]),
        Options::default(),
        ready,
        no_ytdlp,
        no_browser,
    );
    assert_eq!(
        gh_only.seconds_at_least(),
        0,
        "gh has concurrency bounds, no pacing delay"
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
        no_ytdlp,
        no_browser,
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
        no_ytdlp,
        no_browser,
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
    let (_, work) = super::plan(
        &snapshots,
        &state(&[]),
        &log(&[]),
        limited,
        no_gh,
        no_ytdlp,
        no_browser,
    );
    assert_eq!(
        work.notes(),
        ["GitHub API: gh not found, used the web page for 1 page"]
    );

    let never = || -> Readiness { panic!("no GitHub page, so gh is not asked") };
    let web_only = [snapshot(&["https://a.test/"])];
    let (_, work) = super::plan(
        &web_only,
        &state(&[]),
        &log(&[]),
        Options::default(),
        never,
        no_ytdlp,
        no_browser,
    );
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
        no_ytdlp,
        no_browser,
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
        no_ytdlp,
        no_browser,
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
    let (plan, work) = plan(
        &snapshots,
        &state(&[]),
        &known,
        limited,
        no_gh,
        no_ytdlp,
        no_browser,
    );
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
    let (plan, work) = super::plan(
        &snapshots,
        &state(&[]),
        &known,
        refetch,
        no_gh,
        no_ytdlp,
        no_browser,
    );
    let whys: Vec<Why> = plan.todo.iter().map(|i| i.why.clone()).collect();
    assert_eq!(whys, [Why::Retry, Why::Refetch, Why::Refetch, Why::New]);
    let attempts: Vec<u32> = work.fetches.iter().map(|f| f.pages[0].1).collect();
    assert_eq!(attempts, [3, 1, 1, 1]);
}

#[test]
fn videos_wait_for_yt_dlp_unrecorded_or_go_one_at_a_time_once_per_id() {
    use crate::ytdlp::{Runtime, YtDlp};

    let snapshots = [snapshot(&[
        "https://www.youtube.com/watch?v=aBc-12_xYz9",
        "https://youtu.be/aBc-12_xYz9?t=5",
        "https://www.youtube.com/watch?v=zYx-98_cBa7&list=PL0000",
        "https://www.youtube.com/@someone",
        "https://www.youtube.com/watch?v=short",
        "https://a.test/",
    ])];
    let asked = std::cell::Cell::new(0);
    let missing = || {
        asked.set(asked.get() + 1);
        ytdlp::Readiness::NoRuntime("/opt/bin/yt-dlp".into(), None)
    };
    let (plan, work) = fresh(&snapshots, Options::default(), missing);
    assert_eq!(asked.get(), 1, "readiness is asked once");
    let todo: Vec<&str> = plan.todo.iter().map(|i| i.url.as_str()).collect();
    assert_eq!(
        todo,
        ["https://www.youtube.com/watch?v=short", "https://a.test/"]
    );
    assert_eq!(work.waiting, 3);
    assert_eq!(
        work.notes(),
        ["3 YouTube pages waiting for yt-dlp (see knowmoretabs doctor)"]
    );
    assert_eq!(
        unsent(&work),
        [(
            "https://www.youtube.com/@someone",
            Status::Skipped,
            "not a document"
        )],
        "waiting videos are not recorded"
    );
    let limited = Options {
        limit: Some(1),
        ..Options::default()
    };
    let (plan, work) = fresh(&snapshots, limited, missing);
    assert_eq!((plan.todo.len(), plan.more, work.waiting), (1, 1, 3));

    let ready = || {
        ytdlp::Readiness::Ready(YtDlp {
            path: "/opt/bin/yt-dlp".into(),
            version: None,
            runtime: Runtime {
                name: "deno",
                path: "/opt/bin/deno".into(),
            },
        })
    };
    let (_, work) = fresh(&snapshots, Options::default(), ready);
    let fetches: Vec<(&str, &Route, usize)> = work
        .fetches
        .iter()
        .map(|f| (f.host.as_str(), &f.route, f.pages.len()))
        .collect();
    assert_eq!(
        fetches,
        [
            (ytdlp::HOST, &Route::Youtube("aBc-12_xYz9".to_owned()), 2),
            (ytdlp::HOST, &Route::Youtube("zYx-98_cBa7".to_owned()), 1),
            ("www.youtube.com", &Route::Web, 1),
            ("a.test", &Route::Web, 1),
        ]
    );
    assert_eq!((work.waiting, work.notes()), (0, Vec::<String>::new()));
    assert_eq!(
        work.seconds_at_least(),
        2 * ytdlp::SECONDS_AT_LEAST,
        "two videos, one at a time, on the page host's queue with its web page"
    );

    let never = || -> ytdlp::Readiness { panic!("no video, so yt-dlp is not asked") };
    let web_only = [snapshot(&["https://a.test/", "https://www.youtube.com/"])];
    let (_, work) = fresh(&web_only, Options::default(), never);
    assert_eq!(work.youtube, None);
}

/// Library pages an earlier run read as thin, as a script shell, as an
/// app shell, and rendered, and one never read.
fn read_before() -> ([Snapshot; 1], content_store::Log) {
    let snapshots = [snapshot(&[
        "https://a.test/thin",
        "https://a.test/thin#part",
        "https://a.test/shell",
        "https://a.test/app",
        "https://a.test/rendered",
        "https://a.test/new",
    ])];
    let mut known = content_store::Log::default();
    for (url, tier, status, reason) in [
        ("https://a.test/thin", Tier::Web, Status::Thin, "short text"),
        (
            "https://a.test/thin#part",
            Tier::Web,
            Status::Thin,
            "short text",
        ),
        (
            "https://a.test/shell",
            Tier::Web,
            Status::EmptyShell,
            "drawn by scripts",
        ),
        (
            "https://a.test/app",
            Tier::Web,
            Status::EmptyShell,
            "app shell",
        ),
        (
            "https://a.test/rendered",
            Tier::Headless,
            Status::Thin,
            "short text; rendering added no text",
        ),
    ] {
        let line = content_fetch::public_line(url, tier, status).with_reason(reason);
        known.pages.insert(url.to_owned(), line);
    }
    (snapshots, known)
}

#[test]
fn pages_read_thin_before_are_rendered_once_per_document_without_an_http_read() {
    let (snapshots, known) = read_before();
    let asked = std::cell::Cell::new(0);
    let ready = || {
        asked.set(asked.get() + 1);
        browser::Readiness::Ready("/opt/Google Chrome".into())
    };
    let (plan, work) = plan(
        &snapshots,
        &state(&[]),
        &known,
        Options::default(),
        no_gh,
        no_ytdlp,
        ready,
    );
    assert_eq!(asked.get(), 1, "readiness is asked once");
    let whys: Vec<(&str, Why)> = plan
        .todo
        .iter()
        .map(|i| (i.url.as_str(), i.why.clone()))
        .collect();
    assert_eq!(
        whys,
        [
            ("https://a.test/thin", Why::Render),
            ("https://a.test/thin#part", Why::Render),
            ("https://a.test/shell", Why::Render),
            ("https://a.test/new", Why::New),
        ]
    );
    assert_eq!(plan.already_fetched, 2, "an app shell and a render stand");
    let renders: Vec<(&str, Vec<&str>)> = work
        .renders
        .iter()
        .map(|r| {
            (
                r.url.as_str(),
                r.pages.iter().map(|line| line.url.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        renders,
        [
            (
                "https://a.test/thin",
                vec!["https://a.test/thin", "https://a.test/thin#part"]
            ),
            ("https://a.test/shell", vec!["https://a.test/shell"]),
        ],
        "one render per document, with no new HTTP read"
    );
    let fetched: Vec<&str> = work.fetches.iter().map(|f| f.url.as_str()).collect();
    assert_eq!(fetched, ["https://a.test/new"]);
    assert_eq!(work.browser_waiting, 0);
}

#[test]
fn without_a_browser_they_wait_and_refetch_reads_them_over_http_again() {
    let (snapshots, known) = read_before();
    let ready = || browser::Readiness::Ready("/opt/Google Chrome".into());
    let (plan, work) = plan(
        &snapshots,
        &state(&[]),
        &known,
        Options::default(),
        no_gh,
        no_ytdlp,
        no_browser,
    );
    assert_eq!(plan.todo.len(), 1);
    assert_eq!(plan.already_fetched, 5);
    assert_eq!((work.renders.len(), work.browser_waiting), (0, 3));

    let refetch = Options {
        refetch: true,
        ..Options::default()
    };
    let (plan, work) = super::plan(
        &snapshots,
        &state(&[]),
        &known,
        refetch,
        no_gh,
        no_ytdlp,
        ready,
    );
    assert!(plan.todo.iter().all(|i| i.why != Why::Render));
    assert_eq!((work.renders.len(), work.fetches.len()), (0, 5));

    let never = || -> browser::Readiness { panic!("no web page, so no browser is asked") };
    let (_, work) = super::plan(
        &[snapshot(&["https://x.com/someone/status/42"])],
        &state(&[]),
        &log(&[]),
        Options::default(),
        no_gh,
        no_ytdlp,
        never,
    );
    assert_eq!(work.browser, None);
}
