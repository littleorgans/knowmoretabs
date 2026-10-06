//! Which route reads a library page, judged from its address alone.
//!
//! slice: content
//! why: Each kind of document has a cheapest way to its text: an X post
//!      through the X post API, a GitHub repository, issue, pull request or
//!      discussion through `gh api`, anything else from its own site.
//!      Choosing it from the address, before any request, lets the plan say
//!      which host every request goes to, fetch one document once whatever
//!      address it was opened at, and keep profiles, channels and playlists
//!      home as not documents. A new route is a variant here, and `content`
//!      itself does not change.

use url::Url;

use crate::content_fetch::{self, Capture};
use crate::fetch::Fetcher;
use crate::github::Target;
use crate::github_api::{self, Gh};
use crate::guard;
use crate::xpost;

/// How a document is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// From its own site, as a web page.
    Web,
    /// From the X post API, by the post's number.
    XPost(String),
    /// From the GitHub API through `gh`.
    Github(Target),
}

impl Route {
    /// How a page is read, judged from its address; `None` for pages on X
    /// and `YouTube` that list documents rather than being one: profiles,
    /// timelines, channels and playlists.
    pub fn of(url: &Url) -> Option<Self> {
        let host = guard::host_key(url);
        let host = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("mobile."))
            .or_else(|| host.strip_prefix("m."))
            .unwrap_or(&host);
        match host {
            "x.com" | "twitter.com" => xpost::post_id(url).map(Self::XPost),
            "github.com" => Some(Target::of(url).map_or(Self::Web, Self::Github)),
            "youtube.com" => {
                let first = url
                    .path_segments()
                    .into_iter()
                    .flatten()
                    .find(|segment| !segment.is_empty());
                match first {
                    Some(first)
                        if first.starts_with('@')
                            || ["channel", "c", "user", "playlist"].contains(&first) =>
                    {
                        None
                    }
                    _ => Some(Self::Web),
                }
            }
            _ => Some(Self::Web),
        }
    }

    /// The host every request goes to, when it is not the page's own.
    pub fn host(&self) -> Option<&'static str> {
        match self {
            Self::Web => None,
            Self::XPost(_) => Some(xpost::API_HOST),
            Self::Github(_) => Some(github_api::API_HOST),
        }
    }

    /// How many of its host's requests may run at once: one for a host
    /// paced a request a second, more for a tool with limits of its own.
    pub fn lanes(&self) -> usize {
        match self {
            Self::Web | Self::XPost(_) => 1,
            Self::Github(_) => github_api::CONCURRENT,
        }
    }

    /// What one fetch stands for: page `raw` without its fragment, the
    /// post, or the repository or numbered thread, whatever address it was
    /// opened at. Issues, pull requests and discussions share one number
    /// space per repository, and GitHub names ignore case.
    pub fn key(&self, raw: &str) -> String {
        match self {
            Self::Web => without_fragment(raw),
            Self::XPost(id) => xpost::api_url(id),
            Self::Github(target) => {
                let repo = target.repo().full_name().to_ascii_lowercase();
                match target {
                    Target::Repo(_) => format!("github:{repo}"),
                    Target::Issue(_, n) | Target::Pull(_, n) | Target::Discussion(_, n) => {
                        format!("github:{repo}#{n}")
                    }
                }
            }
        }
    }

    /// Reads page `raw`. A GitHub page without a usable `gh` is read from
    /// the web, as the plan routes it.
    pub fn capture(&self, fetcher: &Fetcher, gh: Option<&Gh>, raw: &str) -> Capture {
        match (self, gh) {
            (Self::XPost(id), _) => xpost::capture(fetcher, raw, id),
            (Self::Github(target), Some(gh)) => github_api::capture(gh, raw, target),
            (Self::Web | Self::Github(_), _) => content_fetch::capture(fetcher, raw),
        }
    }
}

