//! The page read: what the toolbar extension saw of a bookmarked post's
//! thread on x.com, checked and kept as the page's text.
//!
//! slice: x-bookmarks, content
//! why: The owner's own view of X shows what the public post API may not:
//!      a protected account, or a thread as the owner was reading it. The
//!      extension takes it from the page at the click, and `serve` trusts
//!      none of it unchecked: post numbers, handles and times must have
//!      X's shapes, every post must be the clicked post's author's, and
//!      each part is bounded, so a reader bug or a changed page is refused
//!      whole and the save goes on through the public service instead. An
//!      image is only ever one X itself serves (`pbs.twimg.com`): the page
//!      may name a post, never a place for `serve` to fetch. The read
//!      becomes a capture like any other, read signed in, drawn by the
//!      same renderer as the public read; whether it is complete decides
//!      only whether the public service is asked for the rest.

use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::content_fetch::{self, Capture};
use crate::content_store::{Completeness, Page, Status, Tier};
use crate::extract;
use crate::image_pick::{Candidate, Found, Source};
use crate::triage::plural;
use crate::xbookmark::is_handle;
use crate::xpost_page::{self, Author};

/// Recorded as the extractor, with [`VERSION`].
pub const EXTRACTOR: &str = "x-page";
pub const VERSION: &str = "1";

/// The one host X serves post images from.
const MEDIA_HOST: &str = "pbs.twimg.com";

const MAX_POSTS: usize = 50;
const MAX_TEXT: usize = 25_000;
const MAX_NAME: usize = 200;
const MAX_ALTS: usize = 8;
const MAX_MEDIA: usize = 4;
const MAX_LINKS: usize = 10;
const MAX_LANG: usize = 35;

/// The reasons a read may give for being incomplete; any other is dropped.
const REASONS: [&str; 6] = [
    "timeline",
    "unexpanded_replies",
    "text_truncated",
    "recommendation_boundary",
    "unresolved_link",
    "missing_identity",
];

/// A read as the extension sends it; checked by [`parse`].
#[derive(Debug, Deserialize)]
struct Sent {
    #[serde(rename = "where")]
    place: String,
    clicked: String,
    author: String,
    complete: bool,
    #[serde(default)]
    reasons: Vec<String>,
    posts: Vec<SentPost>,
}

#[derive(Debug, Deserialize)]
struct SentPost {
    id: String,
    handle: String,
    #[serde(default)]
    name: String,
    at: Option<String>,
    lang: Option<String>,
    #[serde(default)]
    text: String,
    text_complete: bool,
    #[serde(default)]
    alts: Vec<String>,
    #[serde(default)]
    media: Vec<String>,
    #[serde(default)]
    links: Vec<SentLink>,
    quote: Option<SentQuote>,
}

