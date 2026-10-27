//! `serve`'s X bookmark route: a post the owner bookmarked on x.com, put in
//! the library as one page and read through the X post route.
//!
//! slice: x-bookmarks
//! why: Bookmarking on X is where the owner already says "keep this", so a
//!      second step in the Library is the wrong cost. The toolbar extension
//!      names the clicked post and nothing else; the post is kept under one
//!      canonical address, so the same post from Home, its own page or
//!      twitter.com is one page, and anything that is not a post with its
//!      author's handle writes nothing. The page goes in through `add`'s own
//!      admission, with the click written to the X log under the same hold
//!      of the lock, so a listed page always has its mark and a forgotten
//!      one stays forgotten with nothing written. Its text and image come
//!      from `add`'s content stage, which asks the public post API for the
//!      post's number alone. One save at a time, so two clicks never race
//!      to mint one page twice and the API sees one paced request at once.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use url::Url;

use crate::add::{self, Outcome};
use crate::add_content::{Ask, Ended};
use crate::archive;
use crate::capture::Log;
use crate::content_route::Route;
use crate::error::Error;
use crate::guard;
use crate::platform;
use crate::xbookmark_log::{self, Line};

/// Serializes saves across `serve`'s connection threads.
static SAVING: Mutex<()> = Mutex::new(());

/// A bookmarked post: the page key it is kept under and its number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// `https://x.com/<handle>/status/<id>`.
    pub key: String,
    pub id: String,
}

impl Post {
    /// An X post's address, by the X route's rule for hosts, with its
    /// author's handle; query, fragment and anything after the number
    /// (`/photo/1`) dropped. `None` for anything else, `/i/status/<id>`
    /// included: it names no author.
    pub fn parse(raw: &str) -> Option<Self> {
        let url = Url::parse(raw).ok().filter(guard::is_web)?;
        let Some(Route::XPost(id)) = Route::of(&url) else {
            return None;
        };
        let handle = url.path_segments()?.find(|segment| !segment.is_empty())?;
        if !is_handle(handle) || handle == "i" || id.len() > 19 {
            return None;
        }
        Some(Self {
            key: format!("https://x.com/{handle}/status/{id}"),
            id,
        })
    }
}

/// X's rule for a handle: one to fifteen letters, digits or underscores.
fn is_handle(handle: &str) -> bool {
    (1..=15).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Saves `post`: the page and its click line, then its text, image and
/// title by `add`'s content stage. The reply is `add`'s last line.
pub fn save(root: &Path, post: &Post) -> Result<Value, Error> {
    let _saving = SAVING.lock().unwrap_or_else(PoisonError::into_inner);
    let at = archive::now();
    let outcome = add::admit(
        root,
        &post.key,
        None,
        false,
        || {},
        |lock, key| xbookmark_log::append(root, &Line::click(key, at, &post.key, &post.id), lock),
    )?;
    let url = outcome.key().unwrap_or(&post.key);
    let ended = match outcome {
        Outcome::Added(_) | Outcome::Known(_) => add::capture(root, url, ASK, false, QUIET),
        Outcome::Forgotten(_) | Outcome::Refused(_) => Ended::default(),
    };
    Ok(add::done_line(url, &outcome, &ended, json!({})))
}

/// The public tiers only: a save never opens the owner's browser.
const ASK: Ask<'static> = Ask {
    browser: platform::CHROME,
    signed_in: false,
    retry: &[],
};

/// The route answers with its reply; nothing is said on `serve`'s terminal.
const QUIET: Log = Log {
    quiet: true,
    verbose: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str) -> Option<String> {
        Post::parse(raw).map(|post| post.key)
    }

    #[test]
    fn a_post_is_kept_under_its_x_com_address_whatever_host_or_suffix() {
        let kept = Some("https://x.com/example_author/status/100".to_owned());
        for raw in [
            "https://x.com/example_author/status/100",
            "https://twitter.com/example_author/status/100",
            "https://www.x.com/example_author/status/100",
            "https://mobile.twitter.com/example_author/status/100",
            "https://m.x.com/example_author/status/100",
            "http://x.com/example_author/status/100",
            "https://x.com/example_author/status/100/photo/1",
            "https://x.com/example_author/status/100?s=20&t=abc",
            "https://x.com/example_author/status/100#reply",
            "https://x.com/example_author/status/100/",
        ] {
            assert_eq!(key(raw), kept, "{raw}");
        }
        assert_eq!(
            Post::parse("https://x.com/Example_Author/status/100").unwrap(),
            Post {
                key: "https://x.com/Example_Author/status/100".to_owned(),
                id: "100".to_owned()
            },
            "the handle is kept as given"
        );
    }

    #[test]
    fn anything_but_a_post_with_its_authors_handle_is_refused() {
        for raw in [
            "https://x.com/i/status/100",
            "https://x.com/example_author",
            "https://x.com/example_author/likes/100",
            "https://x.com/example_author/status/",
            "https://x.com/example_author/status/abc",
            "https://x.com/sixteen_letters_/status/100",
            "https://x.com/bad-handle/status/100",
            "https://x.com/example_author/status/12345678901234567890",
            "https://notx.com/example_author/status/100",
            "https://example.org/example_author/status/100",
            "ftp://x.com/example_author/status/100",
            "x.com/example_author/status/100",
            "",
        ] {
            assert_eq!(key(raw), None, "{raw}");
        }
    }
}
