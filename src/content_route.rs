//! Which route reads a library page, judged from its address alone.
//!
//! slice: content
//! why: Each kind of document has a cheapest way to its text: an X post
//!      through the X post API, anything else from its own site. Choosing
//!      it from the address, before any request, lets the plan say which
//!      host every request goes to, fetch one post once whatever address it
//!      was opened at, and keep profiles, channels and playlists home as not
//!      documents. A new route is a variant here, and `content` itself does
//!      not change.

use url::Url;

use crate::content_fetch::{self, Capture};
use crate::fetch::Fetcher;
use crate::guard;
use crate::xpost;

/// How a document is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// From its own site, as a web page.
    Web,
    /// From the X post API, by the post's number.
    XPost(String),
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
        }
    }

    /// What one fetch stands for: page `raw` without its fragment, or the
    /// post, whatever address it was opened at.
    pub fn key(&self, raw: &str) -> String {
        match self {
            Self::Web => without_fragment(raw),
            Self::XPost(id) => xpost::api_url(id),
        }
    }

    pub fn capture(&self, fetcher: &Fetcher, raw: &str) -> Capture {
        match self {
            Self::Web => content_fetch::capture(fetcher, raw),
            Self::XPost(id) => xpost::capture(fetcher, raw, id),
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
