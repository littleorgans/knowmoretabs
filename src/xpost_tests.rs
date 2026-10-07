//! Regression checks for `xpost`.
//!
//! slice: content
//! why: Synthetic posts and article fixtures exercise image selection and text capture without requesting a real post.

use serde_json::json;

use super::*;

const ARTICLE_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/content/x-article.json");

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
    content_fetch::line("https://x.com/example/status/100", Tier::X, state)
}

fn read_value(status: &serde_json::Value) -> Capture {
    read(&answer(status), line)
}

#[test]
fn a_post_image_is_its_photo_else_its_video_thumbnail_else_its_quote() {
    let image = |status: &serde_json::Value| match read_value(status).images {
        Found::Candidates(found) => {
            assert!(found.iter().all(|c| c.source == Source::XMedia));
            found.into_iter().map(|c| c.url).collect::<Vec<_>>()
        }
        other => panic!("expected candidates, got {other:?}"),
    };
    let mut status = post("Two kinds of media.");
    status["media"] = json!({
        "photos": [{"type": "photo", "url": "https://img.test/photo.jpg"}],
        "videos": [{"type": "video", "url": "https://video.test/v.mp4",
            "thumbnail_url": "https://img.test/thumb.jpg"}],
    });
    assert_eq!(image(&status), ["https://img.test/photo.jpg"]);
    status["media"] = json!({"videos": [{"type": "video", "url": "https://video.test/v.mp4",
        "thumbnail_url": "https://img.test/thumb.jpg"}]});
    assert_eq!(image(&status), ["https://img.test/thumb.jpg"]);

    let mut quoting = post("Look at this.");
    let mut quoted = post("Quoted with a photo.");
    quoted["media"] = json!({"photos": [{"type": "photo", "url": "https://img.test/q.jpg"}]});
    quoting["quote"] = quoted;
    assert_eq!(image(&quoting), ["https://img.test/q.jpg"]);
    quoting["quote"] = json!({"type": "tombstone", "provider": "twitter",
        "reason": "deleted", "media": {"photos": [{"url": "https://img.test/gone.jpg"}]}});
    assert_eq!(read_value(&quoting).images, Found::TextOnly);

    let mut article = post("");
    article["article"] = json!({"title": "Long read",
        "media_entities": [{"media_id": "2", "media_info": {"__typename": "ApiImage",
            "original_img_url": "https://img.test/inline.jpg"}}],
        "cover_media": {"media_id": "1", "media_info": {"__typename": "ApiImage",
            "original_img_url": "https://img.test/cover.jpg"}}});
    assert_eq!(image(&article), ["https://img.test/cover.jpg"]);
    article["article"]["cover_media"] = json!(null);
    assert_eq!(image(&article), ["https://img.test/inline.jpg"]);

    assert_eq!(read_value(&post("Words only.")).images, Found::TextOnly);
    let mut protected = post("Hidden.");
    protected["author"]["protected"] = json!(true);
    protected["media"] = json!({"photos": [{"url": "https://img.test/private.jpg"}]});
    assert_eq!(
        read_value(&protected).images,
        Found::Unread,
        "a protected account's media is not kept"
    );
}

