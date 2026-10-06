//! GitHub addresses: which repository, issue, pull request or discussion a
//! page is; and a public repository's topics and README, from the data its
//! own page embeds for the browser.
//!
//! slice: enrich, content
//! why: A repository's `<head>` says little more than its name, while its
//!      topics and README say what it is about. Both are in the JSON the
//!      repository page carries for its scripts, so the one cookieless fetch
//!      enrich already makes gets them with no token and no API quota.
//!      Content reads the same documents through the GitHub API, so both
//!      commands judge an address by the one parse here: GitHub's own pages
//!      are nobody's repository, and a name GitHub would not accept is never
//!      put into an API path.

use serde_json::Value;
use url::Url;

use crate::head;
use crate::metadata_writer::Github;

/// How much README text is kept: enough to say what a project is.
pub const README_CHARS: usize = 4000;

/// First path segments that are GitHub's own pages, not owners.
const NOT_OWNERS: &[&str] = &[
    "about",
    "apps",
    "codespaces",
    "collections",
    "copilot",
    "customer-stories",
    "dashboard",
    "enterprise",
    "events",
    "explore",
    "features",
    "issues",
    "login",
    "marketplace",
    "new",
    "notifications",
    "orgs",
    "organizations",
    "pricing",
    "pulls",
    "readme",
    "resources",
    "search",
    "security",
    "settings",
    "signup",
    "site",
    "solutions",
    "sponsors",
    "stars",
    "team",
    "topics",
    "trending",
    "users",
];

/// A repository, by the names its address spells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    /// `owner/name`, as GitHub spells it in a title.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// What a GitHub page is a page of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `github.com/OWNER/REPO`.
    Repo(Repo),
    /// `github.com/OWNER/REPO/issues/N`.
    Issue(Repo, u32),
    /// `github.com/OWNER/REPO/pull/N`, and its tabs (`/files`, `/commits`).
    Pull(Repo, u32),
    /// `github.com/OWNER/REPO/discussions/N`.
    Discussion(Repo, u32),
}

impl Target {
    /// What `url` is a page of; `None` for any other page on GitHub (a
    /// file, a profile, the settings) and for any other site.
    pub fn of(url: &Url) -> Option<Self> {
        let host = url.host_str().unwrap_or("");
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        let segments: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let (owner, name, rest) = match segments.as_slice() {
            [owner, name, rest @ ..] => (*owner, *name, rest),
            _ => return None,
        };
        if NOT_OWNERS.contains(&owner.to_ascii_lowercase().as_str())
            || !is_owner_name(owner)
            || !is_repo_name(name)
        {
            return None;
        }
        let repo = Repo {
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        let number = |n: &str| {
            n.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| n.parse::<u32>().ok())
                .flatten()
                .filter(|n| (1..=i32::MAX.unsigned_abs()).contains(n))
        };
        Some(match rest {
            [] => Self::Repo(repo),
            ["issues", n] => Self::Issue(repo, number(n)?),
            ["pull", n, ..] => Self::Pull(repo, number(n)?),
            ["discussions", n] => Self::Discussion(repo, number(n)?),
            _ => return None,
        })
    }

    pub fn repo(&self) -> &Repo {
        match self {
            Self::Repo(repo)
            | Self::Issue(repo, _)
            | Self::Pull(repo, _)
            | Self::Discussion(repo, _) => repo,
        }
    }
}

/// A user or organisation name: letters, digits, `-`, and `_` for the
/// accounts an enterprise manages.
fn is_owner_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A repository name: letters, digits, `-`, `_` and `.`, but not a path
/// step of dots alone.
fn is_repo_name(name: &str) -> bool {
    !name.bytes().all(|b| b == b'.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Whether `url` is a repository's own page, `github.com/OWNER/REPO`. Pages
/// inside a repository (an issue, a file) are enriched from their own head
/// only: fetching the repository page as well would be a second request.
pub fn is_repo_page(url: &Url) -> bool {
    matches!(Target::of(url), Some(Target::Repo(_)))
}

/// Topics and README text, if the page carries either.
pub fn repo_data(page: &str) -> Option<Github> {
    let topics: Vec<String> = json_after(page, "\"topics\":[", 1)
        .as_ref()
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").unwrap_or(item).as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let readme = json_after(page, "\"richText\":\"", 1)
        .as_ref()
        .and_then(Value::as_str)
        .map(head::text_of)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(README_CHARS).collect::<String>());
    (!topics.is_empty() || readme.is_some()).then_some(Github { topics, readme })
}

