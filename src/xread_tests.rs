//! Regression checks for `xread`.
//!
//! slice: x-bookmarks
//! why: Synthetic page reads exercise every check and the capture they make without a browser or a real post.

use serde_json::json;

use super::*;

const KEY: &str = "https://x.com/example_author/status/102";

fn post(id: &str, text: &str) -> Value {
    json!({"id": id, "handle": "example_author", "name": "Example Author",
        "at": "2026-01-01T10:00:00Z", "lang": "en", "text": text, "text_complete": true})
}

fn read(posts: &[Value]) -> Value {
    json!({"where": "status", "clicked": "102", "author": "example_author", "complete": true,
        "reasons": [], "posts": posts})
}

fn parsed(value: &Value) -> Result<Read, String> {
    parse(value, "102", "example_author")
}

#[test]
fn a_whole_thread_read_on_its_page_is_one_signed_in_capture_in_the_pages_order() {
    let mut second = post("102", "Second.\nWith a line break.");
    second["quote"] = json!({"url": "https://x.com/example_quote/status/900",
        "handle": "example_quote", "text": "Quoted words."});
    let read = parsed(&read(&[post("101", "First."), second])).unwrap();
    assert!(read.complete);
    assert_eq!(read.ids(), ["101", "102"]);
    let Capture { line, page, images } = capture(&read, KEY, KEY);
    let page = page.unwrap();
    assert_eq!(
        (line.status, line.tier, line.access),
        (
            Status::Ok,
            Some(Tier::SignedIn),
            Some(crate::content_store::Access::SignedIn)
        )
    );
    assert_eq!(
        (line.extractor.as_deref(), line.extractor_version.as_deref()),
        (Some("x-page"), Some("1"))
    );
    assert_eq!(line.reason.as_deref(), Some("2 posts as shown"));
    assert_eq!(line.final_url.as_deref(), Some(KEY));
    assert_eq!(line.lang.as_deref(), Some("en"));
    assert_eq!(page.extractor, "x-page 1");
    assert_eq!(
        page.title.as_deref(),
        Some("Example Author (@example_author) on X")
    );
    assert_eq!(
        page.markdown,
        "# Example Author (@example_author) on X\n\n\
         Example Author (@example_author), 2026-01-01T10:00:00Z\n\nFirst.\n\n---\n\n\
         Example Author (@example_author), 2026-01-01T10:00:00Z\n\n\
         Second.\nWith a line break.\n\n\
         > Quoting @example_quote\n>\n> Quoted words.\n"
    );
    assert_eq!(page.chars, extract::plain_chars(&page.markdown));
    assert_eq!(images, Found::TextOnly);
}

#[test]
fn a_read_is_complete_only_on_the_posts_page_with_nothing_cut_or_folded() {
    let complete = |value: &Value| parsed(value).unwrap().complete;
    assert!(complete(&read(&[post("102", "Text.")])));
    let mut timeline = read(&[post("102", "Text.")]);
    timeline["where"] = json!("timeline");
    assert!(!complete(&timeline));
    let mut folded = read(&[post("102", "Text.")]);
    folded["complete"] = json!(false);
    folded["reasons"] = json!(["unexpanded_replies"]);
    assert!(!complete(&folded));
    let mut cut = post("102", "A preview…");
    cut["text_complete"] = json!(false);
    assert!(!complete(&read(&[cut])));
    let mut said = read(&[post("102", "Text.")]);
    said["reasons"] = json!(["unresolved_link"]);
    assert!(!complete(&said), "a reason given is not complete");
    let mut unknown = read(&[post("102", "Text.")]);
    unknown["reasons"] = json!(["a_reason_from_later"]);
    assert!(complete(&unknown), "an unknown reason is dropped");

    let read = parsed(&timeline).unwrap();
    assert_eq!(
        capture(&read, KEY, KEY).line.reason.as_deref(),
        Some("1 post as shown; thread may continue")
    );
}

