//! The report a batch `content` run ends with: what it did, counted.
//!
//! slice: content
//! why: The owner reads a run by its end: how each page ended, by the
//!      run's last line for it, how long fetches took, how images ended,
//!      what waits for a browser or yt-dlp, and what was left for another
//!      run. Counting and wording live apart from the run that captures, so
//!      neither grows past what a reader holds at once.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::capture::Log;
use crate::content_headless::Headless;
use crate::content_image;
use crate::content_store::{self, Line, Status};
use crate::image_store;
use crate::out;
use crate::targets::{self, Counts, Plan};
use crate::triage::plural;

/// What a run did.
#[derive(Debug, Default)]
pub struct Tally {
    /// The run's last line for each library page, so a page rendered after
    /// its HTTP read is counted once, as rendered.
    pub lines: BTreeMap<String, Line>,
    pub fetches: usize,
    /// How long each fetch took, retries and extraction included.
    pub durations: Vec<Duration>,
    /// Video pages left for a run with yt-dlp, unrecorded.
    pub waiting: usize,
    pub headless: Headless,
    /// How each page's image ended, when images were captured.
    pub images: Option<Counts<image_store::Status>>,
}

impl Tally {
    /// How many pages ended in each status, and why.
    fn counts(&self) -> Counts<Status> {
        let mut counts = Counts::default();
        for line in self.lines.values() {
            let reason =
                (line.status != Status::Ok).then(|| line.reason.as_deref().unwrap_or_default());
            counts.add(line.status, reason);
        }
        counts
    }
}

/// Seconds to a tenth.
pub fn seconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10.0).round() / 10.0
}

/// The median and the longest of `durations`.
pub fn spread(durations: &[Duration]) -> Option<(Duration, Duration)> {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    Some((*sorted.get(sorted.len() / 2)?, *sorted.last()?))
}

pub fn report(
    plan: &Plan,
    tally: &Tally,
    notes: &[String],
    took: Duration,
    root: &Path,
    json: bool,
    log: Log,
) {
    let counts = tally.counts();
    let recorded = counts.total();
    let spread = spread(&tally.durations);
    if json {
        let (statuses, reasons) = counts.json();
        let mut report = serde_json::json!({
            "recorded": recorded,
            "fetched": tally.fetches,
            "statuses": statuses,
            "reasons": reasons,
            "deferred": tally.waiting,
            "not_fetched": targets::counts_json(plan)["not_fetched"],
            "more": plan.more,
            "notes": notes,
            "seconds": seconds(took),
            "fetch_seconds": spread.map(|(median, longest)| serde_json::json!({
                "median": seconds(median), "longest": seconds(longest),
            })),
            "headless": tally.headless.json(),
            "log": content_store::log_path(root),
            "dir": content_store::dir(root),
        });
        if let Some(images) = &tally.images {
            report["images"] = content_image::report_json(images, root);
        }
        out::json(&report);
        return;
    }
    if log.quiet {
        return;
    }
    let mut text = String::new();
    if recorded == 0 {
        let _ = writeln!(text, "nothing to capture");
    } else {
        let _ = writeln!(
            text,
            "recorded {} in {} s, {}: {}",
            plural(recorded, "page"),
            took.as_secs_f64().round(),
            match tally.fetches {
                1 => "1 fetch".to_owned(),
                n => format!("{n} fetches"),
            },
            counts.summary(),
        );
        for line in counts.reason_lines() {
            let _ = writeln!(text, "{line}");
        }
        if let Some((median, longest)) = spread {
            let _ = writeln!(
                text,
                "  a fetch took {} s at the median, {} s at the longest",
                seconds(median),
                seconds(longest)
            );
        }
        let _ = writeln!(
            text,
            "text in {}, one line per attempt in {}",
            content_store::dir(root).display(),
            content_store::log_path(root).display()
        );
    }
    if let Some(images) = &tally.images {
        text.push_str(&content_image::report(images, root));
    }
    for line in tally.headless.lines() {
        let _ = writeln!(text, "{line}");
    }
    for note in notes {
        let _ = writeln!(text, "{note}");
    }
    let _ = writeln!(
        text,
        "{}",
        targets::not_fetched_line(plan, tally.headless.signed_in.is_some())
    );
    if plan.more > 0 {
        let _ = writeln!(text, "{} left for another run", plural(plan.more, "page"));
    }
    out::block(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_is_counted_once_by_its_last_line_in_the_run() {
        let mut tally = Tally::default();
        for (url, status) in [
            ("https://a.test/p", Status::Thin),
            ("https://a.test/p#part", Status::Thin),
            ("https://b.test/", Status::Ok),
            ("https://a.test/p", Status::Ok),
            ("https://a.test/p#part", Status::Ok),
        ] {
            tally.lines.insert(url.to_owned(), Line::new(url, status));
        }
        let counts = tally.counts();
        assert_eq!(counts.total(), 3, "one per page, aliases apart");
        assert_eq!(
            counts.summary(),
            "3 ok",
            "the render replaced the HTTP read"
        );
    }

    #[test]
    fn spreads_are_the_median_and_the_longest() {
        let ms = Duration::from_millis;
        assert_eq!(spread(&[]), None);
        assert_eq!(spread(&[ms(5), ms(1), ms(3)]), Some((ms(3), ms(5))));
    }
}
