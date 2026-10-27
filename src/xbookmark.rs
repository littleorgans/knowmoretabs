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
//!      costs the bookmark. The first web link of the thread that leaves X
//!      is the article the post is about: it becomes a page of its own by
//!      the same admission, its tie to the post written to the X log under
//!      that hold of the lock, so a forgotten article stays forgotten and
//!      untied; its text and image are read in the background through
//!      `add`'s content stage, so the reply never waits on another site.
//!      Post images and Article covers also run in the background, after
//!      the post's text and title are safe, so a slow image cannot cost
//!      the extension its bookmark reply.
//!      One save at a time, so two clicks in one thread never race to mint
//!      two pages and the API sees one paced request at once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use url::Url;

use crate::add::{self, Outcome};
use crate::add_content::{Ask, Ended};
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
use crate::platform;
use crate::targets::Outcome as _;
use crate::xbookmark_log::{self, Line, Mark};
use crate::xpost;
use crate::xread::{self, Read};

/// Serializes saves across `serve`'s connection threads.
static SAVING: Mutex<()> = Mutex::new(());

/// Serializes background image and linked article reads, apart from saves.
static READING: Mutex<()> = Mutex::new(());

/// X itself, its link shortener and its media host: a link to one of them,
/// or to a host below one, is no article.
const X_HOSTS: [&str; 4] = ["x.com", "twitter.com", "t.co", "twimg.com"];

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
/// page and its click line, then its text and title, then the article it
/// links to; images run in the background. The reply is `add`'s last line,
/// saying what became of the read and, when a link qualified, of the article.
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
    let (ended, said, article) = match (&outcome, &shown) {
        (Outcome::Added(_) | Outcome::Known(_), _) => {
            let read = capture(root, url, post, &shown)?;
            let article = article(root, url, &read.links)?;
            (read.ended, read.said, article)
        }
        (_, Shown::Refused) => (Ended::default(), "refused", None),
        _ => (Ended::default(), "none", None),
    };
    let mut also = json!({"read": said});
    if let Some(article) = article {
        also["article"] = article;
    }
    Ok(add::done_line(url, &outcome, &ended, also))
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

/// What became of a saved post's text.
struct Captured {
    /// How the text and image stand.
    ended: Ended,
    /// What became of the page read: `kept`, `replaced` by the public
    /// thread, `failed` (it named no post), `refused` or `none`.
    said: &'static str,
    /// Where the thread's links lead: the public thread's, in its order,
    /// then the page read's.
    links: Vec<String>,
}

/// Library page `url`'s text from what the page showed, then from the
/// public post API's thread unless the page showed it whole, and its title
/// when it had none. Its images follow in the background, the public
/// capture's alone when it replaced the page read.
fn capture(root: &Path, url: &str, post: &Post, shown: &Shown) -> Result<Captured, Error> {
    let fetcher = Fetcher::new(&State::read(root)?.forgotten);
    let mut images = Vec::new();
    let (mut said, mut content, whole) = match shown {
        Shown::Nothing => ("none", None, false),
        Shown::Refused => ("refused", None, false),
        Shown::Read(read) if read.posts.is_empty() => ("failed", None, false),
        Shown::Read(read) => {
            let capture = xread::capture(read, url, &post.key);
            let kept = content_keep::keep(root, url, &capture, "page read added no text")?;
            let line = kept.line.clone();
            images.push(kept);
            ("kept", Some(line), read.complete)
        }
    };
    let mut links: Vec<String> = match shown {
        Shown::Read(read) => read.links().cloned().collect(),
        _ => Vec::new(),
    };
    if !whole {
        let thread = xpost::thread_capture(&fetcher, url, &post.id);
        let public = content_keep::keep(root, url, &thread.capture, "public read added no text")?;
        if said == "kept" && public.taken {
            said = "replaced";
            // The public capture's image stands for the page: the page
            // read's would only be overwritten.
            images.clear();
        }
        content = Some(public.line.clone());
        images.push(public);
        links.splice(0..0, thread.links);
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
    let root = root.to_owned();
    std::thread::spawn(move || {
        let _reading = READING.lock().unwrap_or_else(PoisonError::into_inner);
        for kept in images {
            let _ = kept.image(&root, &fetcher, QUIET);
        }
    });
    Ok(Captured { ended, said, links })
}

/// The article page `url`'s thread links to: the first of `links` that
/// leaves X and that `add` would take, admitted with its tie to `url`, its
/// text and image then read in the background. What became of it, as
/// `add`'s library stage says it; `None` when no link qualified.
fn article(root: &Path, url: &str, links: &[String]) -> Result<Option<Value>, Error> {
    let Some(link) = links
        .iter()
        .find(|link| leaves_x(link) && add::refusal(link).is_none())
    else {
        return Ok(None);
    };
    let at = archive::now();
    let outcome = add::admit(
        root,
        link,
        None,
        false,
        || {},
        |lock, held| xbookmark_log::append(root, &Line::link(url, at, held), lock),
    )?;
    let key = outcome.key().unwrap_or(link).to_owned();
    let said = json!({"url": key, "value": outcome.value()});
    if outcome.in_library() {
        let root = root.to_owned();
        std::thread::spawn(move || {
            let _reading = READING.lock().unwrap_or_else(PoisonError::into_inner);
            add::capture(&root, &key, ASK, false, QUIET);
        });
    }
    Ok(Some(said))
}

/// Whether `link` is a web address off X's own hosts.
fn leaves_x(link: &str) -> bool {
    let Some(url) = Url::parse(link).ok().filter(guard::is_web) else {
        return false;
    };
    let host = guard::host_key(&url);
    !X_HOSTS.iter().any(|x| {
        host.strip_suffix(x)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

/// The public tiers only: an article's read never opens the owner's
/// browser signed in.
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
                key: "https://x.com/example_author/status/100".to_owned(),
                id: "100".to_owned(),
                handle: "example_author".to_owned(),
            },
            "two casings of one handle are one page"
        );
    }

    #[test]
    fn the_article_is_a_web_link_off_xs_own_hosts() {
        for link in [
            "https://blog.example.test/post",
            "http://www.video.example.test/watch?v=EXAMPLE",
            "https://code.example.test/example/repo",
            "https://notx.com/post",
            "https://x.company.test/post",
        ] {
            assert!(leaves_x(link), "{link}");
        }
        for link in [
            "https://x.com/example_other/status/200",
            "https://X.com./i/article/1",
            "https://pic.x.com/EXAMPLE",
            "https://pic.twitter.com/EXAMPLE",
            "https://mobile.twitter.com/example_other",
            "https://t.co/EXAMPLE",
            "https://pbs.twimg.com/media/EXAMPLE.jpg",
            "mailto:someone@example.test",
            "not a link",
        ] {
            assert!(!leaves_x(link), "{link}");
        }
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