#[test]
fn a_post_of_images_alone_is_thin_with_its_descriptions_and_x_images_only() {
    let mut media = post("102", "");
    media["alts"] = json!(["A synthetic diagram", " "]);
    media["media"] = json!([
        "https://example.org/elsewhere.jpg",
        "https://pbs.twimg.com/media/EXAMPLE.jpg",
        "http://pbs.twimg.com/media/PLAIN.jpg"
    ]);
    let read = parsed(&read(&[media])).unwrap();
    assert_eq!(
        read.posts[0].media,
        ["https://pbs.twimg.com/media/EXAMPLE.jpg"]
    );
    let Capture { line, page, images } = capture(&read, KEY, KEY);
    let page = page.unwrap();
    assert_eq!(line.status, Status::Thin);
    assert_eq!(page.completeness, Completeness::Thin);
    assert!(
        page.markdown
            .ends_with("2026-01-01T10:00:00Z\n\nImage: A synthetic diagram\n"),
        "{}",
        page.markdown
    );
    assert_eq!(
        images,
        Found::Candidates(vec![Candidate {
            url: "https://pbs.twimg.com/media/EXAMPLE.jpg".to_owned(),
            source: Source::XMedia,
        }])
    );
}

#[test]
fn a_read_with_no_post_passes_and_names_none() {
    let read = parsed(&read(&[])).unwrap();
    assert_eq!(read.posts, []);
}

#[test]
fn an_unresolved_link_is_incomplete_even_when_the_reader_omits_its_reason() {
    let mut one = post("102", "Read the linked article.");
    one["links"] = json!([{"href": "https://t.co/example", "label": "example.org/full…"}]);
    let mut sent = read(&[one]);
    assert!(!parsed(&sent).unwrap().complete);
    sent["posts"][0]["links"][0]["target"] = json!("https://example.org/full");
    assert!(parsed(&sent).unwrap().complete);
}

#[test]
fn a_read_out_of_bounds_or_of_another_post_is_refused_whole() {
    let many: Vec<Value> = (1..=51).map(|n| post(&n.to_string(), "x")).collect();
    let mut long = post("102", &"x".repeat(25_001));
    let at_most = post("102", &"é".repeat(25_000));
    assert!(
        parsed(&read(&[at_most])).is_ok(),
        "the bound counts characters"
    );
    let mut cases = vec![read(&many), read(&[long.clone()])];
    long["text"] = json!("ok");
    let with = |key: &str, value: Value| {
        let mut one = post("102", "Text.");
        one[key] = value;
        read(&[one])
    };
    cases.extend([
        with("alts", json!(vec!["a"; 9])),
        with("media", json!(vec!["https://pbs.twimg.com/media/A.jpg"; 5])),
        with(
            "links",
            json!(vec![json!({"href": "https://t.co/EXAMPLE"}); 11]),
        ),
        with(
            "links",
            json!([{"href": "https://t.co/EXAMPLE", "target": "javascript:alert(1)"}]),
        ),
        with("id", json!("12345678901234567890")),
        with("id", json!("10a")),
        with("handle", json!("example_reader")),
        with("handle", json!("bad-handle")),
        with("at", json!("yesterday")),
        with("lang", json!("en; drop")),
        with("name", json!("n".repeat(201))),
        with("text_complete", json!("yes")),
        with(
            "quote",
            json!({"url": "not a url", "handle": "example_quote"}),
        ),
        read(&[post("102", "Twice."), post("102", "Twice.")]),
        read(&[post("101", "Not the clicked post.")]),
    ]);
    let mut other = read(&[post("102", "Text.")]);
    other["clicked"] = json!("103");
    cases.push(other);
    let mut author = read(&[post("102", "Text.")]);
    author["author"] = json!("example_reader");
    cases.push(author);
    let mut place = read(&[post("102", "Text.")]);
    place["where"] = json!("profile");
    cases.push(place);
    cases.extend([json!(null), json!("read"), json!({"posts": []})]);
    for case in cases {
        let shown = case.to_string();
        assert!(parsed(&case).is_err(), "{}", &shown[..shown.len().min(200)]);
    }
    let mut folded = read(&[post("102", "Text.")]);
    folded["author"] = json!("Example_Author");
    assert!(parsed(&folded).is_ok(), "handles compare case folded");
}