#[derive(Debug, Deserialize)]
struct SentLink {
    href: String,
    #[serde(default)]
    label: String,
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SentQuote {
    url: String,
    handle: String,
    #[serde(default)]
    text: String,
}

/// A read that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The author's posts as the page showed them, in its order.
    pub posts: Vec<ReadPost>,
    /// The page showed the whole thread, by the extension's rule and the
    /// shell's: nothing truncated, nothing folded, read on the post's own
    /// page.
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPost {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub at: Option<Timestamp>,
    pub lang: Option<String>,
    pub text: String,
    pub alts: Vec<String>,
    /// The post's images on X's own image host.
    pub media: Vec<String>,
    pub quote: Option<Quote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub handle: String,
    pub text: String,
}

impl Read {
    /// The posts' numbers, in the page's order.
    pub fn ids(&self) -> Vec<String> {
        self.posts.iter().map(|post| post.id.clone()).collect()
    }
}

/// The read `value` for a click on post number `clicked` by `handle`, or
/// why it is refused.
pub fn parse(value: &Value, clicked: &str, handle: &str) -> Result<Read, String> {
    let sent = Sent::deserialize(value).map_err(|err| format!("not a page read: {err}"))?;
    if sent.place != "status" && sent.place != "timeline" {
        return Err(format!("where: {}", sent.place));
    }
    if sent.clicked != clicked {
        return Err("clicked: not the bookmarked post".to_owned());
    }
    if !sent.author.eq_ignore_ascii_case(handle) {
        return Err("author: not the bookmarked post's".to_owned());
    }
    if sent.posts.len() > MAX_POSTS {
        return Err(format!("posts: more than {MAX_POSTS}"));
    }
    // Cut text and unresolved links ask for the rest even when the reader
    // fails to include the corresponding reason.
    let parts_complete = sent
        .posts
        .iter()
        .all(|post| post.text_complete && post.links.iter().all(|link| link.target.is_some()));
    let mut posts: Vec<ReadPost> = Vec::with_capacity(sent.posts.len());
    for post in sent.posts {
        let post = checked(post, handle)?;
        if posts.iter().any(|seen| seen.id == post.id) {
            return Err(format!("posts: {} twice", post.id));
        }
        posts.push(post);
    }
    if !posts.is_empty() && !posts.iter().any(|post| post.id == clicked) {
        return Err("posts: the bookmarked post is not among them".to_owned());
    }
    let reasons = sent
        .reasons
        .iter()
        .filter(|reason| REASONS.contains(&reason.as_str()));
    let complete =
        sent.complete && sent.place == "status" && parts_complete && reasons.count() == 0;
    Ok(Read { posts, complete })
}

fn checked(post: SentPost, handle: &str) -> Result<ReadPost, String> {
    let id = &post.id;
    if !is_id(id) {
        return Err(format!("post id: {id}"));
    }
    if !is_handle(&post.handle) || !post.handle.eq_ignore_ascii_case(handle) {
        return Err(format!("post {id}: not the author's"));
    }
    let at = post
        .at
        .as_deref()
        .map(str::parse::<Timestamp>)
        .transpose()
        .map_err(|_| format!("post {id}: at is not RFC 3339"))?;
    if post.lang.as_deref().is_some_and(|lang| {
        lang.len() > MAX_LANG || !lang.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return Err(format!("post {id}: lang"));
    }
    let long = |text: &str, most: usize| text.chars().count() > most;
    if long(&post.text, MAX_TEXT) || long(&post.name, MAX_NAME) {
        return Err(format!("post {id}: text or name too long"));
    }
    if post.alts.len() > MAX_ALTS || post.alts.iter().any(|alt| long(alt, MAX_TEXT)) {
        return Err(format!("post {id}: alts"));
    }
    if post.media.len() > MAX_MEDIA {
        return Err(format!("post {id}: media"));
    }
    if post.links.len() > MAX_LINKS || !post.links.iter().all(is_link) {
        return Err(format!("post {id}: links"));
    }
    let quote = match post.quote {
        Some(quote) if is_quote(&quote) => Some(Quote {
            handle: quote.handle,
            text: quote.text,
        }),
        Some(_) => return Err(format!("post {id}: quote")),
        None => None,
    };
    Ok(ReadPost {
        media: post
            .media
            .into_iter()
            .filter(|url| is_x_media(url))
            .collect(),
        id: post.id,
        handle: post.handle,
        name: post.name,
        at,
        lang: post.lang,
        text: post.text,
        alts: post.alts,
        quote,
    })
}

/// A post number: one to nineteen digits.
fn is_id(id: &str) -> bool {
    (1..=19).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

fn is_web(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn is_link(link: &SentLink) -> bool {
    is_web(&link.href)
        && link.label.chars().count() <= MAX_TEXT
        && link.target.as_deref().is_none_or(is_web)
}

fn is_quote(quote: &SentQuote) -> bool {
    is_web(&quote.url) && is_handle(&quote.handle) && quote.text.chars().count() <= MAX_TEXT
}

fn is_x_media(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some(MEDIA_HOST))
}

/// The capture of `read` for library page `key`, the clicked post being
/// `post`: read signed in, its text drawn as the public read's is, its
/// image the first post's first.
pub fn capture(read: &Read, key: &str, post: &str) -> Capture {
    let posts: Vec<xpost_page::Post> = read.posts.iter().map(neutral).collect();
    let markdown = xpost_page::markdown(&posts);
    let has_text = read.posts.iter().any(|post| !post.text.trim().is_empty());
    let mut line = content_fetch::line(
        key,
        Tier::SignedIn,
        if has_text { Status::Ok } else { Status::Thin },
    );
    let mut reason = format!("{} as shown", plural(read.posts.len(), "post"));
    if !read.complete {
        reason.push_str("; thread may continue");
    }
    line.reason = Some(reason);
    line.final_url = Some(post.to_owned());
    line.lang = read.posts.first().and_then(|first| first.lang.clone());
    line.extractor = Some(EXTRACTOR.to_owned());
    line.extractor_version = Some(VERSION.to_owned());
    let page = Page {
        title: Some(xpost_page::title(&posts)),
        extractor: format!("{EXTRACTOR} {VERSION}"),
        completeness: if has_text {
            Completeness::Full
        } else {
            Completeness::Thin
        },
        chars: extract::plain_chars(&markdown),
        captions: None,
        markdown,
    };
    let image = read.posts.first().and_then(|first| first.media.first());
    Capture {
        line,
        page: Some(page),
        images: image.map_or(Found::TextOnly, |url| {
            Found::Candidates(vec![Candidate {
                url: url.clone(),
                source: Source::XMedia,
            }])
        }),
    }
}

fn neutral(post: &ReadPost) -> xpost_page::Post {
    xpost_page::Post {
        author: Some(Author {
            name: post.name.trim().to_owned(),
            handle: post.handle.clone(),
        }),
        at: post.at,
        text: post.text.clone(),
        alts: post
            .alts
            .iter()
            .filter(|alt| !alt.trim().is_empty())
            .map(|alt| ("Image", alt.trim().to_owned()))
            .collect(),
        quote: post.quote.as_ref().map(|quote| {
            Box::new(xpost_page::Post {
                author: Some(Author {
                    name: String::new(),
                    handle: quote.handle.clone(),
                }),
                text: quote.text.clone(),
                ..xpost_page::Post::default()
            })
        }),
        ..xpost_page::Post::default()
    }
}

#[cfg(test)]
#[path = "xread_tests.rs"]
mod tests;
