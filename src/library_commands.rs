//! List/export orchestration and terminal presentation.
//!
//! slice: library
//! why: Degradation belongs in command reports, keeping the frontend contract
//!      stable and the data builder independent of terminal output.

use std::path::Path;

use serde::Serialize;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::{export, library, library_history, out, suggestions};

pub fn list(root: &Path, json: bool, log: Log) -> Result<(), Error> {
    let loaded = library::load(&Archive::at(root))?;
    report_skipped(&loaded, log);
    let snapshots: Vec<_> = loaded
        .snapshots
        .iter()
        .rev()
        .map(|snapshot| {
            serde_json::json!({
                "id": snapshot.id, "captured_at": snapshot.captured_at,
                "browser": snapshot.source.browser, "profile": snapshot.source.profile,
                "tabs": snapshot.tabs.len(), "windows": snapshot.windows.len(),
                "groups": snapshot.groups.len(), "degraded": snapshot.stats.is_degraded(),
            })
        })
        .collect();
    if json {
        out::json(&serde_json::json!({
            "snapshots": snapshots, "skipped_snapshots": loaded.unreadable.len(),
        }));
    } else if !log.quiet {
        if loaded.snapshots.is_empty() {
            out::line("No snapshots found.");
        }
        for snapshot in loaded.snapshots.iter().rev() {
            out::line(&format!(
                "{}  {}  {} / {}  {} tabs across {} windows, {} groups{}",
                snapshot.id,
                snapshot.captured_at,
                snapshot
                    .source
                    .browser
                    .as_deref()
                    .unwrap_or("unknown browser"),
                snapshot
                    .source
                    .profile
                    .as_deref()
                    .unwrap_or("unknown profile"),
                snapshot.tabs.len(),
                snapshot.windows.len(),
                snapshot.groups.len(),
                if snapshot.stats.is_degraded() {
                    " (degraded)"
                } else {
                    ""
                }
            ));
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct Exported<'a> {
    path: &'a Path,
    stats: &'a library::Stats,
    skipped_snapshots: usize,
}

pub fn export(
    root: &Path,
    destination: Option<&Path>,
    with_history: bool,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let loaded = library::load(&archive)?;
    report_skipped(&loaded, log);
    let state = library::State::read(root)?;
    let suggested = suggestions::by_page(&suggestions::read(root)?, &state);
    let recorded = library_history::for_library(root, log);
    let library = library::build(
        &loaded.snapshots,
        &recorded,
        &state,
        &suggested,
        library::Shape::Export { with_history },
    );
    let default_destination = root.join("export");
    let destination = destination.unwrap_or(&default_destination);
    export::write(root, destination, &library)?;
    if json {
        out::json(&serde_json::json!({ "exported": Exported {
            path: destination, stats: &library.stats, skipped_snapshots: loaded.unreadable.len(),
        }}));
    } else if !log.quiet {
        out::line(&format!(
            "exported {} pages across {} snapshots ({} sightings, {} forgotten) to {}",
            library.stats.pages,
            library.stats.snapshots,
            library.stats.sightings,
            library.stats.forgotten,
            destination.display()
        ));
    }
    Ok(())
}

fn report_skipped(loaded: &library::Loaded, log: Log) {
    if !loaded.unreadable.is_empty() {
        log.warn(&format!(
            "skipped {} unreadable or invalid snapshot.json files",
            loaded.unreadable.len()
        ));
        for path in &loaded.unreadable {
            log.note(&format!("skipped {}", path.display()));
        }
    }
}
