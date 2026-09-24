//! A public GitHub repository's topics and README, from the data its own page
//! embeds for the browser.
//!
//! slice: library
//! why: A repository's `<head>` says little more than its name, while its
//!      topics and README say what it is about. Both are in the JSON the
//!      repository page carries for its scripts, so the one cookieless fetch
//!      enrich already makes gets them with no token and no API quota.

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

/// Whether `url` is a repository's own page, `github.com/OWNER/REPO`. Pages
/// inside a repository (an issue, a file) are enriched from their own head
/// only: fetching the repository page as well would be a second request.
pub fn is_repo_page(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("");
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return false;
    }
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    matches!(segments.as_slice(), [owner, _repo]
        if !NOT_OWNERS.contains(&owner.to_ascii_lowercase().as_str()))
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
        ] {
            assert_eq!(is_repo_page(&Url::parse(raw).unwrap()), repo, "{raw}");
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
        assert!(data.topics.is_empty());
        assert_eq!(repo_data("<html><title>x</title></html>"), None);
        assert_eq!(repo_data(r#""topics":[not json"#), None);
    }
}
