//! What `content` does about a page's image: which pages get one, which
//! candidates are tried, the line recorded, and earlier failures retried.
//!
//! slice: content
//! why: The image is part of what a capture keeps of a page, so it is
//!      settled in the same run as the text, right after it, from what the
//!      text's route already read: a post's media, a video's thumbnail, a
//!      repository's preview, a page's head and body. A page whose text
//!      this run did not read, behind a login or gone, falls back to the
//!      head `enrich` recorded. A page rendered in a browser gets its image
//!      after the render, from the rendered page, once: when this run read
//!      it over HTTP first, or when it has no image yet. A page whose text
//!      failed waits for its text,
//!      and a page a rule kept home gets no image either. An image that
//!      failed in a way that may pass is retried on the next run from the
//!      candidates its line kept, without fetching the page again, until
//!      it has failed on three runs. Nothing here holds the archive lock
//!      across a request or a decode: the store takes it only to write.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use url::Url;

use crate::capture::Log;
use crate::content_fetch;
use crate::content_plan::Work;
use crate::content_store::{Line as TextLine, Status as Text};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::guard;
use crate::image_fetch::{self, Captured};
use crate::image_pick::{self, Candidate, Found, SiteDefaults};
use crate::image_store::{self, Status, Store};
use crate::library::State;
use crate::metadata::{self, Metadata};
use crate::model::Snapshot;
use crate::targets::{self, Counts, Forgotten, Options, Outcome as _, Why};
use crate::triage::plural;

/// The image work a run will do, settled before any request.
#[derive(Debug)]
pub struct Plan {
    known: image_store::Log,
    metadata: Metadata,
    /// Pages whose text this run settles, each of which gets its image
    /// after: those fetched, and those rendered that have no image yet.
    after_text: HashSet<String>,
    /// Pages whose image failed in an earlier run and whose text is not
    /// fetched in this one.
    retries: Vec<Retry>,
}

/// A page whose image is tried again from the candidates its line kept.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Retry {
    url: String,
    /// The first candidate's host, the queue it waits in.
    host: String,
    candidates: Vec<Candidate>,
}

