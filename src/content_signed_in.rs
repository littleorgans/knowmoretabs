//! `content --signed-in`: which pages are opened in the owner's own
//! browser, signed in, and how that browser is found.
//!
//! slice: content
//! why: A page a public read found behind a login, paywalled or blocked is
//!      often the owner's to read, in the browser they are signed in to.
//!      Only such a page is opened, and only when a public tier read it, so
//!      a signed in attempt is final and a tier added later must choose. The
//!      owner's personal apps are never opened: mail, chat, documents and
//!      account consoles. The browser is found from two files it keeps and
//!      nothing else, its remote debugging switch and the port file it
//!      writes while it listens, and is reached only once the owner allows
//!      the connection; each way that fails says what to do, in one line,
//!      before anything is recorded.

use std::fs;
use std::path::Path;

use url::Url;

use crate::browser::{self, Browser};
use crate::content_plan::{self, Work};
use crate::content_route::Route;
use crate::content_store::{self, Line, Status, Tier};
use crate::guard;
use crate::library::State;
use crate::model::Snapshot;
use crate::platform;
use crate::targets::{self, Forgotten, Options, Plan, Recorded, Skip, Why};
use crate::tools::{Probe, System};
use crate::triage::plural;

/// Pages a signed in run opens when `--limit` does not say.
pub const LIMIT: usize = 25;

/// Why the owner's browser could not be reached. Nothing is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unavailable {
    #[error(
        "Chrome remote debugging is off. Turn it on at chrome://inspect/#remote-debugging, then run again."
    )]
    Off,
    #[error(
        "Chrome is not running with remote debugging. Open Chrome, check chrome://inspect/#remote-debugging, then run again."
    )]
    NotRunning,
    #[error("Chrome did not allow the connection: click Allow when Chrome asks (60 s).")]
    NotAllowed,
}

impl Unavailable {
    /// The name a caller following the run gets.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::NotRunning => "not_running",
            Self::NotAllowed => "not_allowed",
        }
    }
}

/// Whether a page whose latest line is `line` is opened signed in: read by
/// a public tier as behind a login, paywalled or blocked, or rendered thin
/// or empty because the site answered 403.
pub fn eligible(line: &Line) -> bool {
    signed_in_source(line.tier)
        && match line.status {
            Status::BehindLogin | Status::Paywalled | Status::Blocked => true,
            Status::Thin | Status::EmptyShell => line
                .reason
                .as_deref()
                .is_some_and(|reason| reason.ends_with("not rendered: HTTP 403")),
            _ => false,
        }
}

/// The tiers whose lines a signed in run may improve on: the public ones.
fn signed_in_source(tier: Option<Tier>) -> bool {
    match tier {
        Some(Tier::Web | Tier::X | Tier::Github | Tier::Youtube | Tier::Headless) => true,
        Some(Tier::SignedIn | Tier::Other) | None => false,
    }
}

/// What a signed in run opens, settled before anything is: the eligible
/// library pages by the shared rules, every variant of a forgotten page
/// left home, personal apps counted and never opened, no sign-in screen's
/// own address, at most `--limit` ([`LIMIT`] when not given), and one
/// render per document, loaded from its library address. Also how many
/// personal app pages were left.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    known: &content_store::Log,
    options: Options,
) -> (Plan, Work, usize) {
    let mut plan = targets::plan(
        snapshots,
        state,
        |url| match known.pages.get(url) {
            Some(line) if eligible(line) => Recorded::Render,
            _ => Recorded::Final,
        },
        Options {
            limit: None,
            ..options
        },
    );
    let forgotten = Forgotten::of(state);
    let mut personal = 0;
    for item in std::mem::take(&mut plan.todo) {
        if forgotten.covers(&item.url) {
            plan.not_fetched.push((item.url, Skip::Forgotten));
        } else if Url::parse(&item.url).is_ok_and(|url| guard::is_personal_app(&url)) {
            personal += 1;
        } else if item.why == Why::Render {
            plan.todo.push(item);
        }
    }
    let limit = limit(options);
    plan.more = plan.todo.len().saturating_sub(limit);
    plan.todo.truncate(limit);
    let routes = vec![Route::Web; plan.todo.len()];
    let (_, renders) = content_plan::group(&plan.todo, routes, known, &mut Vec::new(), library);
    let mut work = Work::default();
    work.renders = renders;
    (plan, work, personal)
}

