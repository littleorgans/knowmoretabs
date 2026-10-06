//! Synthetic fixtures shared by content command and planner tests.
//!
//! slice: content
//! why: Both test the same library and attempt shapes, so their fixtures
//!      have one owner independent of either module's private tests.

use crate::content_plan::Work;
use crate::content_store::{self, Line, Status};
use crate::github_api::Readiness;
use crate::library::State;
use crate::model::Snapshot;
use crate::ytdlp;

pub(crate) fn snapshot(urls: &[&str]) -> Snapshot {
    let tabs: Vec<serde_json::Value> = urls
        .iter()
        .enumerate()
        .map(|(i, url)| {
            serde_json::json!({"tab_id": i, "window": 1, "position": i, "url": url,
                "title": "", "pinned": false, "active": false, "group": null,
                "last_active": null, "window_id": 1})
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "schema_version": 1, "id": "2026-10-07-090000Z", "captured_at": "2026-10-07T09:00:00Z",
        "source": {"browser": null, "profile": null, "profile_display": null, "path": "/x",
            "file": "session.snss", "sha256": "", "bytes": 0, "saved_at": null,
            "session_started_at": null},
        "stats": {"file_version": 3, "command_table": "session", "commands": 0,
            "commands_by_id": {}, "unknown_commands": 0, "unknown_command_ids": [],
            "malformed_commands": 0, "truncated_bytes": 0, "marker_count": 1, "marker_ok": true,
            "windows": 0, "tabs": 0, "groups": 0, "dropped_tabs": 0,
            "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0, "window_closed": 0},
            "navigation_fallbacks": 0, "groups_without_metadata": 0},
        "windows": [], "groups": [], "tabs": tabs,
    }))
    .unwrap()
}

pub(crate) fn state(forgotten: &[&str]) -> State {
    serde_json::from_value(serde_json::json!({"schema_version": 1, "forgotten": forgotten}))
        .unwrap()
}

pub(crate) fn log(lines: &[(&str, Status, u32)]) -> content_store::Log {
    let mut log = content_store::Log::default();
    for (url, status, attempt) in lines {
        let mut line = Line::new(url, *status);
        line.attempt = *attempt;
        log.pages.insert((*url).to_owned(), line);
    }
    log
}

pub(crate) fn no_gh() -> Readiness {
    Readiness::Missing
}

pub(crate) fn no_ytdlp() -> ytdlp::Readiness {
    ytdlp::Readiness::Missing
}

pub(crate) fn unsent(work: &Work) -> Vec<(&str, Status, &str)> {
    work.unsent
        .iter()
        .map(|line| {
            (
                line.url.as_str(),
                line.status,
                line.reason.as_deref().unwrap_or(""),
            )
        })
        .collect()
}
