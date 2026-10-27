//! The X post route: a post on `x.com` or `twitter.com`, read through the
//! public X post API (`api.fxtwitter.com`, version 2) and kept as markdown.
//!
//! slice: content, x-bookmarks
//! why: An X post page is an app shell to plain HTTP, so the generic route
//!      keeps nothing of it, while the public post API answers with the
//!      post itself. It is the one third party service content capture
//!      uses, so it is asked as little as possible: only the post's number,
//!      never the page's address, cookieless and one request a second.
//!      What is kept is what a reader of the post sees as text: the post,
//!      the post it quotes, an article's body with a placeholder where it
//!      shows an image or video, who wrote it and when, and what its
//!      images say in their descriptions. A `content` run reads one post:
//!      a thread is not unrolled there. A bookmark on X may ask for the
//!      post's thread instead, and keeps only its author's posts, in the
//!      API's order, a deleted one as one line, and where their links lead,
//!      so the article the thread links to can be saved beside it. A
//!      deleted post is final, a
//!      private one waits for a signed in browser, and a busy API is
//!      retried as any site is. The markdown is `xpost_page`'s, shared
//!      with the page read of X itself.

use jiff::Timestamp;
use serde::Deserialize;
use url::Url;

use crate::content_fetch::{self, Capture, Passing};
use crate::content_store::{Completeness, Line, Page, Status, Tier};
use crate::extract;
use crate::fetch::{self, Fetcher};
use crate::head;
use crate::image_pick::{Candidate, Found, Source};
use crate::xpost_page;

/// The API's host: the one host every X post is paced on.
pub const API_HOST: &str = "api.fxtwitter.com";
/// Recorded as the extractor, with [`API_VERSION`].
pub const EXTRACTOR: &str = "fxtwitter";
pub const API_VERSION: &str = "2";
const ACCEPT_JSON: &str = "application/json";

/// The post number in an X address's path, `/<user>/status/<id>`, with
/// anything after the number (`/photo/1`) ignored. The caller has checked
/// the host.
pub fn post_id(url: &Url) -> Option<String> {
    let mut segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty());
    let (_, status, id) = (segments.next(), segments.next()?, segments.next()?);
    (status == "status" && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
}

pub fn api_url(id: &str) -> String {
    format!("{}://{API_HOST}/{API_VERSION}/status/{id}", scheme())
}

/// The same post with the thread it is part of, when the API has one.
pub fn thread_url(id: &str) -> String {
    format!("{}://{API_HOST}/{API_VERSION}/thread/{id}", scheme())
}

/// `https`, except in a debug build whose every name resolves to a test's
/// own server (`fetch::test_address`), which speaks plain HTTP.
fn scheme() -> &'static str {
    if fetch::test_address().is_some() {
        "http"
    } else {
        "https"
    }
}

/// The public post the API's own documentation uses as its example:
/// `doctor --live` asks for it, never for one of the owner's.
const SAMPLE_POST: &str = "20";

/// The HTTP status the API answers for [`SAMPLE_POST`], or why it gave
/// none.
pub fn reachable(fetcher: &Fetcher) -> Result<u16, String> {
    fetcher
        .get(&api_url(SAMPLE_POST), ACCEPT_JSON)
        .map(|response| response.status)
        .map_err(|refusal| refusal.reason())
}

/// Asks the API for post `id`, recorded for library page `raw`, retrying
/// passing failures as the web route does. Never fails: a failure is a
/// capture too.
pub fn capture(fetcher: &Fetcher, raw: &str, id: &str) -> Capture {
    content_fetch::retrying(fetcher, || {
        once(fetcher, raw, &api_url(id), |bytes, line| read(bytes, line))
    })
}

/// What the API gave of a post's thread: the capture of its author's
/// posts, their numbers and the web links they make, in the API's order.
#[derive(Debug, Clone)]
pub struct Thread {
    pub capture: Capture,
    pub posts: Vec<String>,
    /// Each post's link targets, else its card's address.
    pub links: Vec<String>,
}

