//! Derives the compact frontend contract from immutable capture snapshots.
//!
//! slice: library
//! why: One bad snapshot must not hide a whole archive; one repeated URL must
//!      retain every tab sighting without repeating page metadata in the payload.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::archive::{self, Archive, SNAPSHOT_JSON};
use crate::error::Error;
use crate::local;
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

pub const STATE_FILE: &str = "library.json";
const STATE_SCHEMA_VERSION: u32 = 1;

/// The user's own state, beside the snapshots and never inside them: the
/// forgotten set, the tags set on pages and the vocabulary they come from.
/// Unknown fields are carried through a rewrite so a newer build's additions
/// survive an older build's `forget`, which is also why tags did not need a
/// new `schema_version`: an older build keeps them as it keeps any field.
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    schema_version: u32,
    pub forgotten: BTreeSet<String>,
    /// Absent reads as untagged, and empty is not written, so a library that
    /// was never tagged keeps exactly the file an older build wrote.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, PageTags>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vocabulary: BTreeMap<String, Term>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// What the owner decided about one page. Kept as two lists from the start
/// so that suggested tags can arrive later without a migration: until then a
/// page's tags are its `add` set. The lists never share a name.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PageTags {
    #[serde(default)]
    pub add: BTreeSet<String>,
    #[serde(default)]
    pub remove: BTreeSet<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl PageTags {
    /// Nothing worth keeping a map entry for.
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty() && self.extra.is_empty()
    }
}

/// A vocabulary entry. Retiring records the time rather than deleting the
/// entry, so a page's history stays readable and the name can come back.
#[derive(Debug, Serialize, Deserialize)]
pub struct Term {
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<Timestamp>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Term {
    pub fn new(created_at: Timestamp) -> Self {
        Self {
            created_at,
            retired_at: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// A tag as the page and the API name it: active entries only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VocabularyEntry {
    pub name: String,
    pub created_at: Timestamp,
}

/// Tag names compare without regard to case, so "mcp" finds "MCP".
pub fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// The one order tags are listed in, everywhere: alphabetical ignoring case,
/// and the exact spelling only to break a tie.
pub fn tag_order(a: &str, b: &str) -> std::cmp::Ordering {
    fold(a).cmp(&fold(b)).then_with(|| a.cmp(b))
}

impl State {
    pub fn read(root: &Path) -> Result<Self, Error> {
        let path = root.join(STATE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    schema_version: STATE_SCHEMA_VERSION,
                    forgotten: BTreeSet::new(),
                    tags: BTreeMap::new(),
                    vocabulary: BTreeMap::new(),
                    extra: serde_json::Map::new(),
                });
            }
            Err(err) => return Err(Error::io("read library state", &path)(err)),
        };
        // Unlike a lost snapshot, ignoring damaged user state could disclose hidden pages.
        let state: Self = serde_json::from_slice(&bytes).map_err(|err| Error::LibraryState {
            path: path.clone(),
            reason: err.to_string(),
        })?;
        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(Error::LibraryState {
                path,
                reason: "unsupported schema_version".to_owned(),
            });
        }
        Ok(state)
    }

    /// Caller holds the archive lock across the read that preceded this.
    pub fn write(&self, root: &Path) -> Result<(), Error> {
        let path = root.join(STATE_FILE);
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        bytes.push(b'\n');
        archive::replace_file(&path, &bytes)
    }

    /// The vocabulary's own spelling of `name`, retired entries included.
    pub fn term(&self, name: &str) -> Option<(&str, &Term)> {
        let folded = fold(name);
        self.vocabulary
            .iter()
            .find(|(known, _)| fold(known) == folded)
            .map(|(known, term)| (known.as_str(), term))
    }

    /// Active entries, in tag order.
    pub fn active_vocabulary(&self) -> Vec<VocabularyEntry> {
        let mut entries: Vec<_> = self
            .vocabulary
            .iter()
            .filter(|(_, term)| term.retired_at.is_none())
            .map(|(name, term)| VocabularyEntry {
                name: name.clone(),
                created_at: term.created_at,
            })
            .collect();
        entries.sort_by(|a, b| tag_order(&a.name, &b.name));
        entries
    }

    /// Folded name to vocabulary spelling, for looking up many pages at once.
    /// With `retired`, retired entries resolve too.
    pub fn spellings(&self, retired: bool) -> HashMap<String, &str> {
        self.vocabulary
            .iter()
            .filter(|(_, term)| retired || term.retired_at.is_none())
            .map(|(name, _)| (fold(name), name.as_str()))
            .collect()
    }

    /// A page's tags as the library shows them: what the owner added, in the
    /// vocabulary's spelling and in tag order. A name `spellings` does not
    /// resolve (retired, or never in the vocabulary) is not shown.
    pub fn page_tags(&self, url: &str, spellings: &HashMap<String, &str>) -> Vec<String> {
        let Some(page) = self.tags.get(url) else {
            return Vec::new();
        };
        let mut tags: Vec<String> = page
            .add
            .iter()
            .filter_map(|name| spellings.get(&fold(name)))
            .map(|name| (*name).to_owned())
            .collect();
        tags.sort_by(|a, b| tag_order(a, b));
        tags.dedup();
        tags
    }
}

