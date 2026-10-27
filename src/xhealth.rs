//! The toolbar extension's health on X: the report it sends `serve`, kept
//! as `extension.json` at the archive root, and the `doctor` line it makes.
//!
//! slice: x-bookmarks
//! why: The extension finds X's bookmark button by X's own markup, which X
//!      changes without notice, and a save that never starts fails in
//!      silence: nothing reaches the Library to say so. So the extension
//!      counts what it sees on each page of X and says when it sees posts
//!      but no bookmark button, or clicks that X never shows as saved, and
//!      `doctor` turns the last report, or its absence or age, into one
//!      line. A bookmark the Library did not take while it was down is
//!      dropped, not kept, so the report also counts the drops and says
//!      when the last was, and `doctor` says so for two weeks. The report
//!      holds a state, times and counts, never an address, a handle or a
//!      word of a post, and anything else is refused. It is one small file replaced whole under the archive
//!      lock, so a reader sees the old report or the new one.

use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::archive::{self, Archive};
use crate::doctor::{Check, State};
use crate::error::Error;

pub const FILE: &str = "extension.json";
/// A report older than this says the extension has gone quiet; a drop
/// older than this is no longer news.
const FRESH: SignedDuration = SignedDuration::from_hours(14 * 24);
/// More drops than anyone bookmarks, the bound the extension counts to.
const DROPPED_MAX: u32 = 1_000_000;
const TIER: &str = "extension";

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// What the extension sees on X.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ok,
    /// Posts on several pages in a row, and no bookmark button on any.
    Blind,
    /// Several bookmark clicks in a row X never showed as saved.
    Unconfirmed,
}

/// The body the extension posts: its state since `since`, and what it
/// counted meanwhile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub state: Health,
    pub since: Timestamp,
    /// The extension's manifest version.
    pub version: String,
    pub views: u32,
    pub posts: u32,
    pub controls: u32,
    pub intents: u32,
    pub confirmed: u32,
    /// Bookmarks dropped because the Library did not take them.
    pub dropped: u32,
    /// When the last was dropped; null when none was.
    pub last_dropped: Option<Timestamp>,
}

impl Report {
    /// Why the report is refused, when it is: a version that is not a
    /// manifest version, one to four dot separated numbers; more drops
    /// than the bound; or drops without the time of the last, or a time
    /// without drops.
    pub fn refusal(&self) -> Option<&'static str> {
        let parts: Vec<&str> = self.version.split('.').collect();
        let numbers = parts
            .iter()
            .all(|part| (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()));
        if !(1..=4).contains(&parts.len()) || !numbers {
            return Some("version is not a manifest version");
        }
        if self.dropped > DROPPED_MAX {
            return Some("dropped is more than the extension counts");
        }
        ((self.dropped == 0) != self.last_dropped.is_none())
            .then_some("last_dropped is null exactly when dropped is 0")
    }
}

/// The report as written: the body and when it arrived.
#[derive(Serialize)]
struct Written<'a> {
    #[serde(flatten)]
    report: &'a Report,
    reported_at: Timestamp,
}

/// What `doctor` reads back of the kept report.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Record {
    pub state: Health,
    pub since: Timestamp,
    pub reported_at: Timestamp,
    pub dropped: u32,
    pub last_dropped: Option<Timestamp>,
}

/// Keeps `report`, received now, in place of the last one.
pub fn record(root: &Path, report: &Report) -> Result<(), Error> {
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| {})?;
    let path = path(root);
    let written = Written {
        report,
        reported_at: archive::now(),
    };
    let bytes = serde_json::to_vec_pretty(&written).map_err(|source| Error::Json {
        path: path.clone(),
        source,
    })?;
    archive::replace_file(&path, &bytes)
}

/// The kept report; `None` when the extension never sent one.
pub fn read(root: &Path) -> Result<Option<Record>, Error> {
    let path = path(root);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::Json { path, source }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io("read", &path)(err)),
    }
}

/// `doctor`'s line for the extension, by its last report at `now`.
pub fn check(root: &Path, now: Timestamp) -> Check {
    line(read(root), now)
}

fn line(kept: Result<Option<Record>, Error>, now: Timestamp) -> Check {
    let record = match kept {
        Ok(Some(record)) => record,
        Ok(None) => {
            return Check::new(TIER, State::Missing, "no report from the toolbar extension")
                .hint("load extension/ in Chrome (lab/tagger/README.md)");
        }
        Err(err) => {
            return Check::new(TIER, State::Degraded, format!("report unreadable: {err}"))
                .hint("the next page the extension sees on x.com replaces it");
        }
    };
    let since = day(record.since);
    match record.state {
        Health::Blind => Check::new(
            TIER,
            State::Degraded,
            format!("sees posts on X but no bookmark button since {since}"),
        )
        .hint("X changed its page; bookmarks are not saved until the extension is updated"),
        Health::Unconfirmed => Check::new(
            TIER,
            State::Degraded,
            format!("sees bookmark clicks on X it cannot confirm since {since}"),
        )
        .hint("X changed its page; check that bookmarks reach the Library"),
        Health::Ok
            if record
                .last_dropped
                .is_some_and(|last| now.duration_since(last) <= FRESH) =>
        {
            Check::new(TIER, State::Degraded, dropped(&record))
                .hint("keep `just up` running while you bookmark on X")
        }
        Health::Ok if now.duration_since(record.reported_at) > FRESH => Check::new(
            TIER,
            State::Degraded,
            format!("no report since {}", day(record.reported_at)),
        )
        .hint("open x.com in Chrome with the extension loaded"),
        Health::Ok => Check::new(
            TIER,
            State::Ready,
            format!(
                "sees bookmark buttons on X; reported {}",
                day(record.reported_at)
            ),
        ),
    }
}

