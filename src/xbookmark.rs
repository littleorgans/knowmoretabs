//! `serve`'s X bookmark route: a post the owner bookmarked on x.com, put in
//! the library as one page and read through the X post route.
//!
//! slice: x-bookmarks
//! why: Bookmarking on X is where the owner already says "keep this", so a
//!      second step in the Library is the wrong cost. The toolbar extension
//!      names the clicked post and nothing else; the post is kept under one
//!      canonical address, its handle in one case, so the same post from
//!      Home, its own page or twitter.com is one page, and anything that is
//!      not a post with its author's handle writes nothing. A post of a
//!      thread already saved is that thread's page, found by the post
//!      numbers the X log holds for it. The page goes in through `add`'s
//!      own admission, with the click written to the X log under the same
//!      hold of the lock, so a listed page always has its mark and a
//!      forgotten one stays forgotten with nothing written. Its text is
//!      first what the owner's page showed (`xread`); when that read is
//!      missing, refused or may not be the whole thread, the public post
//!      API is asked for the thread by the post's number alone, and the
//!      longer text wins (`content_keep`). A read the shell refuses never
//!      costs the bookmark. One save at a time, so two clicks in one
//!      thread never race to mint two pages and the API sees one paced
//!      request at once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use url::Url;

use crate::add::{self, Outcome};
use crate::add_content::Ended;
use crate::archive;
use crate::capture::Log;
use crate::content_keep;
use crate::content_route::Route;
use crate::content_store;
use crate::error::Error;
use crate::fetch::Fetcher;
use crate::guard;
use crate::image_store;
use crate::intake;
use crate::library::State;
use crate::targets::Outcome as _;
use crate::xbookmark_log::{self, Line, Mark};
use crate::xpost;
use crate::xread::{self, Read};

/// Serializes saves across `serve`'s connection threads.
static SAVING: Mutex<()> = Mutex::new(());

/// A bookmarked post: the page key it is kept under and its number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// `https://x.com/<handle>/status/<id>`, the handle in lower case.
    pub key: String,
    pub id: String,
    /// The author's handle, in lower case: X reads handles in any case.
    pub handle: String,
}

impl Post {
    /// An X post's address, by the X route's rule for hosts, with its
    /// author's handle in lower case; query, fragment and anything after
    /// the number (`/photo/1`) dropped. `None` for anything else,
    /// `/i/status/<id>` included: it names no author.
    pub fn parse(raw: &str) -> Option<Self> {
        let url = Url::parse(raw).ok().filter(guard::is_web)?;
        let Some(Route::XPost(id)) = Route::of(&url) else {
            return None;
        };
        let handle = url.path_segments()?.find(|segment| !segment.is_empty())?;
        let handle = handle.to_ascii_lowercase();
        if !is_handle(&handle) || handle == "i" || id.len() > 19 {
            return None;
        }
        Some(Self {
            key: format!("https://x.com/{handle}/status/{id}"),
            id,
            handle,
        })
    }
}

/// X's rule for a handle: one to fifteen letters, digits or underscores.
pub fn is_handle(handle: &str) -> bool {
    (1..=15).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Saves `post`, with what the owner's page showed of it, `read`: the
/// page and its click line, then its text, image and title. The reply is
/// `add`'s last line, saying what became of the read.
pub fn save(root: &Path, post: &Post, read: Option<&Value>) -> Result<Value, Error> {
    let _saving = SAVING.lock().unwrap_or_else(PoisonError::into_inner);
    let shown = match read.map(|value| xread::parse(value, &post.id, &post.handle)) {
        None => Shown::Nothing,
        Some(Ok(read)) => Shown::Read(read),
        Some(Err(_)) => Shown::Refused,
    };
    let ids = match &shown {
        Shown::Read(read) if !read.posts.is_empty() => read.ids(),
        _ => vec![post.id.clone()],
    };
    let key = thread_page(&xbookmark_log::for_library(root, QUIET), &ids)
        .unwrap_or_else(|| post.key.clone());
    let at = archive::now();
    let outcome = add::admit(
        root,
        &key,
        None,
        false,
        || {},
        |lock, held| {
            xbookmark_log::append(root, &Line::click(held, at, &post.key, ids.clone()), lock)
        },
    )?;
    let url = outcome.key().unwrap_or(&key);
    let (ended, said) = match (&outcome, &shown) {
        (Outcome::Added(_) | Outcome::Known(_), _) => capture(root, url, post, &shown)?,
        (_, Shown::Refused) => (Ended::default(), "refused"),
        _ => (Ended::default(), "none"),
    };
    Ok(add::done_line(url, &outcome, &ended, json!({"read": said})))
}

/// What the owner's page showed of a bookmarked post, by the request.
enum Shown {
    /// No read came with the post.
    Nothing,
    /// A read came and failed `xread`'s checks.
    Refused,
    Read(Read),
}

/// The page saved for the thread holding any of posts `ids`: the latest
/// bookmarked of them, should there be more than one.
fn thread_page(marks: &HashMap<String, Mark>, ids: &[String]) -> Option<String> {
    marks
        .iter()
        .filter(|(_, mark)| mark.thread.iter().any(|id| ids.contains(id)))
        .max_by(|(a, x), (b, y)| (x.bookmarked_at, a).cmp(&(y.bookmarked_at, b)))
        .map(|(url, _)| url.clone())
}

/// Library page `url`'s text from what the page showed, then from the
/// public post API's thread unless the page showed it whole; its image,
/// and its title when it had none. How the text and image stand, and what
/// became of the page read: `kept`, `replaced` by the public thread,
/// `failed` (it named no post), `refused` or `none`.
fn capture(
    root: &Path,
    url: &str,
    post: &Post,
    shown: &Shown,
) -> Result<(Ended, &'static str), Error> {
    let fetcher = Fetcher::new(&State::read(root)?.forgotten);
    let (mut said, mut content, whole) = match shown {
        Shown::Nothing => ("none", None, false),
        Shown::Refused => ("refused", None, false),
        Shown::Read(read) if read.posts.is_empty() => ("failed", None, false),
        Shown::Read(read) => {
            let capture = xread::capture(read, url, &post.key);
            let kept = content_keep::keep(
                root,
                &fetcher,
                url,
                &capture,
                "page read added no text",
                QUIET,
            )?;
            ("kept", Some(kept.line), read.complete)
        }
    };
    if !whole {
        let thread = xpost::thread_capture(&fetcher, url, &post.id);
        let public = content_keep::keep(
            root,
            &fetcher,
            url,
            &thread.capture,
            "public read added no text",
            QUIET,
        )?;
        if said == "kept" && public.taken {
            said = "replaced";
        }
        content = Some(public.line);
        if !thread.posts.is_empty() {
            xbookmark_log::learned(root, &Line::thread(url, archive::now(), thread.posts))?;
        }
    }
    if let Some(title) = content_store::kept_text(root, url)?.and_then(|text| text.title) {
        intake::retitle(root, url, &title, || {})?;
    }
    let ended = Ended {
        content: content.map(|line| line.status.word().to_owned()),
        image: image_store::read(root)?
            .pages
            .get(url)
            .map(|line| line.status.word().to_owned()),
        title: None,
    };
    Ok((ended, said))
}

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
                key: "https://x.com/example_author/status/100".to_owned(),
                id: "100".to_owned(),
                handle: "example_author".to_owned(),
            },
            "two casings of one handle are one page"
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