/// The JSON value that starts `back` bytes before the end of the first
/// `key` in `page`: the key includes the value's opening `[` or `"`.
fn json_after(page: &str, key: &str, back: usize) -> Option<Value> {
    let start = page.find(key)? + key.len() - back;
    serde_json::Deserializer::from_str(&page[start..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_repository_root_is_a_repository_page() {
        for (raw, repo) in [
            ("https://github.com/owner/repo", true),
            ("https://github.com/owner/repo/", true),
            (
                "https://www.GitHub.com/owner/repo?tab=readme-ov-file#install",
                true,
            ),
            ("https://github.com/owner/repo/issues/4", false),
            ("https://github.com/owner", false),
            ("https://github.com/topics/rust", false),
            ("https://github.com/settings/profile", false),
            ("https://github.com/Orgs/x", false),
            ("https://gist.github.com/owner/abc", false),
            ("https://example.test/owner/repo", false),
            ("https://github.com/owner/..", false),
            ("https://github.com/own%2Fer/repo", false),
        ] {
            assert_eq!(is_repo_page(&Url::parse(raw).unwrap()), repo, "{raw}");
        }
    }

    #[test]
    fn issues_pulls_and_discussions_are_targets_and_other_pages_are_not() {
        let target = |raw: &str| Target::of(&Url::parse(raw).unwrap());
        let repo = Repo {
            owner: "some-owner".to_owned(),
            name: "repo.rs".to_owned(),
        };
        assert_eq!(
            target("https://github.com/some-owner/repo.rs/issues/12#issuecomment-1"),
            Some(Target::Issue(repo.clone(), 12))
        );
        assert_eq!(
            target("https://github.com/some-owner/repo.rs/pull/7/files"),
            Some(Target::Pull(repo.clone(), 7))
        );
        assert_eq!(
            target("https://www.github.com/some-owner/repo.rs/discussions/3?sort=new"),
            Some(Target::Discussion(repo.clone(), 3))
        );
        assert_eq!(repo.full_name(), "some-owner/repo.rs");
        for raw in [
            "https://github.com/owner/repo/blob/main/README.md",
            "https://github.com/owner/repo/tree/main",
            "https://github.com/owner/repo/issues",
            "https://github.com/owner/repo/issues/new",
            "https://github.com/owner/repo/issues/0",
            "https://github.com/owner/repo/issues/99999999999",
            "https://github.com/owner/repo/pull/x1",
            "https://github.com/owner/repo/discussions/categories",
            "https://github.com/settings/profile",
            "https://github.com/notifications/beta",
            "https://github.com/",
        ] {
            assert_eq!(target(raw), None, "{raw}");
        }
    }

    #[test]
    fn topics_and_readme_come_from_the_embedded_data() {
        let page = concat!(
            r#"<html><body><script type="application/json">{"payload":{"#,
            r#""topics":[{"name":"rust"},{"name":"cli"}],"#,
            r#""overview":{"richText":"<article><h1>Tool<\/h1><p>Keeps \"tabs\" &amp; more.<\/p>"#,
            r#"<pre>code<\/pre><\/article>"}}}</script></body></html>"#,
        );
        let data = repo_data(page).unwrap();
        assert_eq!(data.topics, ["rust", "cli"]);
        assert_eq!(data.readme.as_deref(), Some("Tool\nKeeps \"tabs\" & more."));
    }

    #[test]
    fn a_long_readme_is_cut_at_a_character_count_and_nothing_found_is_none() {
        let long = "é".repeat(README_CHARS + 10);
        let page = format!(r#"{{"richText":"<p>{long}</p>","topics":[]}}"#);
        let data = repo_data(&page).unwrap();
        assert_eq!(data.readme.unwrap().chars().count(), README_CHARS);
        assert_eq!(data.topics, Vec::<String>::new());
        assert_eq!(repo_data("<html><title>x</title></html>"), None);
        assert_eq!(repo_data(r#""topics":[not json"#), None);
    }
}