fn dropped(record: &Record) -> String {
    let last = record.last_dropped.map(day).unwrap_or_default();
    match record.dropped {
        1 => format!(
            "1 bookmark on X was not saved because knowmoretabs was not answering; last {last}"
        ),
        n => format!(
            "{n} bookmarks on X were not saved because knowmoretabs was not answering; last {last}"
        ),
    }
}

fn day(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn kept(state: Health, reported: &str) -> Record {
        Record {
            state,
            since: at("2026-10-01T09:00:00Z"),
            reported_at: at(reported),
            dropped: 0,
            last_dropped: None,
        }
    }

    const NOW: &str = "2026-10-27T09:00:00Z";

    #[test]
    fn the_line_follows_the_state_and_the_age_of_the_last_report() {
        let now = at(NOW);
        let ready = line(Ok(Some(kept(Health::Ok, "2026-10-14T09:00:00Z"))), now);
        assert_eq!(
            (ready.state, ready.detail.as_str()),
            (
                State::Ready,
                "sees bookmark buttons on X; reported 2026-10-14"
            )
        );
        let stale = line(Ok(Some(kept(Health::Ok, "2026-10-12T08:59:59Z"))), now);
        assert_eq!(
            (stale.state, stale.detail.as_str()),
            (State::Degraded, "no report since 2026-10-12")
        );
        let blind = line(Ok(Some(kept(Health::Blind, NOW))), now);
        assert_eq!(blind.state, State::Degraded);
        assert_eq!(
            blind.detail,
            "sees posts on X but no bookmark button since 2026-10-01"
        );
        assert!(blind.hint.unwrap().contains("not saved"));
        let unconfirmed = line(Ok(Some(kept(Health::Unconfirmed, NOW))), now);
        assert_eq!(unconfirmed.state, State::Degraded);
        let missing = line(Ok(None), now);
        assert_eq!(missing.state, State::Missing);
        assert!(missing.hint.unwrap().contains("lab/tagger/README.md"));
    }

    #[test]
    fn drops_in_the_last_two_weeks_degrade_the_line() {
        let now = at(NOW);
        let dropped = |count, last: &str| Record {
            dropped: count,
            last_dropped: Some(at(last)),
            ..kept(Health::Ok, NOW)
        };
        let recent = line(Ok(Some(dropped(3, "2026-10-13T09:00:00Z"))), now);
        assert_eq!(
            (recent.state, recent.detail.as_str()),
            (
                State::Degraded,
                "3 bookmarks on X were not saved because knowmoretabs was not answering; last 2026-10-13"
            )
        );
        assert_eq!(
            recent.hint.as_deref(),
            Some("keep `just up` running while you bookmark on X")
        );
        let one = line(Ok(Some(dropped(1, NOW))), now);
        assert!(
            one.detail.starts_with("1 bookmark on X was not saved"),
            "{}",
            one.detail
        );
        let old = line(Ok(Some(dropped(3, "2026-10-13T08:59:59Z"))), now);
        assert_eq!(old.state, State::Ready, "{}", old.detail);
        let blind = line(
            Ok(Some(Record {
                state: Health::Blind,
                ..dropped(3, NOW)
            })),
            now,
        );
        assert!(
            blind.detail.contains("no bookmark button"),
            "{}",
            blind.detail
        );
    }

    #[test]
    fn a_report_carries_a_state_a_time_counts_and_a_version_only() {
        let body = serde_json::json!({"state": "ok", "since": "2026-10-27T09:00:00Z",
            "version": "0.2.0", "views": 12, "posts": 140, "controls": 140, "intents": 3,
            "confirmed": 3, "dropped": 0, "last_dropped": null});
        let report: Report = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(report.refusal(), None);
        for (field, value) in [
            ("url", serde_json::json!("https://x.com/example_author")),
            ("state", serde_json::json!("lost")),
            ("views", serde_json::json!(-1)),
            ("posts", serde_json::json!(1.5)),
            ("since", serde_json::json!("yesterday")),
            ("dropped", serde_json::json!(-1)),
            ("dropped", serde_json::json!("2")),
            ("last_dropped", serde_json::json!("yesterday")),
        ] {
            let mut bad = body.clone();
            bad[field] = value;
            assert!(
                serde_json::from_value::<Report>(bad).is_err(),
                "{field} refused"
            );
        }
        for version in ["", "0.2.0-beta", "1.2.3.4.5", "v1", "1..2", "123456"] {
            let mut bad = report.clone();
            bad.version = version.to_owned();
            assert!(bad.refusal().is_some(), "{version:?}");
        }
        let last = Some(at(NOW));
        for (dropped, last_dropped, refused) in [
            (2, last, false),
            (DROPPED_MAX, last, false),
            (DROPPED_MAX + 1, last, true),
            (2, None, true),
            (0, last, true),
        ] {
            let drops = Report {
                dropped,
                last_dropped,
                ..report.clone()
            };
            assert_eq!(
                drops.refusal().is_some(),
                refused,
                "{dropped} {last_dropped:?}"
            );
        }
    }
}