impl Plan {
    /// Reads the image log and what `enrich` recorded, and finds the
    /// images to retry among the library pages `snapshots` hold. `--limit`
    /// caps the retries as it caps the pages fetched.
    pub fn new(
        root: &Path,
        snapshots: &[Snapshot],
        state: &State,
        work: &Work,
        options: Options,
        log: Log,
    ) -> Result<Self, Error> {
        let known = image_store::read(root)?;
        if let Some(note) = known.unreadable_note(&image_store::log_path(root)) {
            log.warn(&note);
        }
        let fetched = work
            .fetches
            .iter()
            .flat_map(|fetch| fetch.pages.iter().map(|(url, _)| url));
        let rendered = work
            .renders
            .iter()
            .flat_map(|render| render.pages.iter().map(|line| &line.url))
            .filter(|url| !known.pages.contains_key(*url));
        let after_text: HashSet<String> = fetched.chain(rendered).cloned().collect();
        let retries = retries(snapshots, state, &known, &after_text, options.limit);
        Ok(Self {
            metadata: metadata::read(root)?,
            known,
            after_text,
            retries,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.after_text.is_empty() && self.retries.is_empty()
    }

    /// What `--dry-run` says about images, when there is image work.
    pub fn note(&self) -> Option<String> {
        note(self.after_text.len(), self.retries.len())
    }
}

/// Names only the work there is: a run with only retries pending used to
/// promise images "for each of up to 0 pages".
fn note(after_text: usize, retries: usize) -> Option<String> {
    let parts: Vec<String> = [
        (after_text > 0).then(|| {
            format!(
                "one for each of up to {} once its text is settled",
                plural(after_text, "page")
            )
        }),
        (retries > 0).then(|| format!("{} retried from an earlier run", plural(retries, "page"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| format!("images: {}", parts.join(", and ")))
}

/// The library pages whose latest image line is `error`, by the shared
/// rules, other than those whose image follows their text in this run.
fn retries(
    snapshots: &[Snapshot],
    state: &State,
    known: &image_store::Log,
    after_text: &HashSet<String>,
    limit: Option<usize>,
) -> Vec<Retry> {
    let plan = targets::plan(
        snapshots,
        state,
        |url| image_store::recorded(known, url),
        Options::default(),
    );
    let forgotten = Forgotten::of(state);
    let mut retries: Vec<Retry> = plan
        .todo
        .into_iter()
        .filter(|item| {
            item.why == Why::Retry
                && !after_text.contains(&item.url)
                && !forgotten.covers(&item.url)
        })
        .filter_map(|item| {
            let candidates = known.pages.get(&item.url)?.candidates.clone();
            let host = candidates
                .first()
                .and_then(|first| Url::parse(&first.url).ok())
                .map(|url| guard::host_key(&url))
                .unwrap_or_default();
            Some(Retry {
                url: item.url,
                host,
                candidates,
            })
        })
        .collect();
    if let Some(limit) = limit {
        retries.truncate(limit);
    }
    retries
}

/// A run's image work: the store it writes, and what it counted.
pub struct Images {
    plan: Plan,
    store: Mutex<Store>,
    defaults: SiteDefaults,
    counts: Mutex<Counts<Status>>,
    log: Log,
}

impl Images {
    /// Opens the image store, which clears what an interrupted run left
    /// staged in it.
    pub fn open(root: &Path, plan: Plan, log: Log) -> Result<Self, Error> {
        Ok(Self {
            store: Mutex::new(Store::open(root)?),
            defaults: SiteDefaults::of(&plan.metadata),
            plan,
            counts: Mutex::new(Counts::default()),
            log,
        })
    }

    /// The image of the pages one fetch or render stood for, by the
    /// `lines` their text was recorded with, and what it `found`. Fetched
    /// once and recorded for each page whose text is settled and whose
    /// image this run planned.
    pub fn after(&self, fetcher: &Fetcher, lines: &[TextLine], found: &Found) -> Result<(), Error> {
        let settled: Vec<&str> = lines
            .iter()
            .filter(|line| wants_image(line.status) && self.plan.after_text.contains(&line.url))
            .map(|line| line.url.as_str())
            .collect();
        let Some(first) = settled.first() else {
            return Ok(());
        };
        let candidates = match found {
            Found::TextOnly => {
                let none = Captured::ended(Status::None, "text_only_post", &[]);
                return settled.iter().try_for_each(|url| self.record(url, &none));
            }
            Found::Candidates(candidates) => candidates.clone(),
            Found::Unread => settled
                .iter()
                .find_map(|url| self.plan.metadata.pages.get(*url))
                .map(image_pick::recorded)
                .unwrap_or_default(),
        };
        let candidates = match Url::parse(first) {
            Ok(page) => image_pick::ladder(candidates, &page, &self.defaults),
            Err(_) => candidates,
        };
        let captured = image_fetch::capture(fetcher, &candidates);
        settled
            .iter()
            .try_for_each(|url| self.record(url, &captured))
    }

    /// Tries again the images that failed in an earlier run, each host's
    /// in order on the shared workers.
    pub fn retry(&self, fetcher: &Fetcher) -> Result<(), Error> {
        targets::by_host(
            targets::WORKERS,
            self.plan
                .retries
                .iter()
                .map(|retry| (retry.host.as_str(), retry)),
            |retry| {
                let captured = image_fetch::capture(fetcher, &retry.candidates);
                self.record(&retry.url, &captured)
            },
            |_, _| {},
        )
    }

    /// Records `captured` for page `url`, numbered for this run.
    fn record(&self, url: &str, captured: &Captured) -> Result<(), Error> {
        let mut line = captured.line.clone();
        url.clone_into(&mut line.url);
        let attempt = content_fetch::next_attempt(self.plan.known.pages.get(url));
        let line = self
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(
                content_fetch::settle(line, attempt),
                captured.jpeg.as_deref(),
            )?;
        let reason =
            (line.status != Status::Ok).then(|| line.reason.as_deref().unwrap_or_default());
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(line.status, reason);
        self.log.note(&format!(
            "image {} {url}{}",
            line.status.word(),
            reason.map(|r| format!(" ({r})")).unwrap_or_default()
        ));
        Ok(())
    }

    pub fn counts(self) -> Counts<Status> {
        self.counts
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A page whose text attempt has settled gets an image; one whose text
/// failed waits for it, and one a rule kept home gets none.
fn wants_image(text: Text) -> bool {
    !matches!(text, Text::Error | Text::Skipped)
}

/// What a run's report says about images.
pub fn report(counts: &Counts<Status>, root: &Path) -> String {
    let mut text = String::new();
    if counts.total() == 0 {
        return text;
    }
    let _ = writeln!(text, "images: {}", counts.summary());
    for line in counts.reason_lines() {
        let _ = writeln!(text, "{line}");
    }
    let _ = writeln!(
        text,
        "images in {}, one line per attempt in {}",
        image_store::dir(root).display(),
        image_store::log_path(root).display()
    );
    text
}

/// The same, for `--json`.
pub fn report_json(counts: &Counts<Status>, root: &Path) -> serde_json::Value {
    let (statuses, reasons) = counts.json();
    serde_json::json!({
        "recorded": counts.total(),
        "statuses": statuses,
        "reasons": reasons,
        "log": image_store::log_path(root),
        "dir": image_store::dir(root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_test::{snapshot, state};
    use crate::image_pick::Source;

    fn error_line(url: &str, image: &str) -> image_store::Line {
        let mut line = image_store::Line::new(url, Status::Error).with_reason("HTTP 503");
        line.candidates = vec![Candidate {
            url: image.to_owned(),
            source: Source::OgImage,
        }];
        line
    }

    fn known(lines: Vec<image_store::Line>) -> image_store::Log {
        let mut log = image_store::Log::default();
        for line in lines {
            log.pages.insert(line.url.clone(), line);
        }
        log
    }

    #[test]
    fn failed_images_are_retried_from_their_candidates_by_the_shared_rules() {
        let snapshots = [snapshot(&[
            "https://a.test/one",
            "https://a.test/two",
            "https://a.test/three#part",
            "https://a.test/fetched",
            "https://a.test/ok",
            "https://192.168.1.1/admin",
        ])];
        let state = state(&["https://a.test/three"]);
        let mut ok = image_store::Line::new("https://a.test/ok", Status::Ok);
        ok.image_url = Some("https://cdn.test/ok.jpg".to_owned());
        let known = known(vec![
            error_line("https://a.test/one", "https://cdn.test/1.jpg"),
            error_line("https://a.test/two", "https://img.test/2.jpg"),
            error_line("https://a.test/three#part", "https://cdn.test/3.jpg"),
            error_line("https://a.test/fetched", "https://cdn.test/f.jpg"),
            error_line("https://192.168.1.1/admin", "https://cdn.test/p.jpg"),
            ok,
        ]);
        let fetched: HashSet<String> = ["https://a.test/fetched".to_owned()].into();
        let planned = retries(&snapshots, &state, &known, &fetched, None);
        let named: Vec<(&str, &str)> = planned
            .iter()
            .map(|r| (r.url.as_str(), r.host.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("https://a.test/one", "cdn.test"),
                ("https://a.test/two", "img.test")
            ],
            "a forgotten page's other fragment, a page fetched for its text, \
             the private network and a kept image are not retried"
        );
        assert_eq!(planned[0].candidates[0].url, "https://cdn.test/1.jpg");
        assert_eq!(
            retries(&snapshots, &state, &known, &fetched, Some(1)).len(),
            1
        );
    }

    #[test]
    fn a_page_gets_its_image_after_its_text_once_and_is_not_retried_too() {
        use crate::content_headless::Render;
        use crate::content_plan::Fetch;
        use crate::content_route::Route;

        let snapshots = [snapshot(&[
            "https://a.test/fetched",
            "https://a.test/rendered",
            "https://a.test/pictured",
            "https://a.test/failed",
        ])];
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let mut ok = image_store::Line::new("https://a.test/pictured", Status::Ok);
        ok.image_url = Some("https://cdn.test/p.jpg".to_owned());
        for line in [
            error_line("https://a.test/fetched", "https://cdn.test/f.jpg"),
            ok,
            error_line("https://a.test/failed", "https://cdn.test/x.jpg"),
        ] {
            store.record(line, None).unwrap();
        }
        let render = |urls: &[&str]| {
            let pages = urls
                .iter()
                .map(|url| TextLine::new(url, Text::Thin))
                .collect();
            Render::of(pages, Found::Unread).unwrap()
        };
        let mut work = Work::default();
        work.fetches = vec![Fetch {
            url: "https://a.test/fetched".to_owned(),
            host: "a.test".to_owned(),
            route: Route::Web,
            pages: vec![("https://a.test/fetched".to_owned(), 1)],
        }];
        work.renders = vec![
            render(&["https://a.test/rendered"]),
            render(&["https://a.test/pictured"]),
        ];
        let plan = Plan::new(
            root.path(),
            &snapshots,
            &state(&[]),
            &work,
            Options::default(),
            Log::default(),
        )
        .unwrap();
        let mut after: Vec<&str> = plan.after_text.iter().map(String::as_str).collect();
        after.sort_unstable();
        assert_eq!(
            after,
            ["https://a.test/fetched", "https://a.test/rendered"],
            "a rendered page with an image keeps it"
        );
        let retried: Vec<&str> = plan.retries.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            retried,
            ["https://a.test/failed"],
            "a page whose image follows its text is not retried as well"
        );
        assert_eq!(
            plan.note().as_deref(),
            Some(
                "images: one for each of up to 2 pages once its text is settled, \
                 and 1 page retried from an earlier run"
            )
        );
    }

    #[test]
    fn only_pages_whose_text_settled_without_a_rule_get_an_image() {
        for text in [
            Text::Ok,
            Text::Thin,
            Text::EmptyShell,
            Text::BehindLogin,
            Text::Paywalled,
            Text::Blocked,
            Text::NotFound,
            Text::NotHtml,
            Text::Unavailable,
        ] {
            assert!(wants_image(text), "{text:?}");
        }
        assert!(!wants_image(Text::Error));
        assert!(!wants_image(Text::Skipped));
    }

    #[test]
    fn the_dry_run_note_names_only_the_work_there_is() {
        assert_eq!(note(0, 0), None);
        assert_eq!(
            note(10, 0).as_deref(),
            Some("images: one for each of up to 10 pages once its text is settled")
        );
        assert_eq!(
            note(0, 3).as_deref(),
            Some("images: 3 pages retried from an earlier run")
        );
        assert_eq!(
            note(1, 1).as_deref(),
            Some(
                "images: one for each of up to 1 page once its text is settled, \
                 and 1 page retried from an earlier run"
            )
        );
    }

    #[test]
    fn the_report_names_counts_reasons_and_where_images_are() {
        let mut counts = Counts::default();
        counts.add(Status::Ok, None);
        counts.add(Status::None, Some("no_candidate"));
        counts.add(Status::None, Some("no_candidate"));
        let root = Path::new("/archive");
        let text = report(&counts, root);
        assert!(text.starts_with("images: 1 ok, 2 none\n  none: 2 no_candidate\n"));
        assert!(text.contains("one line per attempt in"));
        assert_eq!(report(&Counts::default(), root), "");
        let json = report_json(&counts, root);
        assert_eq!(json["recorded"], 3);
        assert_eq!(json["statuses"]["none"], 2);
        assert_eq!(json["reasons"]["none"]["no_candidate"], 2);
    }
}