/// Every URL the library would list: what `forget` may name. The same rule
/// as `build`, so "not in your library" and "not shown" cannot disagree.
pub fn known_urls(snapshots: &[Snapshot]) -> HashSet<&str> {
    snapshots
        .iter()
        .flat_map(|snapshot| snapshot.tabs.iter())
        .filter(|tab| public_domain(&tab.url).is_some())
        .map(|tab| tab.url.as_str())
        .collect()
}

/// Whether forgotten pages are left out (an export is read-only, so a hidden
/// page has no way back) or kept and flagged (serve has a Restore button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Export,
    Serve,
}

#[derive(Debug, Serialize)]
pub struct Library {
    schema_version: u32,
    generated_at: Timestamp,
    pub stats: Stats,
    snapshots: Vec<LibrarySnapshot>,
    pages: Vec<Page>,
    /// Active tags only; per-tag counts are the page's to compute.
    vocabulary: Vec<VocabularyEntry>,
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
    /// Absent means false, so the export shape is unchanged.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    forgotten: bool,
    /// Always present, empty when untagged, so every page has the same shape.
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct LibrarySnapshot {
    id: String,
    captured_at: Timestamp,
    browser: String,
    profile: String,
    windows: usize,
    tabs_total: usize,
    stats: SnapshotStats,
    groups: Vec<Group>,
    tabs: Vec<Tab>,
}

/// The counters the page can name, and only those: `model::Stats` also carries
/// per-command-id bookkeeping that would cost every snapshot in the payload
/// bytes no reader ever renders.
#[derive(Debug, Serialize)]
struct SnapshotStats {
    dropped_tabs: u64,
    unknown_commands: u64,
    malformed_commands: u64,
    truncated_bytes: u64,
    marker_ok: bool,
    /// Degradation the named counters do not cover still has to show.
    degraded: bool,
}

impl From<&model::Stats> for SnapshotStats {
    fn from(stats: &model::Stats) -> Self {
        Self {
            dropped_tabs: stats.dropped_tabs,
            unknown_commands: stats.unknown_commands,
            malformed_commands: stats.malformed_commands,
            truncated_bytes: stats.truncated_bytes,
            marker_ok: stats.marker_ok,
            degraded: stats.is_degraded(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Group {
    id: usize,
    title: String,
    colour: String,
    collapsed: bool,
}

// `page_index`, window, position, `tab_id`, pinned (0/1), `group_index_or_null`.
type Tab = (usize, u32, usize, i32, u8, Option<usize>);

pub fn build(snapshots: &[Snapshot], state: &State, shape: Shape) -> Library {
    let forgotten = &state.forgotten;
    let spellings = state.spellings(false);
    let mut library = Library {
        schema_version: 1,
        generated_at: Timestamp::now(),
        stats: Stats::default(),
        snapshots: Vec::with_capacity(snapshots.len()),
        pages: Vec::new(),
        vocabulary: state.active_vocabulary(),
    };
    let mut page_indices = HashMap::new();
    // Which forgotten URLs the archive actually holds; a state file may name
    // URLs no surviving snapshot mentions, and those are not pages.
    let mut forgotten_pages: HashSet<&str> = HashSet::new();
    for snapshot in snapshots {
        let groups: Vec<Group> = snapshot
            .groups
            .iter()
            .enumerate()
            .map(|(id, group)| Group {
                id,
                title: group.title.clone().unwrap_or_default(),
                colour: group.colour.clone(),
                collapsed: group.collapsed,
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
        // Capture writes -1 when the log never said where a tab sat. Those
        // sort after the tabs whose place is known, so that renumbering does
        // not push every real position along by one.
        ordered_tabs.sort_by_key(|tab| (tab.window, tab.position < 0, tab.position, tab.tab_id));
        for tab in ordered_tabs {
            // Number the best-known layout from zero, before filtering, so even
            // the unplaced rows satisfy the frontend contract.
            let position = positions.entry(tab.window).or_default();
            let current_position = *position;
            *position += 1;
            let Some(domain) = public_domain(&tab.url) else {
                continue;
            };
            let is_forgotten = forgotten.contains(&tab.url);
            if is_forgotten {
                forgotten_pages.insert(tab.url.as_str());
                if shape == Shape::Export {
                    continue;
                }
            }
            let index = *page_indices.entry(tab.url.as_str()).or_insert_with(|| {
                let index = library.pages.len();
                library.pages.push(Page {
                    url: tab.url.clone(),
                    title: String::new(),
                    domain,
                    forgotten: is_forgotten,
                    tags: state.page_tags(&tab.url, &spellings),
                });
                index
            });
            // Ascending snapshots make the most recent non-empty title win.
            // Whitespace counts as empty: a blank title renders as a nameless
            // row, which is worse than the older title it would replace.
            if !tab.title.trim().is_empty() {
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
            // Always emitted, zeroed when the parse was clean: a snapshot that
            // lost tabs must not report a low count and say nothing.
            stats: (&snapshot.stats).into(),
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
    library.stats.forgotten = forgotten_pages.len();
    library
}

fn public_domain(raw: &str) -> Option<String> {
    let Ok(url) = Url::parse(raw) else {
        return Some(String::new());
    };
    if url.scheme() == "file" || url.host().as_ref().is_some_and(local::is_this_machine) {
        return None;
    }
    let host = url.host_str().unwrap_or("").to_lowercase();
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
            // The same machine, spelled differently.
            "http://[::1]:8080/a",
            "http://127.0.0.2/a",
            "http://0.0.0.0:3000/a",
            "http://app.localhost/a",
        ] {
            assert_eq!(public_domain(raw), None, "{raw}");
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
