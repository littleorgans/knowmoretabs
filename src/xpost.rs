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
//!      the post it quotes, an article's body, who wrote it and when, and
//!      what its images say in their descriptions. One post only: a thread
//!      is not unrolled. A deleted post is final, a private one waits for a
//!      signed in browser, and a busy API is retried as any site is.

use jiff::Timestamp;
use serde::Deserialize;
use url::Url;

use crate::content_fetch::{self, Capture, Passing};
use crate::content_store::{Completeness, Line, Page, Status, Tier};
use crate::extract;
use crate::fetch::Fetcher;

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

/// Asks the API for post `id`, recorded for library page `raw`, retrying
/// passing failures as the web route does. Never fails: a failure is a
/// capture too.
pub fn capture(fetcher: &Fetcher, raw: &str, id: &str) -> Capture {
    content_fetch::retrying(|| once(fetcher, raw, id))
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
        let mut line = content_fetch::public_line(raw, Tier::X, state);
        line.reason = reason;
        line.http_status = Some(status);
        line
    };
    if let Some((state, passing)) = content_fetch::status_outcome(status) {
        let failed = line(state, Some(failure_reason(state, status)));
        if passing {
            return Err(content_fetch::passing(&response, failed));
        }
        return Ok(Capture {
            line: failed,
            page: None,
        });
    }
    let mime = response.mime();
    if !mime.is_empty() && !mime.contains("json") {
        return Ok(Capture {
            line: line(Status::Error, Some(format!("not JSON ({mime})"))),
            page: None,
        });
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
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ArticleContent {
    blocks: Vec<Block>,
}

/// One paragraph of an article, in the editor's own block types.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Media {
    photos: Vec<Medium>,
    videos: Vec<Medium>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Medium {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "altText")]
    alt_text: Option<String>,
}

/// The line and text for the body of a successful answer; `line` makes
/// the response's line for a status.
fn read(bytes: &[u8], line: impl Fn(Status) -> Line) -> Capture {
    let done = |state: Status, reason: String| Capture {
        line: line(state).with_reason(reason),
        page: None,
    };
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
        markdown,
    };
    Capture {
        line,
        page: Some(page),
    }
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
        parts.extend(blocks(article).map(|block| block.text.trim().to_owned()));
    }
    parts.extend(alt_texts(post).map(|(_, alt)| alt.to_owned()));
    if let Some(quote) = &post.quote {
        parts.push(text_of(quote));
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n")
}

fn blocks(article: &Article) -> impl Iterator<Item = &Block> {
    article
        .content
        .iter()
        .flat_map(|content| content.blocks.iter())
        .filter(|block| !block.text.trim().is_empty())
}

/// Each image or video description, with what it describes.
fn alt_texts(post: &Post) -> impl Iterator<Item = (&'static str, &str)> {
    post.media
        .iter()
        .flat_map(|media| media.photos.iter().chain(media.videos.iter()))
        .filter_map(|medium| {
            let alt = medium.alt_text.as_deref()?.trim();
            let kind = match medium.kind.as_str() {
                "video" => "Video",
                "gif" => "GIF",
                _ => "Image",
            };
            (!alt.is_empty()).then_some((kind, alt))
        })
}

