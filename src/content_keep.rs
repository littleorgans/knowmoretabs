//! A capture made outside a `content` run, kept for one library page: its
//! text by the rule every later reading keeps, its image a separate step.
//!
//! slice: content, x-bookmarks
//! why: A bookmark on X reads its page twice over, from the page the
//!      owner was looking at and from the public post API, and a page may
//!      already hold a longer text from an earlier save. Whichever came
//!      last must not lose text, so a capture replaces the kept text only
//!      when it is longer and did not fail, by the rule a render already
//!      follows (`content_fetch::longer`); otherwise the kept line stands,
//!      saying what the new reading added. A page with no kept text takes
//!      any capture, as a refetch would. The caller can keep the image in
//!      the background after the text is safe. It follows a capture the
//!      page took, or a page that has none yet, through the content run's
//!      own image step, so it is chosen and stored as every image is.

use std::path::Path;

use crate::capture::Log;
use crate::content_events::NoOne;
use crate::content_fetch::{self, Capture};
use crate::content_image::{self, Images};
use crate::content_store::{self, Line, Store, Tier};
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::image_pick::Found;

/// What became of one capture.
#[derive(Debug, Clone)]
pub struct Kept {
    /// The page's line now.
    pub line: Line,
    /// The page took the capture; else its kept line stands, noted.
    pub taken: bool,
    images: Found,
}

impl Kept {
    /// Keeps the image from this capture when its text was taken or the
    /// page still has no image. Network work belongs here, apart from
    /// keeping the text, so a bookmark reply need not wait for it.
    pub fn image(&self, root: &Path, fetcher: &Fetcher, log: Log) -> Result<(), Error> {
        let plan = content_image::Plan::page(root, &self.line.url)?;
        if self.taken || !plan.has_image(&self.line.url) {
            Images::open(root, plan, &NoOne, log)?.after(
                fetcher,
                std::slice::from_ref(&self.line),
                &self.images,
            )?;
        }
        Ok(())
    }
}

/// Keeps `capture` for library page `url`, or, when the page keeps a text
/// the capture is not longer than, the page's line copied with `note`,
/// still saying the tier that read the kept text. The returned image step
/// can run after the caller replies.
pub fn keep(root: &Path, url: &str, capture: &Capture, note: &str) -> Result<Kept, Error> {
    let known = content_store::read(root)?;
    let baseline = known.pages.get(url);
    let (line, page) = match baseline {
        Some(baseline)
            if baseline.content_sha256.is_some() && !content_fetch::longer(baseline, capture) =>
        {
            let tier = baseline.tier.or(capture.line.tier).unwrap_or(Tier::Other);
            let note = (!baseline
                .reason
                .as_deref()
                .is_some_and(|reason| reason.split("; ").any(|said| said == note)))
            .then_some(note);
            (content_fetch::copied(baseline, note, tier), None)
        }
        _ => {
            let mut line = capture.line.clone();
            url.clone_into(&mut line.url);
            let attempt = content_fetch::next_attempt(baseline);
            (content_fetch::settle(line, attempt), capture.page.as_ref())
        }
    };
    let taken = page.is_some() || baseline.is_none_or(|baseline| baseline.content_sha256.is_none());
    let line = Store::open(root)?.record(line, page)?;
    Ok(Kept {
        line,
        taken,
        images: capture.images.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_store::{Completeness, Page, Status};
    use crate::image_pick::Found;

    const URL: &str = "https://x.com/example_author/status/102";

    fn capture(tier: Tier, status: Status, text: Option<&str>) -> Capture {
        Capture {
            line: content_fetch::line(URL, tier, status),
            page: text.map(|text| Page {
                title: Some("Example Author (@example_author) on X".to_owned()),
                extractor: "test 1".to_owned(),
                completeness: Completeness::Full,
                chars: text.len(),
                captions: None,
                markdown: format!("{text}\n"),
            }),
            images: Found::TextOnly,
        }
    }

    fn kept_body(root: &Path) -> String {
        content_store::kept_text(root, URL).unwrap().unwrap().body
    }

    #[test]
    fn a_longer_capture_replaces_the_text_and_a_shorter_or_failed_one_is_a_note() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let fetcher = Fetcher::new(&std::collections::BTreeSet::default());
        let keep = |capture: &Capture, note: &str| {
            let kept = keep(root, URL, capture, note).unwrap();
            kept.image(root, &fetcher, Log::default()).unwrap();
            kept
        };

        let read = capture(Tier::SignedIn, Status::Ok, Some("Four posts as shown."));
        let first = keep(&read, "page read added no text");
        assert!(first.taken);
        assert_eq!(first.line.tier, Some(Tier::SignedIn));
        assert_eq!(kept_body(root), "Four posts as shown.\n");

        let shorter = capture(Tier::X, Status::Ok, Some("One post."));
        let behind = capture(Tier::X, Status::BehindLogin, None);
        for public in [&shorter, &behind] {
            let kept = keep(public, "public read added no text");
            assert!(!kept.taken);
            assert_eq!(
                (kept.line.status, kept.line.tier, kept.line.chars),
                (Status::Ok, Some(Tier::SignedIn), Some(20)),
                "the line keeps the kept text's tier"
            );
            assert_eq!(
                kept.line.reason.as_deref(),
                Some("public read added no text")
            );
            assert_eq!(kept_body(root), "Four posts as shown.\n");
        }

        let longer = capture(Tier::X, Status::Ok, Some("Five posts of the whole thread."));
        assert!(keep(&longer, "public read added no text").taken);
        assert_eq!(kept_body(root), "Five posts of the whole thread.\n");
        let images = crate::image_store::read(root).unwrap();
        assert_eq!(
            images.pages[URL].status,
            crate::image_store::Status::None,
            "a text only post has no image"
        );
    }

    #[test]
    fn a_page_with_no_kept_text_takes_any_capture() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let failed = capture(Tier::X, Status::Error, None);
        let first = keep(root, URL, &failed, "n").unwrap();
        assert_eq!((first.taken, first.line.attempt), (true, 1));
        let again = keep(root, URL, &failed, "n").unwrap();
        assert_eq!(
            (again.taken, again.line.attempt, again.line.reason),
            (true, 2, None),
            "a failure is tried again, numbered"
        );
    }
}
