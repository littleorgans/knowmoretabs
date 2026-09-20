//! Derives the compact frontend contract from immutable capture snapshots.
//!
//! slice: library
//! why: One bad snapshot must not hide a whole archive; one repeated URL must
//!      retain every tab sighting without repeating page metadata in the payload.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::archive::{self, Archive, SNAPSHOT_JSON};
use crate::error::Error;
use crate::model::{self, Snapshot};

#[derive(Debug, Default)]
pub struct Loaded {
    pub snapshots: Vec<Snapshot>,
    pub unreadable: Vec<PathBuf>,
}

/// Directory ordering resolves same-time collisions before the stable time sort.
pub fn load(archive: &Archive) -> Result<Loaded, Error> {
    let mut loaded = Loaded::default();
    for id in archive.snapshot_ids()? {
        let path = archive.snapshots_dir().join(&id).join(SNAPSHOT_JSON);
        match archive::read_snapshot(&path) {
            Ok(mut snapshot) if usable(&snapshot) => {
                // The contract's id is the directory name, even if a copied JSON disagrees.
                snapshot.id = id;
                loaded.snapshots.push(snapshot);
            }
            _ => loaded.unreadable.push(path),
        }
    }
    loaded.snapshots.sort_by_key(|s| s.captured_at);
    Ok(loaded)
}

fn usable(snapshot: &Snapshot) -> bool {
    let mut tab_ids = HashSet::new();
    snapshot.schema_version == model::SCHEMA_VERSION
        && snapshot
            .tabs
            .iter()
            .all(|t| t.window > 0 && tab_ids.insert(t.tab_id))
}

#[derive(Debug, Deserialize)]
struct State {
    schema_version: u32,
    forgotten: HashSet<String>,
}

pub fn forgotten(root: &Path) -> Result<HashSet<String>, Error> {
    let path = root.join("library.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => return Err(Error::io("read library state", &path)(err)),
    };
    // Unlike a lost snapshot, ignoring damaged user state could disclose hidden pages.
    let state: State = serde_json::from_slice(&bytes).map_err(|err| Error::LibraryState {
        path: path.clone(),
        reason: err.to_string(),
    })?;
    if state.schema_version != 1 {
        return Err(Error::LibraryState {
            path,
            reason: "unsupported schema_version".to_owned(),
        });
    }
    Ok(state.forgotten)
}

#[derive(Debug, Serialize)]
pub struct Library {
    schema_version: u32,
    generated_at: Timestamp,
    pub stats: Stats,
    snapshots: Vec<LibrarySnapshot>,
    pages: Vec<Page>,
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub pages: usize,
    pub snapshots: usize,
    pub domains: usize,
    pub sightings: usize,
    pub forgotten: usize,
}

#[derive(Debug, Serialize)]
struct Page {
    url: String,
    title: String,
    domain: String,
}

#[derive(Debug, Serialize)]
struct LibrarySnapshot {
    id: String,
    captured_at: Timestamp,
    browser: String,
    profile: String,
    windows: usize,
    tabs_total: usize,
    groups: Vec<Group>,
    tabs: Vec<Tab>,
}

#[derive(Debug, Serialize)]
struct Group {
    id: usize,
    title: String,
    colour: String,
}

// `page_index`, window, position, `tab_id`, pinned (0/1), `group_index_or_null`.
type Tab = (usize, u32, usize, i32, u8, Option<usize>);

pub fn build(snapshots: &[Snapshot], forgotten: &HashSet<String>) -> Library {
    let mut library = Library {
        schema_version: 1,
        generated_at: Timestamp::now(),
        stats: Stats::default(),
        snapshots: Vec::with_capacity(snapshots.len()),
        pages: Vec::new(),
    };
    let mut page_indices = HashMap::new();
    for snapshot in snapshots {
        let groups: Vec<Group> = snapshot
            .groups
            .iter()
            .enumerate()
            .map(|(id, group)| Group {
                id,
                title: group.title.clone().unwrap_or_default(),
                colour: group.colour.clone(),
            })
            .collect();
        let group_indices: HashMap<&str, usize> = snapshot
            .groups
            .iter()
            .enumerate()
            .map(|(i, group)| (group.id.as_str(), i))
            .collect();
        let mut tabs = Vec::new();
        let mut positions = HashMap::<u32, usize>::new();
        let mut ordered_tabs: Vec<_> = snapshot.tabs.iter().collect();
        ordered_tabs.sort_by_key(|tab| (tab.window, tab.position, tab.tab_id));
        for tab in ordered_tabs {
            // Capture may have position -1. Number the best-known layout from zero,
            // before filtering, so even those rows satisfy the frontend contract.
            let position = positions.entry(tab.window).or_default();
            let current_position = *position;
            *position += 1;
            let Some(domain) = public_domain(&tab.url) else {
                continue;
            };
            if forgotten.contains(&tab.url) {
                continue;
            }
            let index = *page_indices.entry(tab.url.as_str()).or_insert_with(|| {
                let index = library.pages.len();
                library.pages.push(Page {
                    url: tab.url.clone(),
                    title: String::new(),
                    domain,
                });
                index
            });
            // Ascending snapshots make the most recent non-empty title win.
            if !tab.title.is_empty() {
                library.pages[index].title.clone_from(&tab.title);
            }
            let group = tab
                .group
                .as_deref()
                .and_then(|id| group_indices.get(id))
                .copied();
            tabs.push((
                index,
                tab.window,
                current_position,
                tab.tab_id,
                u8::from(tab.pinned),
                group,
            ));
        }
        library.stats.sightings += tabs.len();
        library.snapshots.push(LibrarySnapshot {
            id: snapshot.id.clone(),
            captured_at: snapshot.captured_at,
            browser: snapshot.source.browser.clone().unwrap_or_default(),
            profile: snapshot.source.profile.clone().unwrap_or_default(),
            windows: snapshot.windows.len(),
            tabs_total: tabs.len(),
            groups,
            tabs,
        });
    }
    library.stats.pages = library.pages.len();
    library.stats.snapshots = library.snapshots.len();
    library.stats.domains = library
        .pages
        .iter()
        .map(|p| &p.domain)
        .collect::<HashSet<_>>()
        .len();
    library.stats.forgotten = forgotten.len();
    library
}

fn public_domain(raw: &str) -> Option<String> {
    let Ok(url) = Url::parse(raw) else {
        return Some(String::new());
    };
    let host = url.host_str().unwrap_or("").to_lowercase();
    if url.scheme() == "file" || matches!(host.trim_end_matches('.'), "localhost" | "127.0.0.1") {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(&host).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_use_url_authority_and_keep_nonlocal_urls() {
        for raw in [
            "file:///tmp/a",
            "HTTP://LOCALHOST:8080/a",
            "http://127.0.0.1/a",
            "http://localhost./",
        ] {
            assert_eq!(public_domain(raw), None);
        }
        for (raw, domain) in [
            ("https://user:pass@WWW.Example.test:8443/a", "example.test"),
            ("http://localhost.example.test/a", "localhost.example.test"),
            ("https://example.test/localhost", "example.test"),
            ("http://[2001:db8::1]:80/", "[2001:db8::1]"),
            ("about:blank", ""),
            ("not a URL", ""),
        ] {
            assert_eq!(public_domain(raw).as_deref(), Some(domain));
        }
    }
}
