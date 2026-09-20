//! The shape of `snapshot.json`, the archive's source of truth.
//!
//! slice: capture
//! why: Snapshots are immutable and every later slice reads them, so a field
//!      added here is a field every reader must understand for as long as the
//!      archive exists. These are plain data types with serde derives and no
//!      knowledge of how the bytes were parsed: unknown fields are ignored on
//!      read, `schema_version` names the contract, and the degradation
//!      counters in `Stats` are part of the record because a snapshot that
//!      hides what it could not read is a snapshot that lies.

use std::collections::BTreeMap;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Bump when a reader of the previous version could misread the new one.
pub const SCHEMA_VERSION: u32 = 1;
/// The verbatim copy of the browser's file, beside `snapshot.json`.
pub const SESSION_FILE_NAME: &str = "session.snss";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    /// Directory name: UTC, second resolution, lexicographically sortable.
    pub id: String,
    /// When `knowmoretabs save` ran.
    pub captured_at: Timestamp,
    pub source: Source,
    pub stats: Stats,
    pub windows: Vec<Window>,
    pub groups: Vec<Group>,
    pub tabs: Vec<Tab>,
}

/// Where the session file came from and how to recognise it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// `None` when the file was passed with `--session` and its browser is unknown.
    pub browser: Option<String>,
    /// Profile directory name (`Default`, `Profile 1`), never the display name.
    pub profile: Option<String>,
    /// The name the browser shows for that profile, when `Local State` had it.
    pub profile_display: Option<String>,
    /// The file that was read, as it was found.
    pub path: PathBuf,
    /// Name of the verbatim copy inside the snapshot directory.
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    /// The file's modification time: the last moment it reflected the browser.
    pub saved_at: Option<Timestamp>,
    /// Decoded from the `Session_<n>` suffix: when the browser started this log.
    pub session_started_at: Option<Timestamp>,
}

/// What the parser saw, including everything it could not use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub file_version: u32,
    /// Which command-ID table was applied; see `session::CommandTable`.
    pub command_table: String,
    pub commands: u64,
    /// A year of snapshots makes this a longitudinal record of Chrome's format.
    pub commands_by_id: BTreeMap<u8, u64>,
    pub unknown_commands: u64,
    pub unknown_command_ids: Vec<u8>,
    /// Known id, unusable payload.
    pub malformed_commands: u64,
    pub truncated_bytes: u64,
    /// Chrome writes exactly one marker per file; anything else is a quality signal.
    pub marker_count: u64,
    pub marker_ok: bool,
    pub windows: u64,
    pub tabs: u64,
    pub groups: u64,
    pub dropped_tabs: u64,
    pub dropped_tab_reasons: DroppedTabs,
    /// How often the selected navigation had to be approximated.
    pub navigation_fallbacks: u64,
    pub groups_without_metadata: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DroppedTabs {
    pub no_navigations: u64,
    pub window_missing: u64,
    pub window_closed: u64,
}

impl Stats {
    /// True when anything about the parse was not the clean, expected case.
    pub fn is_degraded(&self) -> bool {
        self.unknown_commands > 0
            || self.malformed_commands > 0
            || self.truncated_bytes > 0
            || self.dropped_tabs > 0
            || self.navigation_fallbacks > 0
            || self.groups_without_metadata > 0
            || !self.marker_ok
    }

    /// The one-line degradation summary shown on stdout; empty when clean.
    pub fn degradation_summary(&self) -> String {
        let mut parts = Vec::new();
        if self.unknown_commands > 0 {
            let ids: Vec<String> = self
                .unknown_command_ids
                .iter()
                .map(ToString::to_string)
                .collect();
            parts.push(format!(
                "{} unknown commands (ids {})",
                self.unknown_commands,
                ids.join(", ")
            ));
        }
        if self.malformed_commands > 0 {
            parts.push(format!("{} malformed commands", self.malformed_commands));
        }
        if self.truncated_bytes > 0 {
            parts.push(format!("{} trailing bytes unparsed", self.truncated_bytes));
        }
        if self.dropped_tabs > 0 {
            parts.push(format!("{} tabs dropped", self.dropped_tabs));
        }
        if self.navigation_fallbacks > 0 {
            parts.push(format!(
                "{} navigation fallbacks",
                self.navigation_fallbacks
            ));
        }
        if self.groups_without_metadata > 0 {
            parts.push(format!(
                "{} groups without metadata",
                self.groups_without_metadata
            ));
        }
        if !self.marker_ok {
            parts.push(format!("{} state markers (expected 1)", self.marker_count));
        }
        parts.join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    /// Chrome's window id. Increments per new window; resets when Chrome restarts.
    pub id: i32,
    /// 1-based position among the windows in this snapshot, by creation order.
    pub number: u32,
    /// `normal`, `popup`, `app`, `devtools`, `app_popup`, or `unknown`.
    pub kind: String,
    pub kind_id: i32,
    /// The tab that was in front, when the log said which.
    pub active_tab: Option<i32>,
    pub tabs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Chrome's 128-bit group token as 32 upper-case hex digits.
    pub id: String,
    /// `None` when no metadata record was found for the token.
    pub title: Option<String>,
    /// One of Chrome's nine names; unknown values become `grey`.
    pub colour: String,
    pub colour_id: Option<u32>,
    pub collapsed: bool,
    /// Identity in Chrome's saved-groups store, when the group is saved.
    pub saved_guid: Option<String>,
    /// Window number the group's tabs live in; groups never span windows.
    pub window: u32,
}

// `tab_id` is the name the brief's data model uses and the reference
// implementation wrote; `id` alone would be ambiguous beside `window_id`.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub tab_id: i32,
    pub window: u32,
    /// Visual index in the window; `-1` when the log never said.
    pub position: i32,
    pub url: String,
    pub title: String,
    pub pinned: bool,
    /// Front tab of its window.
    pub active: bool,
    /// Group id from the `groups` table, when the tab is in a group.
    pub group: Option<String>,
    /// When the tab was last brought to the front, from Chrome's own clock.
    pub last_active: Option<Timestamp>,
    pub window_id: i32,
}