/// Asks the API for post `id` and its thread, recorded for library page
/// `raw`, as [`capture`] asks for the post alone. Only the post's author's
/// posts are kept; another's reply in the thread is not.
pub fn thread_capture(fetcher: &Fetcher, raw: &str, id: &str) -> Thread {
    let (mut posts, mut links) = (Vec::new(), Vec::new());
    let capture = content_fetch::retrying(fetcher, || {
        once(fetcher, raw, &thread_url(id), |bytes, line| {
            let thread = read_posts(bytes, line, true);
            (posts, links) = (thread.posts, thread.links);
            thread.capture
        })
    });
    Thread {
        capture,
        posts,
        links,
    }
}

impl Thread {
    /// A capture that read no post.
    fn ended(capture: Capture) -> Self {
        Self {
            capture,
            posts: Vec::new(),
            links: Vec::new(),
        }
    }
}

/// One request to the API at `url`, its successful body read by `read`.
fn once(
    fetcher: &Fetcher,
    raw: &str,
    url: &str,
    read: impl FnOnce(&[u8], &dyn Fn(Status) -> Line) -> Capture,
) -> Result<Capture, Passing> {
    let mut response = match fetcher.get(url, ACCEPT_JSON) {
        Ok(response) => response,
        Err(refusal) => return content_fetch::refused(raw, Tier::X, refusal),
    };
    if response.status == 429 {
        fetcher.slow_down(&response.url);
    }
    let status = response.status;
    let line = |state: Status, reason: Option<String>| {
        let mut line = content_fetch::line(raw, Tier::X, state);
        line.reason = reason;
        line.http_status = Some(status);
        line
    };
    if let Some((state, passing)) = content_fetch::status_outcome(status) {
        let failed = line(state, Some(failure_reason(state, status)));
        if passing {
            return Err(content_fetch::passing(&response, failed));
        }
        return Ok(Capture::ended(failed));
    }
    let mime = response.mime();
    if !mime.is_empty() && !mime.contains("json") {
        return Ok(Capture::ended(line(
            Status::Error,
            Some(format!("not JSON ({mime})")),
        )));
    }
    let bytes = match content_fetch::body(&mut response, |state, reason| line(state, Some(reason)))
    {
        Ok(bytes) => bytes,
        Err(ended) => return *ended,
    };
    Ok(read(&bytes, &|state| line(state, None)))
}

/// What a final or passing HTTP status says about a post.
fn failure_reason(state: Status, status: u16) -> String {
    match state {
        Status::BehindLogin => format!("private post (HTTP {status})"),
        Status::NotFound => format!("post not found (HTTP {status})"),
        _ => format!("HTTP {status}"),
    }
}

/// The API's answer: the post, or a tombstone in its place, and from the
/// thread route the thread it is part of, itself included.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Answer {
    code: Option<u16>,
    status: Option<Post>,
    thread: Option<Vec<Post>>,
}

/// A post as the API gives it; only what is kept is read. A tombstone has
/// `type` `tombstone` and a `reason`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Post {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    url: Option<String>,
    text: String,
    created_timestamp: Option<serde_json::Number>,
    lang: Option<String>,
    author: Option<Author>,
    quote: Option<Box<Post>>,
    article: Option<Article>,
    media: Option<Media>,
    raw_text: Option<RawText>,
    card: Option<Card>,
    /// Why a tombstone stands in for the post.
    reason: Option<String>,
}

/// The post's text as X stores it, with what each span of it stands for.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawText {
    facets: Vec<Facet>,
}

/// A span of the text: for `type` `url`, the `t.co` address the text
/// holds and, as `replacement`, where it leads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Facet {
    #[serde(rename = "type")]
    kind: String,
    replacement: Option<String>,
}

