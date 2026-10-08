//! The X post route: a post on `x.com` or `twitter.com`, read through the
//! public X post API (`api.fxtwitter.com`, version 2) and kept as markdown.
//!
//! slice: content
//! why: An X post page is an app shell to plain HTTP, so the generic route
//!      keeps nothing of it, while the public post API answers with the
//!      post itself. It is the one third party service content capture
//!      uses, so it is asked as little as possible: only the post's number,
//!      never the page's address, cookieless and one request a second.
//!      What is kept is what a reader of the post sees as text: the post,
//!      the post it quotes, an article's body with a placeholder where it
//!      shows an image or video, who wrote it and when, and what its
//!      images say in their descriptions. One post only: a thread
//!      is not unrolled. A deleted post is final, a private one waits for a
//!      signed in browser, and a busy API is retried as any site is.

use jiff::Timestamp;
use serde::Deserialize;
use url::Url;

use crate::content_fetch::{self, Capture, Passing};
use crate::content_store::{Completeness, Line, Page, Status, Tier};
use crate::extract;
use crate::fetch::Fetcher;
use crate::head;
use crate::image_pick::{Candidate, Found, Source};

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
    format!("https://{API_HOST}/{API_VERSION}/status/{id}")
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
    content_fetch::retrying(fetcher, || once(fetcher, raw, id))
}

fn once(fetcher: &Fetcher, raw: &str, id: &str) -> Result<Capture, Passing> {
    let mut response = match fetcher.get(&api_url(id), ACCEPT_JSON) {
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
    Ok(read(&bytes, |state| line(state, None)))
}

/// What a final or passing HTTP status says about a post.
fn failure_reason(state: Status, status: u16) -> String {
    match state {
        Status::BehindLogin => format!("private post (HTTP {status})"),
        Status::NotFound => format!("post not found (HTTP {status})"),
        _ => format!("HTTP {status}"),
    }
}

/// The API's answer: the post, or a tombstone in its place.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Answer {
    code: Option<u16>,
    status: Option<Post>,
}

/// A post as the API gives it; only what is kept is read. A tombstone has
/// `type` `tombstone` and a `reason`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Post {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    text: String,
    created_timestamp: Option<serde_json::Number>,
    lang: Option<String>,
    author: Option<Author>,
    quote: Option<Box<Post>>,
    article: Option<Article>,
    media: Option<Media>,
    /// Why a tombstone stands in for the post.
    reason: Option<String>,
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
    let done = |state: Status, reason: String| Capture::ended(line(state).with_reason(reason));
    let Ok(answer) = serde_json::from_slice::<Answer>(bytes) else {
        return done(Status::Error, "not a post answer".to_owned());
    };
    // The answer's own code mirrors the HTTP status; trust the stricter.
    let code = answer.code.unwrap_or(200);
    if let Some((state, _)) = content_fetch::status_outcome(code) {
        return done(state, failure_reason(state, code));
    }
    let Some(post) = answer.status else {
        return done(Status::Error, "no post in the answer".to_owned());
    };
    if let Some((state, reason)) = unavailable(&post) {
        return done(state, reason);
    }
    let markdown = markdown(&post);
    let chars = extract::plain_chars(&markdown);
    let has_text = !text_of(&post).is_empty();
    let mut line = line(if has_text { Status::Ok } else { Status::Thin });
    if !has_text {
        line.reason = Some("no text in the post".to_owned());
    }
    line.final_url.clone_from(&post.url);
    line.lang.clone_from(&post.lang);
    line.extractor = Some(EXTRACTOR.to_owned());
    line.extractor_version = Some(API_VERSION.to_owned());
    let page = Page {
        title: Some(title(&post)),
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
    Capture {
        line,
        page: Some(page),
        images: images(&post).map_or(Found::TextOnly, |url| {
            Found::Candidates(vec![Candidate {
                url,
                source: Source::XMedia,
            }])
        }),
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

/// The article's title, or who wrote the post.
fn title(post: &Post) -> String {
    match &post.article {
        Some(article) if !article.title.trim().is_empty() => article.title.trim().to_owned(),
        _ => format!("{} on X", byline_name(post)),
    }
}

fn byline_name(post: &Post) -> String {
    match &post.author {
        Some(author) if !author.name.is_empty() => {
            format!("{} (@{})", author.name, author.screen_name)
        }
        Some(author) => format!("@{}", author.screen_name),
        None => "Someone".to_owned(),
    }
}

/// `Name (@handle), 2026-10-07T09:00:00Z`.
fn byline(post: &Post) -> String {
    let when = post
        .created_timestamp
        .as_ref()
        .and_then(serde_json::Number::as_i64)
        .and_then(|seconds| Timestamp::from_second(seconds).ok());
    match when {
        Some(when) => format!("{}, {when}", byline_name(post)),
        None => byline_name(post),
    }
}

/// Every piece of text a post holds, quoted post included, joined; empty
/// when it holds none.
fn text_of(post: &Post) -> String {
    if unavailable(post).is_some() {
        return String::new();
    }
    let mut parts = vec![post.text.trim().to_owned()];
    if let Some(article) = &post.article {
        parts.push(article.title.trim().to_owned());
        parts.extend(article_parts(article));
    }
    parts.extend(alt_texts(post).map(|(_, alt)| alt.to_owned()));
    if let Some(quote) = &post.quote {
        parts.push(text_of(quote));
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n")
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

/// `# Title`, the byline, the post's text, its article and its image
/// descriptions, then the quoted post as a block quote. Ends in one
/// newline.
fn markdown(post: &Post) -> String {
    let mut text = format!("# {}\n\n{}", title(post), body(post, false));
    if let Some(quote) = &post.quote {
        let quoted = match unavailable(quote) {
            Some((_, reason)) => format!("Quoted {reason}."),
            None => format!("Quoting {}", body(quote, true)),
        };
        text.push_str("\n\n");
        for line in quoted.trim_end().lines() {
            text.push_str(if line.is_empty() { ">" } else { "> " });
            text.push_str(line);
            text.push('\n');
        }
    }
    format!("{}\n", text.trim_end())
}

/// A post without its quote: the byline, then each part a paragraph. A
/// quoted article's title is a heading; the page's own is its title. The
/// text is trimmed at its edges only: after a blank line, a leading indent
/// of four spaces would make it a code block.
fn body(post: &Post, quoted: bool) -> String {
    let mut parts = vec![byline(post)];
    if !post.text.trim().is_empty() {
        parts.push(post.text.trim().to_owned());
    }
    if let Some(article) = &post.article {
        if quoted && !article.title.trim().is_empty() {
            parts.push(format!("## {}", article.title.trim()));
        }
        parts.extend(article_parts(article));
    }
    parts.extend(alt_texts(post).map(|(kind, alt)| format!("{kind}: {alt}")));
    parts.join("\n\n")
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
