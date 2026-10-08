//! The intake log, `pages/added.jsonl`: pages added one at a time by their
//! address, and the fold that shows them to the library as one snapshot.
//!
//! slice: add
//! why: A snapshot is a whole session, copied verbatim and never written
//!      again, so a single page has no snapshot to join. It is kept instead
//!      as one line in an append only log, and read back as one in-memory
//!      snapshot, id `added`, one window with one tab per page in the order
//!      they were added. Every reader already derives pages from snapshot
//!      tabs, so "not in your library" and "not shown" keep agreeing by
//!      construction, and nothing on disk changes schema. The snapshot is
//!      dated by the first add. The frontend excludes this snapshot when
//!      deciding what is open now.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::jsonl::{self, Keyed};
use crate::metadata;
use crate::model::{self, Snapshot, Source, Stats, Tab, Window};

pub const FILE: &str = "added.jsonl";
/// The id the added pages' snapshot goes by; no snapshot directory can be
/// named this, as those are dates.
pub const SNAPSHOT_ID: &str = "added";
pub const SCHEMA_VERSION: u32 = 1;

pub fn path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(FILE)
}

/// One page added. The latest line for a URL wins, so a later line can
/// carry a title the first one lacked; the page keeps its place from its
/// first line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    #[serde(deserialize_with = "jsonl::schema::<_, SCHEMA_VERSION>")]
    pub schema_version: u32,
    pub url: String,
    pub added_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Keyed for Line {
    fn key(&self) -> &str {
        &self.url
    }
}

impl Line {
    /// A blank title is no title.
    pub fn new(url: &str, added_at: Timestamp, title: Option<&str>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            added_at,
            title: title
                .filter(|title| !title.trim().is_empty())
                .map(ToOwned::to_owned),
        }
    }
}

/// The added pages as one snapshot; `None` when nothing was ever added.
pub fn snapshot(root: &Path) -> Result<Option<Snapshot>, Error> {
    let path = path(root);
    let (lines, _) = jsonl::records::<Line>(&jsonl::text(&path, "read the intake log")?);
    Ok(fold(lines, path))
}

/// One tab per URL, in the order of each URL's first line, titled by its
/// latest line.
fn fold(lines: Vec<Line>, path: PathBuf) -> Option<Snapshot> {
    let first_added = lines.iter().map(|line| line.added_at).min()?;
    let mut places = HashMap::new();
    let mut pages: Vec<Line> = Vec::new();
    for line in lines {
        if let Some(&place) = places.get(&line.url) {
            pages[place] = line;
        } else {
            places.insert(line.url.clone(), pages.len());
            pages.push(line);
        }
    }
    let tabs: Vec<Tab> = pages
        .into_iter()
        .zip(0..)
        .map(|(line, position)| Tab {
            tab_id: position + 1,
            window: 1,
            position,
            url: line.url,
            title: line.title.unwrap_or_default(),
            pinned: false,
            active: false,
            group: None,
            last_active: None,
            window_id: 1,
            history: None,
        })
        .collect();
    let count = u32::try_from(tabs.len()).unwrap_or(u32::MAX);
    Some(Snapshot {
        schema_version: model::SCHEMA_VERSION,
        id: SNAPSHOT_ID.to_owned(),
        captured_at: first_added,
        source: Source {
            browser: None,
            profile: None,
            profile_display: None,
            path,
            file: String::new(),
            sha256: String::new(),
            bytes: 0,
            saved_at: None,
            session_started_at: None,
        },
        stats: Stats {
            marker_count: 1,
            marker_ok: true,
            windows: 1,
            tabs: u64::from(count),
            ..Stats::default()
        },
        windows: vec![Window {
            id: 1,
            number: 1,
            kind: "normal".to_owned(),
            kind_id: 0,
            active_tab: None,
            tabs: count,
        }],
        groups: Vec::new(),
        tabs,
        history: None,
    })
}

#[cfg(test)]
#[path = "intake_tests.rs"]
mod tests;