/// The preview X draws for a post's link.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Card {
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Author {
    name: String,
    screen_name: String,
    protected: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Article {
    title: String,
    content: Option<ArticleContent>,
    /// What each MEDIA entity's `mediaId` names.
    media_entities: Vec<ArticleMedium>,
    /// The image the article opens with.
    cover_media: Option<ArticleMedium>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ArticleContent {
    blocks: Vec<Block>,
    #[serde(rename = "entityMap")]
    entities: Vec<Entity>,
}

/// One paragraph of an article, in the editor's own block types.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: String,
    #[serde(rename = "entityRanges")]
    entities: Vec<EntityRange>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EntityRange {
    key: u64,
}

/// Entity keys are strings in the map and numbers in the block ranges.
/// The map's array order does not determine a key.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Entity {
    key: String,
    value: EntityValue,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EntityValue {
    #[serde(rename = "type")]
    kind: String,
    data: EntityData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EntityData {
    markdown: String,
    #[serde(rename = "tweetId")]
    tweet_id: String,
    #[serde(rename = "mediaItems")]
    media_items: Vec<MediaItem>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MediaItem {
    #[serde(rename = "mediaId")]
    media_id: String,
}

/// An article's image or video, as X itself lists it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ArticleMedium {
    media_id: String,
    media_info: MediaInfo,
}

/// `ApiImage`, `ApiVideo` or `ApiGif`. The published schema gives a
/// description to videos and GIFs only; one on an image is read the same.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MediaInfo {
    #[serde(rename = "__typename")]
    kind: String,
    ext_alt_text: Option<String>,
    /// An image's address, or a video's still.
    original_img_url: Option<String>,
    media_url_https: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Media {
    photos: Vec<Medium>,
    videos: Vec<Video>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Medium {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    #[serde(rename = "altText")]
    alt_text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Video {
    thumbnail_url: Option<String>,
}

/// The line and text for the body of a successful answer; `line` makes
/// the response's line for a status.
fn read(bytes: &[u8], line: impl Fn(Status) -> Line) -> Capture {
    read_posts(bytes, &line, false).capture
}

/// [`read`] of the post, or with `thread` of its author's posts in its
/// thread; with the numbers of the posts kept and their links.
fn read_posts(bytes: &[u8], line: &dyn Fn(Status) -> Line, thread: bool) -> Thread {
    let done = |state: Status, reason: String| {
        Thread::ended(Capture::ended(line(state).with_reason(reason)))
    };
    let Ok(answer) = serde_json::from_slice::<Answer>(bytes) else {
        return done(Status::Error, "not a post answer".to_owned());
    };
    // The answer's own code mirrors the HTTP status; trust the stricter.
    let code = answer.code.unwrap_or(200);
    if let Some((state, _)) = content_fetch::status_outcome(code) {
        return done(state, failure_reason(state, code));
    }
    let Some(post) = &answer.status else {
        return done(Status::Error, "no post in the answer".to_owned());
    };
    if let Some((state, reason)) = unavailable(post) {
        return done(state, reason);
    }
    let posts = if thread {
        authors_posts(post, answer.thread.as_deref().unwrap_or_default())
    } else {
        vec![post]
    };
    let neutral: Vec<xpost_page::Post> = posts.iter().map(|post| neutral(post)).collect();
    let markdown = xpost_page::markdown(&neutral);
    let chars = extract::plain_chars(&markdown);
    let has_text = neutral
        .iter()
        .any(|post| !xpost_page::text_of(post).is_empty());
    let mut line = line(if has_text { Status::Ok } else { Status::Thin });
    if !has_text {
        line.reason = Some("no text in the post".to_owned());
    }
    line.final_url.clone_from(&post.url);
    line.lang.clone_from(&post.lang);
    line.extractor = Some(EXTRACTOR.to_owned());
    line.extractor_version = Some(API_VERSION.to_owned());
    let page = Page {
        title: Some(xpost_page::title(&neutral)),
        extractor: format!("{EXTRACTOR} {API_VERSION}"),
        completeness: if has_text {
            Completeness::Full
        } else {
            Completeness::Thin
        },
        chars,
        captions: None,
        markdown,
    };
    let capture = Capture {
        line,
        page: Some(page),
        images: posts
            .iter()
            .find_map(|post| images(post))
            .map_or(Found::TextOnly, |url| {
                Found::Candidates(vec![Candidate {
                    url,
                    source: Source::XMedia,
                }])
            }),
    };
    Thread {
        capture,
        posts: posts
            .iter()
            .filter(|post| !post.id.is_empty())
            .map(|post| post.id.clone())
            .collect(),
        links: posts.iter().flat_map(|post| links(post)).collect(),
    }
}

/// Where `post`'s links lead: each `url` facet's target, else its card's
/// address.
fn links(post: &Post) -> Vec<String> {
    let facets: Vec<String> = post
        .raw_text
        .iter()
        .flat_map(|raw| &raw.facets)
        .filter(|facet| facet.kind == "url")
        .filter_map(|facet| facet.replacement.clone())
        .collect();
    if facets.is_empty() {
        post.card
            .iter()
            .filter_map(|card| card.url.clone())
            .collect()
    } else {
        facets
    }
}

/// The posts of `focal`'s author in `thread`, in its order, a deleted one
/// kept in its place; `focal` alone when the thread does not hold it.
fn authors_posts<'a>(focal: &'a Post, thread: &'a [Post]) -> Vec<&'a Post> {
    let author = |post: &Post| {
        post.author
            .as_ref()
            .map(|author| author.screen_name.to_ascii_lowercase())
    };
    let theirs = author(focal);
    let kept: Vec<&Post> = thread
        .iter()
        .filter(|post| post.kind == "tombstone" || author(post) == theirs)
        .collect();
    if kept.iter().any(|post| post.id == focal.id) {
        kept
    } else {
        vec![focal]
    }
}

/// `post` as the shared renderer draws it.
fn neutral(post: &Post) -> xpost_page::Post {
    xpost_page::Post {
        author: post.author.as_ref().map(|author| xpost_page::Author {
            name: author.name.clone(),
            handle: author.screen_name.clone(),
        }),
        at: post
            .created_timestamp
            .as_ref()
            .and_then(serde_json::Number::as_i64)
            .and_then(|seconds| Timestamp::from_second(seconds).ok()),
        text: post.text.clone(),
        article: post.article.as_ref().map(|article| xpost_page::Article {
            title: article.title.clone(),
            parts: article_parts(article).collect(),
        }),
        alts: alt_texts(post)
            .map(|(kind, alt)| (kind, alt.to_owned()))
            .collect(),
        quote: post.quote.as_deref().map(|quote| Box::new(neutral(quote))),
        unavailable: unavailable(post).map(|(_, reason)| reason),
    }
}

/// The image a post shows: its first photo, else its first video's
/// thumbnail, else its article's cover or first image, else the same of
/// the post it quotes. A post of text alone has none.
fn images(post: &Post) -> Option<String> {
    if unavailable(post).is_some() {
        return None;
    }
    let media = post.media.as_ref();
    let photo = media.and_then(|m| m.photos.iter().find_map(|p| p.url.clone()));
    let thumbnail = || media.and_then(|m| m.videos.iter().find_map(|v| v.thumbnail_url.clone()));
    let article = || {
        let article = post.article.as_ref()?;
        article
            .cover_media
            .iter()
            .chain(&article.media_entities)
            .find_map(|medium| {
                let info = &medium.media_info;
                info.original_img_url
                    .clone()
                    .or_else(|| info.media_url_https.clone())
            })
    };
    photo
        .or_else(thumbnail)
        .or_else(article)
        .or_else(|| post.quote.as_deref().and_then(images))
}

/// Why a post cannot be read: a tombstone's reason, or a protected
/// account. `None` for a readable post.
fn unavailable(post: &Post) -> Option<(Status, String)> {
    if post.kind == "tombstone" {
        let reason = post.reason.as_deref().unwrap_or("unavailable");
        let state = match reason {
            "private" => Status::BehindLogin,
            "blocked" => Status::Blocked,
            _ => Status::NotFound,
        };
        return Some((state, format!("post {reason}")));
    }
    post.author
        .as_ref()
        .is_some_and(|author| author.protected)
        .then(|| (Status::BehindLogin, "protected account".to_owned()))
}

/// The rendered article parts, shared by completeness and the saved body.
fn article_parts(article: &Article) -> impl Iterator<Item = String> + '_ {
    let media = &article.media_entities;
    article
        .content
        .iter()
        .flat_map(move |content| {
            content
                .blocks
                .iter()
                .map(move |part| block(part, content, media))
        })
        .filter(|part| !part.trim().is_empty())
}

/// Each image description, with what it describes.
fn alt_texts(post: &Post) -> impl Iterator<Item = (&'static str, &str)> {
    post.media
        .iter()
        .flat_map(|media| media.photos.iter())
        .filter_map(|medium| {
            let alt = medium.alt_text.as_deref()?.trim();
            let kind = match medium.kind.as_str() {
                "gif" => "GIF",
                _ => "Image",
            };
            (!alt.is_empty()).then_some((kind, alt))
        })
}

/// An article paragraph as markdown, by its editor block type.
fn block(block: &Block, content: &ArticleContent, media: &[ArticleMedium]) -> String {
    if block.kind == "atomic" {
        return block
            .entities
            .iter()
            .filter_map(|range| {
                let key = range.key.to_string();
                let value = &content
                    .entities
                    .iter()
                    .find(|entity| entity.key == key)?
                    .value;
                match value.kind.as_str() {
                    // The API supplies Markdown, including the snippet's
                    // fences and language. Preserve the source and indent.
                    "MARKDOWN" => Some(value.data.markdown.trim().to_owned()),
                    "TWEET" => {
                        let id = &value.data.tweet_id;
                        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
                            .then(|| format!("[Embedded post](https://x.com/i/status/{id})"))
                    }
                    "MEDIA" => Some(
                        value
                            .data
                            .media_items
                            .iter()
                            .map(|item| placeholder(&item.media_id, media))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    ),
                    _ => None,
                }
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    let text = block.text.trim();
    if text.is_empty() {
        return String::new();
    }
    let prefix = match block.kind.as_str() {
        "header-one" => "### ",
        "header-two" => "#### ",
        "header-three" => "##### ",
        "unordered-list-item" => "- ",
        "ordered-list-item" => "1. ",
        "blockquote" => "> ",
        _ => "",
    };
    if block.kind == "code-block" {
        format!("```\n{}\n```", block.text)
    } else {
        format!("{prefix}{text}")
    }
}

/// `[Image: description]`, or `[Image]` without one, where an article
/// shows an image; `[Video]` for a video or GIF. An id the article does
/// not list is an image. The description is one line, and its brackets
/// and backslashes are escaped so it cannot end the placeholder early.
fn placeholder(id: &str, media: &[ArticleMedium]) -> String {
    let info = media
        .iter()
        .find(|medium| medium.media_id == id)
        .map(|medium| &medium.media_info);
    let kind = match info.map(|info| info.kind.as_str()) {
        Some("ApiVideo" | "ApiGif") => "Video",
        _ => "Image",
    };
    let alt = info
        .and_then(|info| info.ext_alt_text.as_deref())
        .map(head::collapse)
        .unwrap_or_default();
    if alt.is_empty() {
        return format!("[{kind}]");
    }
    let alt = alt
        .replace('\\', r"\\")
        .replace('[', r"\[")
        .replace(']', r"\]");
    format!("[{kind}: {alt}]")
}

#[cfg(test)]
#[path = "xpost_tests.rs"]
mod tests;