/// `# Title`, the byline, the post's text, its article and its image
/// descriptions, then the quoted post as a block quote. Ends with a
/// newline without stripping the post's trailing whitespace.
fn markdown(post: &Post) -> String {
    let mut text = format!("# {}\n\n{}", title(post), body(post, false));
    if let Some(quote) = &post.quote {
        let quoted = match unavailable(quote) {
            Some((_, reason)) => format!("Quoted {reason}."),
            None => format!("Quoting {}", body(quote, true)),
        };
        text.push_str("\n\n");
        for line in quoted.lines() {
            text.push_str(if line.is_empty() { ">" } else { "> " });
            text.push_str(line);
            text.push('\n');
        }
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// A post without its quote: the byline, then each part a paragraph. A
/// quoted article's title is a heading; the page's own is its title.
fn body(post: &Post, quoted: bool) -> String {
    let mut parts = vec![byline(post)];
    if !post.text.trim().is_empty() {
        parts.push(post.text.clone());
    }
    if let Some(article) = &post.article {
        if quoted && !article.title.trim().is_empty() {
            parts.push(format!("## {}", article.title.trim()));
        }
        parts.extend(blocks(article).map(block));
    }
    parts.extend(alt_texts(post).map(|(kind, alt)| format!("{kind}: {alt}")));
    parts.join("\n\n")
}

/// An article paragraph as markdown, by its editor block type.
fn block(block: &Block) -> String {
    let text = block.text.trim();
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn a_post_id_is_the_number_after_status() {
        assert_eq!(
            post_id(&url("https://x.com/someone/status/1234567890")).as_deref(),
            Some("1234567890")
        );
        assert_eq!(
            post_id(&url(
                "https://twitter.com/someone/status/12/photo/1?s=20#top"
            ))
            .as_deref(),
            Some("12")
        );
        for raw in [
            "https://x.com/someone",
            "https://x.com/someone/status/",
            "https://x.com/someone/status/not-a-number",
            "https://x.com/someone/likes/123",
            "https://x.com/",
        ] {
            assert_eq!(post_id(&url(raw)), None, "{raw}");
        }
        assert_eq!(api_url("12"), "https://api.fxtwitter.com/2/status/12");
    }

    /// A synthetic post in the API's shape.
    fn post(text: &str) -> serde_json::Value {
        json!({
            "type": "status", "id": "100", "url": "https://x.com/example/status/100",
            "text": text, "created_timestamp": 1_791_363_600, "lang": "en",
            "author": {"type": "profile", "name": "Example Person", "screen_name": "example",
                "protected": false},
            "media": {}, "likes": 1, "replying_to": null, "community_note": null,
        })
    }

    fn answer(status: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"code": 200, "status": status, "thread": null})).unwrap()
    }

    fn line(state: Status) -> Line {
        content_fetch::public_line("https://x.com/example/status/100", Tier::X, state)
    }

    fn read_value(status: &serde_json::Value) -> Capture {
        read(&answer(status), line)
    }

    #[test]
    fn a_post_keeps_its_text_author_date_and_image_descriptions() {
        let mut status = post("First line of the post.\n\nSecond paragraph.");
        status["media"] = json!({"photos": [
            {"type": "photo", "url": "https://img.test/1.jpg", "width": 1, "height": 1,
                "altText": "A chart of tides"},
            {"type": "photo", "url": "https://img.test/2.jpg", "width": 1, "height": 1},
        ]});
        let Capture { line, page } = read_value(&status);
        let page = page.unwrap();
        assert_eq!((line.status, line.reason), (Status::Ok, None));
        assert_eq!(line.tier, Some(Tier::X));
        assert_eq!(line.access, Some(crate::content_store::Access::Public));
        assert_eq!(
            line.final_url.as_deref(),
            Some("https://x.com/example/status/100")
        );
        assert_eq!(line.lang.as_deref(), Some("en"));
        assert_eq!(line.extractor.as_deref(), Some("fxtwitter"));
        assert_eq!(line.extractor_version.as_deref(), Some("2"));
        assert_eq!(page.extractor, "fxtwitter 2");
        assert_eq!(page.completeness, Completeness::Full);
        assert_eq!(
            page.title.as_deref(),
            Some("Example Person (@example) on X")
        );
        assert_eq!(
            page.markdown,
            "# Example Person (@example) on X\n\n\
             Example Person (@example), 2026-10-07T09:00:00Z\n\n\
             First line of the post.\n\nSecond paragraph.\n\n\
             Image: A chart of tides\n"
        );
        assert_eq!(page.chars, extract::plain_chars(&page.markdown));
    }

    #[test]
    fn post_text_keeps_its_original_whitespace() {
        for text in ["  Indented text.\nTrailing spaces.  ", "First line.\n\n\n"] {
            let page = read_value(&post(text)).page.unwrap();
            let mut expected = format!(
                "# Example Person (@example) on X\n\n\
                 Example Person (@example), 2026-10-07T09:00:00Z\n\n{text}"
            );
            if !expected.ends_with('\n') {
                expected.push('\n');
            }
            assert_eq!(page.markdown, expected);
        }
        let mut status = post("Public commentary.");
        status["quote"] = post("  Quoted line.  \n\n\n");
        assert!(
            read_value(&status)
                .page
                .unwrap()
                .markdown
                .ends_with(">   Quoted line.  \n>\n>\n")
        );
        status["article"] = json!({"content": {"blocks": [
            {"type": "code-block", "text": "    first()\n    second()"}
        ]}});
        assert!(
            read_value(&status)
                .page
                .unwrap()
                .markdown
                .contains("```\n    first()\n    second()\n```")
        );
    }

    #[test]
    fn a_quoted_post_follows_as_a_block_quote() {
        let mut status = post("Worth reading.");
        let mut quoted = post("The quoted words.\n\nAnd more.");
        quoted["author"] = json!({"name": "Other", "screen_name": "other", "protected": false});
        quoted["created_timestamp"] = json!(null);
        status["quote"] = quoted;
        let page = read_value(&status).page.unwrap();
        assert_eq!(
            page.markdown,
            "# Example Person (@example) on X\n\n\
             Example Person (@example), 2026-10-07T09:00:00Z\n\n\
             Worth reading.\n\n\
             > Quoting Other (@other)\n>\n> The quoted words.\n>\n> And more.\n"
        );

        status["quote"] = json!({"type": "tombstone", "provider": "twitter",
            "reason": "deleted", "message": "gone"});
        let page = read_value(&status).page.unwrap();
        assert!(
            page.markdown
                .ends_with("Worth reading.\n\n> Quoted post deleted.\n")
        );
    }

    #[test]
    fn unavailable_quotes_keep_no_text_in_the_store() {
        use crate::content_store::{self, Store};

        let mut protected = post("Restricted quote text.");
        protected["author"]["protected"] = json!(true);
        protected["article"] = json!({"title": "Restricted article", "content": {
            "blocks": [{"type": "unstyled", "text": "Restricted article text."}]
        }});
        protected["media"] = json!({"photos": [{"altText": "Restricted image."}]});
        for quote in [
            protected,
            json!({"type": "tombstone", "reason": "private", "text": "Restricted text."}),
        ] {
            let mut status = post("Public commentary.");
            status["quote"] = quote;
            let Capture { line, page } = read_value(&status);
            assert_eq!(line.status, Status::Ok);
            let root = tempfile::tempdir().unwrap();
            let mut store = Store::open(root.path()).unwrap();
            let line = store.record(line, page.as_ref()).unwrap();
            let path = content_store::dir(root.path()).join(content_store::file_name(&line.url));
            let saved = std::fs::read_to_string(path).unwrap();
            let (front, body) = content_store::parse(&saved).unwrap();
            assert_eq!(front["tier"], "x");
            assert_eq!(front["access"], "public");
            assert!(body.contains("Public commentary."));
            assert!(
                !body.contains("Restricted"),
                "unavailable quote text retained"
            );
            assert_eq!(
                line.content_sha256.as_deref(),
                Some(content_store::sha256_hex(body.as_bytes()).as_str())
            );
        }
    }

    #[test]
    fn a_post_with_only_a_protected_quote_is_thin() {
        let mut status = post("");
        let mut quote = post("Restricted quote text.");
        quote["author"]["protected"] = json!(true);
        status["quote"] = quote;
        let Capture { line, page } = read_value(&status);
        assert_eq!(line.status, Status::Thin);
        assert_eq!(page.unwrap().completeness, Completeness::Thin);
    }

    #[test]
    fn an_article_keeps_its_title_and_body() {
        let mut status = post("https://x.com/i/article/9");
        status["article"] = json!({
            "id": "9", "title": "On Tides", "preview_text": "Tides are",
            "content": {"blocks": [
                {"key": "a", "type": "unstyled", "text": "Tides are long waves.", "data": {},
                    "entityRanges": [], "inlineStyleRanges": []},
                {"key": "b", "type": "header-two", "text": "Causes"},
                {"key": "c", "type": "unordered-list-item", "text": "The moon"},
                {"key": "d", "type": "atomic", "text": " "},
                {"key": "e", "type": "code-block", "text": "tide = moon + sun"},
            ], "entityMap": []},
        });
        let Capture { line, page } = read_value(&status);
        let page = page.unwrap();
        assert_eq!(line.status, Status::Ok);
        assert_eq!(page.title.as_deref(), Some("On Tides"));
        assert_eq!(
            page.markdown,
            "# On Tides\n\n\
             Example Person (@example), 2026-10-07T09:00:00Z\n\n\
             https://x.com/i/article/9\n\n\
             Tides are long waves.\n\n#### Causes\n\n- The moon\n\n\
             ```\ntide = moon + sun\n```\n"
        );
    }

    #[test]
    fn a_post_without_text_is_thin() {
        let mut status = post("  ");
        status["media"] = json!({"videos": [{"type": "video", "url": "https://v.test/1.mp4"}]});
        let Capture { line, page } = read_value(&status);
        assert_eq!(
            (line.status, line.reason.as_deref()),
            (Status::Thin, Some("no text in the post"))
        );
        assert_eq!(page.unwrap().completeness, Completeness::Thin);
    }

    #[test]
    fn deleted_private_and_protected_posts_keep_no_text() {
        let tombstone = |reason: &str| {
            json!({"type": "tombstone", "provider": "twitter", "reason": reason,
                "message": "unavailable"})
        };
        let mut protected = post("Only followers see this.");
        protected["author"]["protected"] = json!(true);
        for (status, expected, reason) in [
            (tombstone("deleted"), Status::NotFound, "post deleted"),
            (tombstone("suspended"), Status::NotFound, "post suspended"),
            (tombstone("private"), Status::BehindLogin, "post private"),
            (tombstone("blocked"), Status::Blocked, "post blocked"),
            (protected, Status::BehindLogin, "protected account"),
        ] {
            let Capture { line, page } = read_value(&status);
            assert_eq!(
                (line.status, line.reason.as_deref()),
                (expected, Some(reason))
            );
            assert_eq!(line.tier, Some(Tier::X));
            assert!(page.is_none(), "{reason}");
        }
    }

    #[test]
    fn an_answer_code_or_a_missing_post_is_not_a_post() {
        let body = |value: serde_json::Value| read(&serde_json::to_vec(&value).unwrap(), line).line;
        let not_found = body(json!({"code": 404, "message": "NOT_FOUND", "status": null}));
        assert_eq!(
            (not_found.status, not_found.reason.as_deref()),
            (Status::NotFound, Some("post not found (HTTP 404)"))
        );
        let private = body(json!({"code": 401, "message": "PRIVATE_TWEET"}));
        assert_eq!(
            (private.status, private.reason.as_deref()),
            (Status::BehindLogin, Some("private post (HTTP 401)"))
        );
        let empty = body(json!({"code": 200, "status": null}));
        assert_eq!(
            (empty.status, empty.reason.as_deref()),
            (Status::Error, Some("no post in the answer"))
        );
        let garbled = read(b"<html>not json</html>", line).line;
        assert_eq!(
            (garbled.status, garbled.reason.as_deref()),
            (Status::Error, Some("not a post answer"))
        );
    }

    #[test]
    fn api_statuses_map_to_the_vocabulary() {
        let reason = |code| {
            let (state, passing) = content_fetch::status_outcome(code).unwrap();
            (state, passing, failure_reason(state, code))
        };
        assert_eq!(
            reason(404),
            (
                Status::NotFound,
                false,
                "post not found (HTTP 404)".to_owned()
            )
        );
        assert_eq!(
            reason(401),
            (
                Status::BehindLogin,
                false,
                "private post (HTTP 401)".to_owned()
            )
        );
        assert_eq!(reason(429), (Status::Error, true, "HTTP 429".to_owned()));
        assert_eq!(reason(503), (Status::Error, true, "HTTP 503".to_owned()));
        assert_eq!(reason(500), (Status::Error, false, "HTTP 500".to_owned()));
    }
}