fn without_fragment(raw: &str) -> String {
    raw.split_once('#').map_or(raw, |(page, _)| page).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(raw: &str) -> Option<Route> {
        Route::of(&Url::parse(raw).unwrap())
    }

    #[test]
    fn x_posts_route_to_the_api_and_everything_else_to_the_web() {
        for raw in [
            "https://x.com/someone/status/1234567890",
            "https://twitter.com/someone/status/1234567890/photo/1",
            "https://mobile.twitter.com/someone/status/1234567890?s=20",
            "https://www.x.com/someone/status/1234567890#reply",
        ] {
            assert_eq!(
                route(raw),
                Some(Route::XPost("1234567890".to_owned())),
                "{raw}"
            );
        }
        for raw in [
            "https://example.test/someone/status/1234567890",
            "https://notx.com/someone/status/1234567890",
        ] {
            assert_eq!(route(raw), Some(Route::Web), "{raw}");
        }
    }

    #[test]
    fn profiles_channels_and_playlists_are_not_documents() {
        for raw in [
            "https://x.com/someone",
            "https://x.com/home",
            "https://twitter.com/someone/lists",
            "https://mobile.twitter.com/someone/status/",
            "https://x.com/someone/status/not-a-number",
            "https://www.youtube.com/@someone",
            "https://www.youtube.com/channel/UC000",
            "https://www.youtube.com/c/Someone",
            "https://www.youtube.com/user/someone",
            "https://www.youtube.com/playlist?list=PL000",
        ] {
            assert_eq!(route(raw), None, "{raw}");
        }
        for raw in [
            "https://x.com/someone/status/1234567890",
            "https://twitter.com/someone/status/1234567890/photo/1",
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "https://www.youtube.com/shorts/abc",
            "https://www.youtube.com/",
            "https://example.test/someone",
        ] {
            assert!(route(raw).is_some(), "{raw}");
        }
    }

    #[test]
    fn github_documents_route_to_gh_and_other_github_pages_to_the_web() {
        let repo = |owner: &str, name: &str| crate::github::Repo {
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        for (raw, target) in [
            (
                "https://github.com/owner/repo",
                Target::Repo(repo("owner", "repo")),
            ),
            (
                "https://www.github.com/Owner/Repo/?tab=readme-ov-file#install",
                Target::Repo(repo("Owner", "Repo")),
            ),
            (
                "https://github.com/owner/repo/issues/4",
                Target::Issue(repo("owner", "repo"), 4),
            ),
            (
                "https://github.com/owner/repo/pull/5/files",
                Target::Pull(repo("owner", "repo"), 5),
            ),
            (
                "https://github.com/owner/repo/discussions/6",
                Target::Discussion(repo("owner", "repo"), 6),
            ),
        ] {
            assert_eq!(route(raw), Some(Route::Github(target)), "{raw}");
        }
        for raw in [
            "https://github.com/owner/repo/blob/main/src/lib.rs",
            "https://github.com/owner/repo/releases",
            "https://github.com/owner",
            "https://github.com/settings/profile",
            "https://github.com/",
            "https://gist.github.com/owner/abc",
            "https://docs.github.com/en/rest",
        ] {
            assert_eq!(route(raw), Some(Route::Web), "{raw}");
        }
    }

    #[test]
    fn git_suffix_addresses_share_the_repository_fetch() {
        let root = "https://github.com/Owner/Repo";
        let alias = "https://github.com/Owner/Repo.git/?tab=readme-ov-file#install";
        let root_route = route(root).unwrap();
        let alias_route = route(alias).unwrap();
        assert_eq!(alias_route, root_route);
        assert_eq!(alias_route.key(alias), root_route.key(root));
    }

    #[test]
    fn a_github_document_is_one_fetch_whatever_its_address() {
        let key = |raw: &str| route(raw).unwrap().key(raw);
        assert_eq!(
            key("https://github.com/Owner/Repo"),
            key("https://www.github.com/owner/repo/?tab=readme-ov-file#install")
        );
        assert_eq!(
            key("https://github.com/owner/repo/issues/5"),
            key("https://github.com/owner/repo/pull/5/files")
        );
        assert_ne!(
            key("https://github.com/owner/repo"),
            key("https://github.com/owner/repo/issues/5")
        );
        let repo = route("https://github.com/owner/repo").unwrap();
        assert_eq!(repo.host(), Some(github_api::API_HOST));
        assert_eq!(repo.lanes(), github_api::CONCURRENT);
        assert_eq!(Route::Web.lanes(), 1);
    }

    #[test]
    fn fragments_are_dropped_for_grouping_only() {
        assert_eq!(without_fragment("https://a.test/p#one"), "https://a.test/p");
        assert_eq!(without_fragment("https://a.test/p"), "https://a.test/p");
        assert_eq!(
            without_fragment("https://a.test/p?q=1#"),
            "https://a.test/p?q=1"
        );
    }

    #[test]
    fn a_post_is_one_fetch_on_the_api_host_whatever_its_address() {
        let x = route("https://x.com/someone/status/42").unwrap();
        let twitter = route("https://twitter.com/someone/status/42#top").unwrap();
        assert_eq!(
            x.key("https://x.com/someone/status/42"),
            twitter.key("ignored")
        );
        assert_eq!(x.host(), Some(xpost::API_HOST));
        assert_eq!(Route::Web.host(), None);
        assert_eq!(Route::Web.key("https://a.test/p#one"), "https://a.test/p");
    }
}
