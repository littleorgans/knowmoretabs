//! What a content run tells a caller following one page, in order, run
//! against a loopback site and no network.
//!
//! slice: content
//! why: `add` shows each step as the run takes it, so the order the run
//!      tells them in is the contract: the tier before the read, a wait
//!      before its retry, the text before its image, and a page a browser
//!      renders after is told by its render, not its first read.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::content_plan::plan;
use crate::content_store::Status;
use crate::content_test::{
    PEER_ROUTE, Peer, article, html, log as content_log, no_browser, no_gh, no_ytdlp, site,
    snapshot, state,
};
use crate::image_store;

const URL: &str = "http://a.test/note";

/// What a run told, one word or two per call.
#[derive(Default)]
struct Heard(Mutex<Vec<String>>);

impl Heard {
    fn hear(&self, said: String) {
        self.0.lock().unwrap().push(said);
    }
}

impl Events for Heard {
    fn reading(&self, tier: Tier) {
        self.hear(format!("reading {}", serde_json::json!(tier)));
    }
    fn retrying(&self, wait: Duration) {
        assert!(wait >= Duration::from_secs(2), "{wait:?}");
        self.hear("retrying".to_owned());
    }
    fn content(&self, line: &Line) {
        let http = line.http_status.map(|http| format!(" {http}"));
        self.hear(format!(
            "content {}{}",
            line.status.word(),
            http.unwrap_or_default()
        ));
    }
    fn imaging(&self) {
        self.hear("imaging".to_owned());
    }
    fn image(&self, line: &image_store::Line) {
        self.hear(format!("image {}", line.status.word()));
    }
}

/// Runs content for [`URL`] in the archive at `root` against `site`, with
/// renders started from `start`, as `add` runs it; what it told, in order.
fn follow(root: &Path, site: SocketAddr, start: Option<Start>) -> Vec<String> {
    let snapshots = [snapshot(&[URL])];
    let state = state(&[]);
    let known = content_store::read(root).unwrap();
    let (_, work) = plan(
        &snapshots,
        &state,
        &known,
        Options::default(),
        no_gh,
        no_ytdlp,
        no_browser,
    );
    let images = content_image::Plan::new(
        root,
        &snapshots,
        &state,
        &work,
        Options::default(),
        Log::default(),
    )
    .unwrap();
    let heard = Arc::new(Heard::default());
    let events: Arc<dyn Events> = heard.clone();
    let fetcher = content_events::following(
        Fetcher::resolving_to(site, Duration::from_millis(400)),
        Some(&events),
    );
    let reach = Reach {
        fetcher: &fetcher,
        start,
        events: &*heard,
    };
    let quiet = Log {
        quiet: true,
        verbose: false,
    };
    run(
        root,
        work,
        Some(images),
        Headless::new(None, 0),
        reach,
        quiet,
    )
    .unwrap();
    heard.0.lock().unwrap().clone()
}

/// The page at [`URL`] answers `page`, and nothing else is there: no image.
fn only(page: impl Fn(usize) -> Option<String> + Send + Sync + 'static) -> SocketAddr {
    site(move |path, n| {
        if path == "/note" {
            page(n)
        } else {
            Some(html(404, ""))
        }
    })
}

#[test]
fn a_page_read_whole_is_told_by_its_tier_then_its_text_then_its_image() {
    let root = tempfile::tempdir().unwrap();
    let site = only(|_| Some(html(200, &article("One note", false))));
    assert_eq!(
        follow(root.path(), site, None),
        ["reading \"web\"", "content ok 200", "imaging", "image none"]
    );
}

#[test]
fn final_statuses_are_told_as_recorded_with_their_http_status() {
    for (status, said) in [(403, "content blocked 403"), (404, "content not_found 404")] {
        let root = tempfile::tempdir().unwrap();
        let site = only(move |_| Some(html(status, "<title>no</title>")));
        let heard = follow(root.path(), site, None);
        assert_eq!(heard[..2], ["reading \"web\"", said], "{status}");
    }
}

#[test]
fn a_timeout_is_told_as_a_wait_before_the_retry() {
    let root = tempfile::tempdir().unwrap();
    let site = only(|n| (n > 1).then(|| html(200, &article("Slow", false))));
    assert_eq!(
        follow(root.path(), site, None),
        [
            "reading \"web\"",
            "retrying",
            "content ok 200",
            "imaging",
            "image none"
        ]
    );
}

#[test]
fn a_page_rendered_after_is_told_by_its_render_not_its_first_read() {
    // A browser that does not start leaves the page as it read: nothing
    // more is told of its text, and its image follows the first read.
    let root = tempfile::tempdir().unwrap();
    let site = only(|_| Some(html(200, &article("Short", true))));
    let missing = Start::Launch(PathBuf::from("/nowhere/no-browser"));
    assert_eq!(
        follow(root.path(), site, Some(missing)),
        [
            "reading \"web\"",
            "reading \"headless\"",
            "imaging",
            "image none"
        ]
    );
    let line = &content_store::read(root.path()).unwrap().pages[URL];
    assert_eq!((line.status, line.tier), (Status::Thin, Some(Tier::Web)));

    // A browser that renders it: the text is told once, as rendered.
    let root = tempfile::tempdir().unwrap();
    let peer = Peer::start(None);
    let browser = Browser::attach(peer.port, PEER_ROUTE, "Chrome".to_owned()).unwrap();
    let heard = follow(root.path(), site, Some(Start::Attached(Box::new(browser))));
    assert_eq!(
        heard,
        ["reading \"web\"", "content ok 200", "imaging", "image none"],
        "an attached browser is told of before it is reached, by the run that attaches"
    );
    let line = &content_store::read(root.path()).unwrap().pages[URL];
    assert_eq!((line.status, line.tier), (Status::Ok, Some(Tier::SignedIn)));
}

#[test]
fn a_rerun_reads_again_only_after_an_error_and_otherwise_tells_nothing() {
    let root = tempfile::tempdir().unwrap();
    let site = only(|n| {
        Some(if n == 1 {
            html(500, "")
        } else {
            html(200, &article("Back", false))
        })
    });
    assert_eq!(
        follow(root.path(), site, None),
        ["reading \"web\"", "content error 500"],
        "a failed text waits for its text before an image"
    );
    assert_eq!(
        follow(root.path(), site, None),
        ["reading \"web\"", "content ok 200", "imaging", "image none"]
    );
    assert_eq!(follow(root.path(), site, None), Vec::<String>::new());
}

#[test]
fn a_page_a_rule_keeps_home_is_told_as_skipped_and_never_recorded() {
    let snapshots = [snapshot(&["http://a.test/reset?token=abc123"])];
    let (plan, _) = plan(
        &snapshots,
        &state(&[]),
        &content_log(&[]),
        Options::default(),
        no_gh,
        no_ytdlp,
        no_browser,
    );
    let heard = Heard::default();
    content_events::kept_home(&plan, &heard);
    assert_eq!(heard.0.into_inner().unwrap(), ["content skipped"]);
}
