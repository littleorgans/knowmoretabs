//! The library pages a network command may touch, in the order it should
//! touch them, and why each of the rest stays home.
//!
//! slice: enrich, content
//! why: The decision of what never leaves the machine is made once, before
//!      any request, and can be read in full with `--dry-run`: forgotten
//!      pages, the private network, search results and URLs that carry a
//!      token stay home, and sign-in screens are recorded without a request.
//!      Every network command plans from these same rules, so a page one of
//!      them refuses is refused by all of them for the same stated reason.

use std::collections::{BTreeMap, HashMap, HashSet};

use url::Url;

use crate::guard::{self, carries_token, is_search_results};
use crate::library::{self, State};
use crate::model::Snapshot;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub dry_run: bool,
    pub limit: Option<usize>,
    pub refetch: bool,
}

/// Why a library page is not fetched at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotWeb,
    Forgotten,
    PrivateNetwork,
    SearchResults,
    Token,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotWeb => "not a web page",
            Self::Forgotten => "forgotten",
            Self::PrivateNetwork => "private network",
            Self::SearchResults => "search results",
            Self::Token => "token in URL",
        }
    }
}

/// Why a page is on the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    New,
    Refetch,
    /// A sign-in, sign-up or verification screen: recorded, never fetched.
    Login,
}

impl Why {
    pub fn label(&self) -> String {
        match self {
            Self::New => "new".to_owned(),
            Self::Refetch => "refetch".to_owned(),
            Self::Login => "login page: recorded as behind_login, not fetched".to_owned(),
        }
    }
}

#[derive(Debug)]
pub struct Item {
    pub url: String,
    pub host: String,
    pub why: Why,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub todo: Vec<Item>,
    /// On the list but past `--limit`.
    pub more: usize,
    pub not_fetched: Vec<(String, Skip)>,
    pub already_fetched: usize,
}

impl Plan {
    pub fn sites(&self) -> usize {
        self.todo
            .iter()
            .filter(|item| item.why != Why::Login)
            .map(|item| item.host.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    /// A lower bound: the busiest host's pages, one a second.
    pub fn seconds_at_least(&self) -> usize {
        let mut per_host: HashMap<&str, usize> = HashMap::new();
        for item in self.todo.iter().filter(|item| item.why != Why::Login) {
            *per_host.entry(&item.host).or_default() += 1;
        }
        per_host.values().max().map_or(0, |n| n.saturating_sub(1))
    }

    pub fn skip_counts(&self) -> BTreeMap<Skip, usize> {
        let mut counts = BTreeMap::new();
        for (_, skip) in &self.not_fetched {
            *counts.entry(*skip).or_default() += 1;
        }
        counts
    }
}

/// Library pages newest sighting first, excluding prior attempts unless
/// the owner explicitly asks to fetch them again. `attempted` says whether
/// the command already recorded an attempt for a URL.
pub fn plan(
    snapshots: &[Snapshot],
    state: &State,
    attempted: impl Fn(&str) -> bool,
    options: Options,
) -> Plan {
    let library = library::known_urls(snapshots);
    let mut seen = HashSet::new();
    let mut plan = Plan::default();
    let tabs = snapshots.iter().rev().flat_map(|s| s.tabs.iter());
    for tab in tabs {
        let raw = tab.url.as_str();
        if !library.contains(raw) || !seen.insert(raw) {
            continue;
        }
        let parsed = Url::parse(raw).ok().filter(guard::is_web);
        let skip = match &parsed {
            None => Some(Skip::NotWeb),
            Some(_) if state.forgotten.contains(raw) => Some(Skip::Forgotten),
            Some(url) => skip_reason(url),
        };
        if let Some(skip) = skip {
            plan.not_fetched.push((raw.to_owned(), skip));
            continue;
        }
        let Some(url) = parsed else { continue };
        let attempted = attempted(raw);
        if attempted && !options.refetch {
            plan.already_fetched += 1;
            continue;
        }
        let why = if guard::is_login_page(&url) {
            Why::Login
        } else if attempted {
            Why::Refetch
        } else {
            Why::New
        };
        let item = Item {
            url: raw.to_owned(),
            host: url.host_str().unwrap_or("").to_ascii_lowercase(),
            why,
        };
        plan.todo.push(item);
    }
    if let Some(limit) = options.limit {
        plan.more = plan.todo.len().saturating_sub(limit);
        plan.todo.truncate(limit);
    }
    plan
}

/// The rules for a web page on a public-looking host.
fn skip_reason(url: &Url) -> Option<Skip> {
    if guard::is_private_host(url) {
        Some(Skip::PrivateNetwork)
    } else if is_search_results(url) {
        Some(Skip::SearchResults)
    } else if carries_token(url) {
        Some(Skip::Token)
    } else {
        None
    }
}