/// The user-visible arrangement, for deciding whether anything changed.
#[derive(Debug, PartialEq, Eq)]
pub struct LayoutEntry<'a> {
    pub window: u32,
    pub position: i32,
    pub url: &'a str,
    pub pinned: bool,
    pub group: Option<&'a str>,
}

impl Snapshot {
    /// Window, position, URL, pinned state and group of every tab, in order.
    /// Titles and timestamps are deliberately excluded: they change without
    /// the user doing anything.
    pub fn layout(&self) -> Vec<LayoutEntry<'_>> {
        self.tabs
            .iter()
            .map(|t| LayoutEntry {
                window: t.window,
                position: t.position,
                url: &t.url,
                pinned: t.pinned,
                group: t.group.as_deref(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradation_summary_is_empty_when_clean() {
        let stats = Stats {
            marker_ok: true,
            ..Stats::default()
        };
        assert!(!stats.is_degraded());
        assert_eq!(stats.degradation_summary(), "");
    }

    #[test]
    fn degradation_summary_names_every_counter() {
        let stats = Stats {
            unknown_commands: 2,
            unknown_command_ids: vec![40, 41],
            malformed_commands: 1,
            truncated_bytes: 17,
            dropped_tabs: 3,
            navigation_fallbacks: 1,
            groups_without_metadata: 1,
            marker_count: 0,
            marker_ok: false,
            ..Stats::default()
        };
        assert!(stats.is_degraded());
        let summary = stats.degradation_summary();
        for expected in [
            "2 unknown commands (ids 40, 41)",
            "1 malformed commands",
            "17 trailing bytes unparsed",
            "3 tabs dropped",
            "1 navigation fallbacks",
            "1 groups without metadata",
            "0 state markers (expected 1)",
        ] {
            assert!(summary.contains(expected), "{summary}");
        }
    }

    #[test]
    fn snapshot_round_trips_and_tolerates_unknown_fields() {
        let json = r#"{
            "schema_version": 1, "id": "2026-01-01-000000Z",
            "captured_at": "2026-01-01T00:00:00Z", "future_field": true,
            "source": {"browser": null, "profile": null, "profile_display": null,
                       "path": "/x", "file": "session.snss", "sha256": "", "bytes": 0,
                       "saved_at": null, "session_started_at": null},
            "stats": {"file_version": 3, "command_table": "session", "commands": 0,
                      "commands_by_id": {"6": 2}, "unknown_commands": 0,
                      "unknown_command_ids": [], "malformed_commands": 0,
                      "truncated_bytes": 0, "marker_count": 1, "marker_ok": true,
                      "windows": 0, "tabs": 0, "groups": 0, "dropped_tabs": 0,
                      "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0,
                      "window_closed": 0}, "navigation_fallbacks": 0,
                      "groups_without_metadata": 0},
            "windows": [], "groups": [],
            "tabs": [{"tab_id": 1, "window": 1, "position": 0, "url": "https://example.test/",
                      "title": "", "pinned": false, "active": true, "group": null,
                      "last_active": null, "window_id": 3, "unknown_tab_field": 1}]
        }"#;
        let snapshot: Snapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.stats.commands_by_id.get(&6), Some(&2));
        assert_eq!(snapshot.layout()[0].url, "https://example.test/");
        let back = serde_json::to_string(&snapshot).unwrap();
        assert!(back.contains("\"schema_version\":1"));
    }
}
