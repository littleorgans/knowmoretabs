//! What a `content` run tells a caller following its pages as it
//! happens.
//!
//! slice: content
//! why: A caller adding one page shows its progress while the run works,
//!      so the run says which tier reads a page, each wait before a retry,
//!      and when a page's text and image settle, from the places it already
//!      decides those. A batch run is followed by no one and reports at the
//!      end, so it pays nothing and its report does not change.

use std::sync::Arc;
use std::time::Duration;

use crate::content_store::{Line, Status, Tier};
use crate::fetch::Fetcher;
use crate::image_store;
use crate::targets::{Plan, Skip};

/// What a run tells a caller following its pages, as it happens; each
/// is ignored unless said otherwise. Calls come from the run's workers.
pub trait Events: Send + Sync {
    /// A page is being read by `tier`: its route's, or a browser's after.
    fn reading(&self, _tier: Tier) {}
    /// A failure that may pass; the next try comes after `wait`.
    fn retrying(&self, _wait: Duration) {}
    /// A page's text settled for this run, as `line` says.
    fn content(&self, _line: &Line) {}
    /// A page's image is being fetched.
    fn imaging(&self) {}
    /// A page's image settled for this run, as `line` says.
    fn image(&self, _line: &image_store::Line) {}
}

/// Who follows a batch run: no one.
pub struct NoOne;

impl Events for NoOne {}

/// `fetcher`, telling `events` of each wait before a retry.
pub fn following(fetcher: Fetcher, events: Option<&Arc<dyn Events>>) -> Fetcher {
    match events {
        Some(events) => {
            let events = Arc::clone(events);
            fetcher.telling(Arc::new(move |wait| events.retrying(wait)))
        }
        None => fetcher,
    }
}

/// Pages a shared rule keeps home are counted, never recorded; a caller
/// following the run hears each as skipped, and why. Pages that are not
/// documents are recorded, and heard as they are.
pub fn kept_home(plan: &Plan, events: &dyn Events) {
    for (url, skip) in &plan.not_fetched {
        if *skip != Skip::NotADocument {
            events.content(&Line::new(url, Status::Skipped).with_reason(skip.label()));
        }
    }
}