/// The most pages a signed in run opens: `--limit`, else [`LIMIT`].
pub fn limit(options: Options) -> usize {
    options.limit.unwrap_or(LIMIT)
}

/// Where a signed in render loads its document: the library address,
/// never where a public read ended, which may be a sign-in screen.
fn library(line: &Line) -> &str {
    &line.url
}

/// Each `--url` page a signed in run does not open because its line is not
/// one it improves on.
pub fn ineligible(urls: &[String], known: &content_store::Log) -> Vec<String> {
    urls.iter()
        .filter(|url| !known.pages.get(url.as_str()).is_some_and(eligible))
        .map(|url| format!("not eligible for --signed-in: {url}"))
        .collect()
}

/// What `--dry-run` says of a signed in run: how its `renders` are opened
/// in browser `name`, and how many `personal` app pages are not.
pub fn dry_run_notes(renders: usize, personal: usize, name: &str) -> Vec<String> {
    let mut notes = Vec::new();
    if renders > 0 {
        notes.push(format!(
            "{} opened in {name} signed in, one at a time",
            match renders {
                1 => "1 page is".to_owned(),
                n => format!("{n} pages are"),
            }
        ));
    }
    if personal > 0 {
        notes.push(format!(
            "personal apps: {} not opened",
            plural(personal, "page")
        ));
    }
    notes
}

/// Browser `id`'s name in reports: its binary's, else its id.
pub fn name(id: &str) -> String {
    System
        .browser(id)
        .map_or_else(|| id.to_owned(), |path| browser::name(&path))
}

/// Where browser `id` listens for `DevTools`, from its user-data
/// directory: the first of its places that exists, else the first.
pub fn find(id: &str) -> Result<(u16, String), Unavailable> {
    let user_data = platform::browser(id)
        .zip(platform::Roots::detect())
        .and_then(|(browser, roots)| {
            let dirs = browser.user_data_dirs(&roots);
            dirs.iter()
                .find(|dir| dir.is_dir())
                .or(dirs.first())
                .cloned()
        })
        .ok_or(Unavailable::NotRunning)?;
    discover(&user_data)
}

/// Where the browser whose user data is `user_data` listens for
/// `DevTools`: its remote debugging switch must be on, then the port file
/// it writes while it listens must name a port and a route.
pub fn discover(user_data: &Path) -> Result<(u16, String), Unavailable> {
    if !platform::remote_debugging(user_data).unwrap_or(false) {
        return Err(Unavailable::Off);
    }
    fs::read_to_string(user_data.join(browser::PORT_FILE))
        .ok()
        .and_then(|text| browser::endpoint(&text))
        .ok_or(Unavailable::NotRunning)
}