#[test]
fn a_post_keeps_its_text_author_date_and_image_descriptions() {
    let mut status = post("First line of the post.\n\nSecond paragraph.");
    status["media"] = json!({"photos": [
        {"type": "photo", "url": "https://img.test/1.jpg", "width": 1, "height": 1,
            "altText": "A chart of tides"},
        {"type": "photo", "url": "https://img.test/2.jpg", "width": 1, "height": 1},
    ]});
    let Capture { line, page, .. } = read_value(&status);
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
fn post_text_is_trimmed_at_its_edges_and_code_keeps_its_indent() {
    let page = read_value(&post("    Indented text.\n  Second line.  \n\n")).page;
    assert_eq!(
        page.unwrap().markdown,
        "# Example Person (@example) on X\n\n\
         Example Person (@example), 2026-10-07T09:00:00Z\n\n\
         Indented text.\n  Second line.\n"
    );
    let mut status = post("Public commentary.");
    status["quote"] = post("    Quoted line.  \n\n");
    status["article"] = json!({"content": {"blocks": [
        {"type": "code-block", "text": "    first()\n    second()"}
    ]}});
    let markdown = read_value(&status).page.unwrap().markdown;
    assert!(markdown.contains("```\n    first()\n    second()\n```"));
    assert!(markdown.ends_with(">\n> Quoted line.\n"), "{markdown}");
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
        let Capture { line, page, .. } = read_value(&status);
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
    let Capture { line, page, .. } = read_value(&status);
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
    let Capture { line, page, .. } = read_value(&status);
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
fn an_article_keeps_atomic_code_embeds_and_images_in_order() {
    use crate::content_store::{self, Store};

    let Capture { line, page, .. } = read(ARTICLE_FIXTURE, line);
    let page = page.unwrap();
    assert_eq!(line.status, Status::Ok);
    assert_eq!(page.completeness, Completeness::Full);
    assert_eq!(
        page.markdown,
        "# Synthetic snippets\n\n\
         Example Person (@example), 2026-10-07T09:00:00Z\n\n\
         Here is the implementation:\n\n\
         ```rust\nfn sample() {\n    let fence = \"```\";\n}\n```\n\n\
         Here is a plain snippet:\n\n```\n    sample()\n```\n\n\
         Here is the referenced post:\n\n\
         [Embedded post](https://x.com/i/status/101)\n\n\
         [Image]\n\n\
         End of the article.\n"
    );
    assert_eq!(page.chars, extract::plain_chars(&page.markdown));

    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let line = store.record(line, Some(&page)).unwrap();
    let path = content_store::dir(root.path()).join(content_store::file_name(&line.url));
    let saved = std::fs::read_to_string(path).unwrap();
    let (front, body) = content_store::parse(&saved).unwrap();
    assert_eq!(body, page.markdown);
    assert_eq!(front["completeness"], "full");
    assert_eq!(
        line.content_sha256.as_deref(),
        Some(content_store::sha256_hex(body.as_bytes()).as_str())
    );
}

/// An article whose one paragraph introduces media `7`, with the
/// media it lists; its saved markdown.
fn media_article(media_entities: &serde_json::Value) -> String {
    let mut status = post("");
    status["article"] = json!({
        "id": "9", "title": "Charts", "media_entities": media_entities,
        "content": {"blocks": [
            {"key": "a", "type": "unstyled", "text": "The chart:"},
            {"key": "b", "type": "atomic", "text": " ",
                "entityRanges": [{"key": 3, "offset": 0, "length": 1}]},
        ], "entityMap": [
            {"key": "3", "value": {"type": "MEDIA", "mutability": "Immutable",
                "data": {"entityKey": "c", "mediaItems": [
                    {"localMediaId": "c", "mediaCategory": "photo", "mediaId": "7"},
                ]}}},
        ]},
    });
    read_value(&status).page.unwrap().markdown
}

fn medium(id: &str, info: &serde_json::Value) -> serde_json::Value {
    json!({"id": id, "media_key": format!("3_{id}"), "media_id": id, "media_info": info})
}

const CHART_ARTICLE: &str = "# Charts\n\n\
     Example Person (@example), 2026-10-07T09:00:00Z\n\n\
     The chart:\n\n";

#[test]
fn an_article_image_is_a_placeholder_with_its_one_line_description() {
    let info = json!({"__typename": "ApiImage",
        "original_img_url": "https://example.com/7.jpg",
        "ext_alt_text": " Tides  by\nhour [high](https://example.com) \\ low "});
    assert_eq!(
        media_article(&json!([medium("7", &info)])),
        format!(
            "{CHART_ARTICLE}\
             [Image: Tides by hour \\[high\\](https://example.com) \\\\ low]\n"
        )
    );
}

#[test]
fn an_article_image_without_a_description_is_a_bare_placeholder() {
    let info = json!({"__typename": "ApiImage",
        "original_img_url": "https://example.com/7.jpg"});
    assert_eq!(
        media_article(&json!([medium("7", &info)])),
        format!("{CHART_ARTICLE}[Image]\n")
    );
    let blank = json!({"__typename": "ApiImage", "ext_alt_text": " \n "});
    assert_eq!(
        media_article(&json!([medium("7", &blank)])),
        format!("{CHART_ARTICLE}[Image]\n")
    );
}

#[test]
fn an_article_media_id_it_does_not_list_is_an_image() {
    let video = json!({"__typename": "ApiVideo", "type": "video",
        "ext_alt_text": "Not this one"});
    assert_eq!(
        media_article(&json!([medium("8", &video)])),
        format!("{CHART_ARTICLE}[Image]\n")
    );
    assert_eq!(
        media_article(&json!([])),
        format!("{CHART_ARTICLE}[Image]\n")
    );
}

#[test]
fn an_article_video_or_gif_is_a_video_placeholder() {
    let video = json!({"__typename": "ApiVideo", "type": "video",
        "ext_alt_text": "Waves at dusk"});
    assert_eq!(
        media_article(&json!([medium("7", &video)])),
        format!("{CHART_ARTICLE}[Video: Waves at dusk]\n")
    );
    let gif = json!({"__typename": "ApiGif", "type": "animated_gif", "ext_alt_text": null});
    assert_eq!(
        media_article(&json!([medium("7", &gif)])),
        format!("{CHART_ARTICLE}[Video]\n")
    );
}

#[test]
fn an_article_keeps_each_media_item_without_empty_atomic_paragraphs() {
    let expected = read(ARTICLE_FIXTURE, line)
        .page
        .unwrap()
        .markdown
        .replace("[Image]", "[Image]\n\n[Video]");
    let mut answer: serde_json::Value = serde_json::from_slice(ARTICLE_FIXTURE).unwrap();
    let article = &mut answer["status"]["article"];
    article["media_entities"]
        .as_array_mut()
        .unwrap()
        .push(medium(
            "7",
            &json!({"__typename": "ApiVideo", "ext_alt_text": null}),
        ));
    let content = &mut article["content"];
    let entities = content["entityMap"].as_array_mut().unwrap();
    let media = entities
        .iter_mut()
        .find(|entity| entity["key"] == "12")
        .unwrap();
    media["value"]["data"]["mediaItems"]
        .as_array_mut()
        .unwrap()
        .push(json!({"mediaId": "7"}));
    entities.extend([
        json!({"key": "13", "value": {"type": "MEDIA", "data": {"mediaItems": []}}}),
        json!({"key": "14", "value": {"type": "MARKDOWN", "data": {"markdown": " \n "}}}),
    ]);
    let block = content["blocks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|block| block["key"] == "media")
        .unwrap();
    block["entityRanges"] = json!([
        {"key": 13}, {"key": 14}, {"key": 12}, {"key": 13}, {"key": 14},
    ]);
    let Capture { line, page, .. } = read_value(&answer["status"]);
    let page = page.unwrap();
    assert_eq!(line.status, Status::Ok);
    assert_eq!(page.completeness, Completeness::Full);
    assert!(page.markdown.contains("[Image]\n\n[Video]"));
    assert_eq!(page.markdown, expected);
}

#[test]
fn a_post_without_text_is_thin() {
    let mut status = post("  ");
    status["media"] = json!({"videos": [{"type": "video", "url": "https://v.test/1.mp4"}]});
    let Capture { line, page, .. } = read_value(&status);
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
        let Capture { line, page, .. } = read_value(&status);
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