/// Attaches to browser `id`, `name` in reports, found running with remote
/// debugging on; the owner is asked to allow it.
pub fn attach(id: &str, name: String) -> Result<Browser, Unavailable> {
    let (port, route) = find(id)?;
    Browser::attach(port, &route, name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::browser::Outcome;
    use crate::capture::Log;
    use crate::content_events::NoOne;
    use crate::content_fetch::{self, Capture};
    use crate::content_headless::{Headless, Render, ended, run, settle};
    use crate::content_image::{Images, Plan as ImagePlan};
    use crate::content_store::{Access, Store};
    use crate::content_test::{PEER_ROUTE, Peer, chrome, page, snapshot, state};
    use crate::fetch::{Fetcher, Refusal};
    use crate::image_pick::Found;
    use crate::image_store;

    fn known(lines: &[(&str, Option<Tier>, Status, &str)]) -> content_store::Log {
        let mut log = content_store::Log::default();
        for (url, tier, status, reason) in lines {
            let mut line = Line::new(url, *status).with_reason(*reason);
            line.tier = *tier;
            line.final_url = Some("https://login.a.test/signin".to_owned());
            log.pages.insert((*url).to_owned(), line);
        }
        log
    }

    #[test]
    fn the_switch_then_the_port_file_say_where_the_browser_listens() {
        let ready = chrome(Some(true), Some("9222\n/devtools/browser/abc\n"));
        assert_eq!(
            discover(ready.path()),
            Ok((9222, "/devtools/browser/abc".to_owned()))
        );
        for (flag, port) in [
            (Some(false), Some("9222\n/devtools/browser/abc\n")),
            (None, Some("9222\n/devtools/browser/abc\n")),
        ] {
            let dir = chrome(flag, port);
            assert_eq!(discover(dir.path()), Err(Unavailable::Off), "{flag:?}");
        }
        for port in [
            Some("0\n/devtools/browser/abc\n"),
            Some("garbage"),
            Some("9222\n"),
            None,
        ] {
            let dir = chrome(Some(true), port);
            assert_eq!(
                discover(dir.path()),
                Err(Unavailable::NotRunning),
                "{port:?}"
            );
        }
        assert_eq!(
            Unavailable::Off.to_string(),
            "Chrome remote debugging is off. Turn it on at chrome://inspect/#remote-debugging, then run again."
        );
        assert_eq!(
            Unavailable::NotRunning.to_string(),
            "Chrome is not running with remote debugging. Open Chrome, check chrome://inspect/#remote-debugging, then run again."
        );
        assert_eq!(
            Unavailable::NotAllowed.to_string(),
            "Chrome did not allow the connection: click Allow when Chrome asks (60 s)."
        );
    }

    #[test]
    fn only_public_tiers_lines_behind_a_login_paywalled_blocked_or_refused_with_403_are_eligible() {
        let line = |tier: Option<Tier>, status: Status, reason: &str| {
            let mut line = Line::new("https://a.test/", status).with_reason(reason);
            line.tier = tier;
            line
        };
        let public = [
            Tier::Web,
            Tier::X,
            Tier::Github,
            Tier::Youtube,
            Tier::Headless,
        ];
        for tier in public {
            for status in [Status::BehindLogin, Status::Paywalled, Status::Blocked] {
                assert!(
                    eligible(&line(Some(tier), status, "HTTP 403")),
                    "{tier:?} {status:?}"
                );
            }
            for status in [Status::Thin, Status::EmptyShell] {
                let refused = "short text; not rendered: HTTP 403";
                assert!(
                    eligible(&line(Some(tier), status, refused)),
                    "{tier:?} {status:?}"
                );
                for reason in [
                    "short text; not rendered: HTTP 404",
                    "short text",
                    "HTTP 403; x",
                ] {
                    assert!(!eligible(&line(Some(tier), status, reason)), "{reason}");
                }
            }
            for status in [
                Status::Ok,
                Status::NotFound,
                Status::NotHtml,
                Status::Media,
                Status::Skipped,
                Status::Error,
                Status::Unavailable,
                Status::Unknown,
            ] {
                assert!(
                    !eligible(&line(Some(tier), status, "HTTP 403")),
                    "{status:?}"
                );
            }
        }
        for tier in [None, Some(Tier::SignedIn), Some(Tier::Other)] {
            assert!(!eligible(&line(tier, Status::BehindLogin, "x")), "{tier:?}");
            assert!(!eligible(&line(
                tier,
                Status::Thin,
                "not rendered: HTTP 403"
            )));
        }
        let unknown: Line = serde_json::from_str(
            r#"{"schema_version":1,"url":"https://a.test/","attempted_at":"2026-10-07T09:00:00Z","status":"blocked","tier":"agent"}"#,
        )
        .unwrap();
        assert!(!eligible(&unknown), "a tier this build does not know");
    }

    #[test]
    fn the_plan_opens_eligible_documents_once_from_their_library_address() {
        let web = Some(Tier::Web);
        let urls = [
            "https://a.test/private",
            "https://a.test/private#part",
            "https://forgotten.test/p#x",
            "https://mail.google.com/mail/u/0/",
            "https://console.tailscale.com/admin",
            "https://b.test/fine",
            "https://c.test/signed",
            "https://d.test/untiered",
        ];
        let snapshots = [snapshot(&urls)];
        let known = known(&[
            (
                urls[0],
                web,
                Status::BehindLogin,
                "redirected to a login page",
            ),
            (
                urls[1],
                web,
                Status::BehindLogin,
                "redirected to a login page",
            ),
            (urls[2], web, Status::Blocked, "HTTP 403"),
            (
                urls[3],
                web,
                Status::BehindLogin,
                "redirected to a login page",
            ),
            (
                urls[4],
                web,
                Status::BehindLogin,
                "redirected to a login page",
            ),
            (urls[5], web, Status::Ok, ""),
            (urls[6], Some(Tier::SignedIn), Status::BehindLogin, "x"),
            (
                urls[7],
                None,
                Status::BehindLogin,
                "login page, not fetched",
            ),
        ]);
        let (plan, work, personal) = plan(
            &snapshots,
            &state(&["https://forgotten.test/p"]),
            &known,
            Options::default(),
        );
        assert_eq!(personal, 2, "mail and a console");
        assert_eq!(plan.skip_counts()[&Skip::Forgotten], 1);
        let todo: Vec<&str> = plan.todo.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(todo, [urls[0], urls[1]]);
        assert!(plan.todo.iter().all(|item| item.why == Why::Render));
        assert_eq!(work.renders.len(), 1, "aliases share one render");
        let render = &work.renders[0];
        assert_eq!(render.url.as_str(), "https://a.test/private");
        assert!(render.pages.iter().all(|line| line.final_url.is_some()));
        assert_eq!(render.pages.len(), 2);
        assert!(work.fetches.is_empty() && work.unsent.is_empty());
        assert_eq!(
            ineligible(&[urls[5].to_owned(), urls[0].to_owned()], &known),
            [format!("not eligible for --signed-in: {}", urls[5])]
        );
        assert_eq!(
            dry_run_notes(1, 2, "Google Chrome"),
            [
                "1 page is opened in Google Chrome signed in, one at a time",
                "personal apps: 2 pages not opened",
            ]
        );
    }

    #[test]
    fn a_signed_in_plan_hints_at_a_separate_public_refetch() {
        let url = "https://a.test/p";
        let known = known(&[(url, Some(Tier::SignedIn), Status::BehindLogin, "x")]);
        let (plan, work, _) = plan(&[snapshot(&[url])], &state(&[]), &known, Options::default());
        assert!(work.renders.is_empty());
        assert_eq!(
            targets::not_fetched_line(&plan, true),
            "not fetched: 1 already fetched; use --refetch without --signed-in to fetch pages publicly again"
        );
        assert_eq!(
            targets::not_fetched_line(&plan, false),
            "not fetched: 1 already fetched; --refetch fetches pages again"
        );
        assert_eq!(
            targets::not_fetched_line(&Plan::default(), true),
            "not fetched: 0 already fetched"
        );
    }

    #[test]
    fn a_run_opens_twenty_five_pages_unless_the_limit_says_otherwise() {
        let urls: Vec<String> = (0..30).map(|i| format!("https://site{i}.test/")).collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let mut known = content_store::Log::default();
        for url in &refs {
            let line = content_fetch::line(url, Tier::Web, Status::Blocked).with_reason("HTTP 403");
            known.pages.insert((*url).to_owned(), line);
        }
        let snapshots = [snapshot(&refs)];
        let (plan, work, _) = plan(&snapshots, &state(&[]), &known, Options::default());
        assert_eq!(
            (plan.todo.len(), plan.more, work.renders.len()),
            (LIMIT, 5, LIMIT)
        );
        let five = Options {
            limit: Some(5),
            ..Options::default()
        };
        let (plan, _, _) = super::plan(&snapshots, &state(&[]), &known, five);
        assert_eq!((plan.todo.len(), plan.more), (5, 25));
    }

    #[test]
    fn every_line_a_signed_in_render_writes_says_signed_in_and_only_new_text_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let url = "https://a.test/p";
        let mut public = content_fetch::line(url, Tier::Web, Status::BehindLogin)
            .with_reason("redirected to a login page");
        public.final_url = Some("https://a.test/login".to_owned());
        let baseline = store
            .record(public, Some(&page("# Kept\n\nSign in.\n", 8)))
            .unwrap();
        let file = content_store::dir(dir.path()).join(content_store::file_name(url));
        let before = std::fs::read(&file).unwrap();
        let read = |chars: usize| {
            let mut line = content_fetch::line(url, Tier::SignedIn, Status::Ok);
            line.final_url = Some(url.to_owned());
            Outcome::Read(Box::new(Capture {
                line,
                page: Some(page("# Kept\n\nThe page, signed in.\n", chars)),
                images: Found::Unread,
            }))
        };
        let mut login = content_fetch::line(url, Tier::SignedIn, Status::BehindLogin)
            .with_reason("redirected to a login page");
        login.final_url = Some("https://accounts.a.test/signin".to_owned());
        let was = "redirected to a login page";
        let rows = [
            (read(8), format!("{was}; rendering added no text")),
            (Outcome::SignIn(Box::new(login)), was.to_owned()),
            (
                Outcome::Refused(Refusal::PersonalApp),
                format!("{was}; not rendered: personal app"),
            ),
            (
                Outcome::Refused(Refusal::PrivateAddress),
                format!("{was}; not rendered: private network address"),
            ),
            (
                Outcome::NotRendered("HTTP 404".to_owned()),
                format!("{was}; not rendered: HTTP 404"),
            ),
        ];
        for (outcome, reason) in &rows {
            let (line, page) = settle(&baseline, outcome, Tier::SignedIn);
            assert!(page.is_none(), "{reason}");
            let written = store.record(line, page).unwrap();
            assert_eq!(
                (written.tier, written.access),
                (Some(Tier::SignedIn), Some(Access::SignedIn)),
                "{reason}"
            );
            assert_eq!(written.reason.as_deref(), Some(reason.as_str()));
            assert_eq!(written.status, Status::BehindLogin, "never an error");
            assert_eq!(
                (written.chars, &written.content_sha256),
                (baseline.chars, &baseline.content_sha256)
            );
            assert_eq!(std::fs::read(&file).unwrap(), before, "file untouched");
        }
        let (signed, _) = settle(&baseline, &rows[1].0, Tier::SignedIn);
        assert_eq!(
            signed.final_url.as_deref(),
            Some("https://accounts.a.test/signin")
        );

        let more = read(40);
        let (line, page) = settle(&baseline, &more, Tier::SignedIn);
        let written = store.record(line, page).unwrap();
        assert_eq!(
            (written.status, written.tier, written.access, written.chars),
            (
                Status::Ok,
                Some(Tier::SignedIn),
                Some(Access::SignedIn),
                Some(40)
            )
        );
        let text = std::fs::read_to_string(&file).unwrap();
        let (front, body) = content_store::parse(&text).unwrap();
        assert_eq!(
            (&front["tier"], &front["access"]),
            (&"signed_in".into(), &"signed_in".into())
        );
        assert!(body.ends_with("The page, signed in.\n"));
    }

    #[test]
    fn a_signed_in_image_attempt_requires_newly_kept_text() {
        let urls = [
            "https://a.test/p",
            "https://a.test/p#part",
            "https://b.test/fails",
            "https://c.test/forgotten",
        ];
        let snapshots = [snapshot(&urls)];
        let state = state(&[urls[3]]);
        let baseline = |url: &str| content_fetch::line(url, Tier::Web, Status::BehindLogin);
        let mut unchanged = baseline(urls[1]);
        unchanged.chars = Some(10_000);
        let mut work = Work::default();
        work.renders = vec![
            Render::of(vec![baseline(urls[0]), unchanged], Found::TextOnly, library).unwrap(),
            Render::of(vec![baseline(urls[2])], Found::TextOnly, library).unwrap(),
            Render::of(vec![baseline(urls[3])], Found::TextOnly, library).unwrap(),
        ];
        for closing in [None, Some("Target.createTarget")] {
            let root = tempfile::tempdir().unwrap();
            let plan = ImagePlan::new(
                root.path(),
                &snapshots,
                &state,
                &work,
                Options::default(),
                Log::default(),
            )
            .unwrap()
            .without_retries();
            let images = Images::open(root.path(), plan, &NoOne, Log::default()).unwrap();
            let peer = Peer::start(closing);
            let name = "Google Chrome".to_owned();
            let browser = Browser::attach(peer.port, PEER_ROUTE, name.clone()).unwrap();
            let store = Mutex::new(Store::open(root.path()).unwrap());
            let mut headless = Headless::signed_in(name, 0);
            run(
                &work.renders,
                Ok(browser),
                &Fetcher::new(&state.forgotten),
                Some(&images),
                |line, page| store.lock().unwrap().record(line, page),
                &mut headless,
                Log::default(),
            )
            .unwrap();
            let text = content_store::read(root.path()).unwrap();
            let pictured = image_store::read(root.path()).unwrap();
            if closing.is_none() {
                assert_eq!(text.pages.len(), urls.len());
                assert_eq!(
                    pictured.pages.len(),
                    1,
                    "only newly kept text gets an image attempt"
                );
                assert_eq!(
                    pictured.pages[urls[0]].reason.as_deref(),
                    Some("no_candidate")
                );
                assert!(text.pages[urls[0]].chars.unwrap() > 0);
                assert_eq!(text.pages[urls[1]].chars, Some(10_000));
                let file = content_store::dir(root.path()).join(content_store::file_name(urls[0]));
                let saved = std::fs::read_to_string(file).unwrap();
                let (front, _) = content_store::parse(&saved).unwrap();
                assert_eq!(
                    (&front["tier"], &front["access"]),
                    (&"signed_in".into(), &"signed_in".into())
                );
            } else {
                assert!(text.pages.is_empty());
                assert!(
                    pictured.pages.is_empty(),
                    "a broken browser keeps no image attempts"
                );
                assert_eq!(headless.waiting, urls.len());
            }
            drop(images);
            assert!(
                peer.received()
                    .iter()
                    .all(|message| { message["method"] != "Browser.close" })
            );
        }
    }

    #[test]
    fn a_signed_in_browser_that_goes_away_leaves_its_pages_waiting_and_unwritten() {
        let peer = Peer::start(Some("Target.createTarget"));
        let name = "Google Chrome".to_owned();
        let browser = Browser::attach(peer.port, PEER_ROUTE, name.clone()).unwrap();
        let first = content_fetch::line("https://a.test/p", Tier::Web, Status::BehindLogin);
        let mut alias = first.clone();
        alias.url = "https://a.test/p#part".to_owned();
        let render = Render::of(vec![first, alias], Found::Unread, ended).unwrap();
        let written = Mutex::new(Vec::new());
        let mut headless = Headless::signed_in(name, 3);
        run(
            &[render],
            Ok(browser),
            &Fetcher::new(&BTreeSet::new()),
            None,
            |line, _| {
                written.lock().unwrap().push(line.clone());
                Ok(line)
            },
            &mut headless,
            Log::default(),
        )
        .unwrap();
        assert!(written.into_inner().unwrap().is_empty(), "nothing written");
        assert_eq!(headless.waiting, 2);
        assert_eq!(headless.renders, Vec::<Duration>::new());
        assert!(
            headless
                .why
                .as_deref()
                .is_some_and(|why| why.starts_with("Google Chrome stopped answering"))
        );
        assert_eq!(
            peer.received().last().unwrap()["method"],
            "Target.createTarget"
        );
    }
}
