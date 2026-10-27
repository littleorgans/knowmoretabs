//! A post bookmarked on X, saved through `serve` by the real binary: the
//! page, its click line, its text read from the owner's page or from a
//! stand-in X post API's thread, whichever is longer, and the thread's
//! later bookmarks joining the same page; the article the thread links to,
//! a page of its own tied to the post; the origins the routes accept and
//! refuse; the extension's health report and the `doctor` line it makes.

mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use common::add::article;
use common::serve::{A, HIDDEN, Reply, Server, archive, kept_name, page};
use common::site::{self, Site};
use common::{Fixture, stderr, stdout};
use serde_json::{Value, json};

const POST: &str = "https://x.com/example_author/status/100";
const PRIVATE: &str = "https://x.com/example_private/status/401";
/// Plain HTTP: the stand-in site speaks no TLS.
const ARTICLE: &str = "http://blog.example.test/post";

/// The extension's origin as `serve` pins it, read where it is declared.
fn extension_origin() -> String {
    let source = include_str!("../src/http.rs");
    let declared = source
        .lines()
        .find_map(|line| line.strip_prefix("pub(crate) const EXTENSION_ORIGIN: &str = \""))
        .expect("EXTENSION_ORIGIN in src/http.rs");
    declared.trim_end_matches("\";").to_owned()
}

/// A synthetic post in the X post API's shape.
fn answer() -> Vec<u8> {
    serde_json::to_vec(&json!({"code": 200, "thread": null, "status": {
        "type": "status", "id": "100", "url": POST,
        "text": "A synthetic post about keeping what you bookmark.",
        "created_timestamp": 1_791_363_600, "lang": "en",
        "author": {"type": "profile", "name": "Example Author", "screen_name": "example_author",
            "protected": false},
        "media": {}, "replying_to": null}}))
    .unwrap()
}

/// A post of the synthetic thread by `example_author`, numbered `id`.
fn thread_post(handle: &str, id: u64, text: &str) -> Value {
    json!({"type": "status", "id": id.to_string(),
        "url": format!("https://x.com/{handle}/status/{id}"),
        "text": text, "created_timestamp": 1_791_363_600 + id, "lang": "en",
        "author": {"type": "profile", "name": "Example Author", "screen_name": handle,
            "protected": false},
        "media": {}})
}

/// The thread's five posts by its author, numbers 200 to 204, and a reply
/// by someone else, as the thread route answers for any of them.
fn thread_answer(focal: u64) -> Vec<u8> {
    let mut thread: Vec<Value> = (200..=204)
        .map(|id| {
            thread_post(
                "example_author",
                id,
                &format!("Thread post {id}, the whole of it as the public service has it."),
            )
        })
        .collect();
    thread.insert(
        2,
        thread_post("example_reader", 250, "A reply by someone else."),
    );
    let status = thread_post("example_author", focal, "");
    serde_json::to_vec(&json!({"code": 200, "status": status, "thread": thread})).unwrap()
}

fn json_reply(body: Vec<u8>) -> site::Reply {
    site::Reply {
        status: 200,
        headers: vec![("Content-Type", "application/json".into())],
        body,
        stall: None,
    }
}

/// A thread of posts 500 and 501 whose links the public service resolves:
/// 500's card leads only to `t.co`, 501's text links to the article.
fn linked_thread_answer(focal: u64) -> Vec<u8> {
    let mut first = thread_post("example_author", 500, "A thread with a link.");
    first["card"] = json!({"url": "https://t.co/EXAMPLE0"});
    let mut second = thread_post("example_author", 501, "The article it is about.");
    second["raw_text"] = json!({"text": "The article it is about. https://t.co/EXAMPLE1",
        "facets": [{"type": "url", "original": "https://t.co/EXAMPLE1",
            "replacement": ARTICLE, "display": "blog.example.test/post"}]});
    let status = if focal == 500 {
        first.clone()
    } else {
        second.clone()
    };
    serde_json::to_vec(&json!({"code": 200, "status": status, "thread": [first, second]})).unwrap()
}

/// Post 600, an X Article: its post text is empty, its title, body and
/// cover are the article's, and its only link is the article on X itself.
fn article_answer() -> Vec<u8> {
    let mut status = thread_post("example_author", 600, "");
    status["raw_text"] = json!({"text": "https://t.co/EXAMPLE6",
        "facets": [{"type": "url", "original": "https://t.co/EXAMPLE6",
            "replacement": "https://x.com/i/article/600", "display": "x.com/i/article/600"}]});
    status["article"] = json!({"id": "600", "title": "A synthetic article title",
        "preview_text": "The opening paragraph",
        "media_entities": [],
        "content": {"entityMap": [], "blocks": [
            {"type": "unstyled", "text": "The opening paragraph of the article.", "entityRanges": []},
            {"type": "header-two", "text": "A section", "entityRanges": []},
            {"type": "unordered-list-item", "text": "A point in a list.", "entityRanges": []},
            {"type": "unstyled", "text": "The closing paragraph of the article.", "entityRanges": []}]}});
    serde_json::to_vec(&json!({"code": 200, "status": status.clone(), "thread": [status]})).unwrap()
}

/// The X post API, standing in: post 100 is a post alone, post 401 is
/// private, posts 200 to 204 are one thread, 500 and 501 another with a
/// link, 600 an X Article; and the article at `blog.example.test`.
fn x_api() -> Site {
    Site::start(|host, path, _| {
        let id = path
            .strip_prefix("/2/thread/")
            .and_then(|id| id.parse::<u64>().ok());
        match (host, id) {
            ("api.fxtwitter.com", Some(100)) => json_reply(answer()),
            ("api.fxtwitter.com", Some(401)) => site::Reply::status(401),
            ("api.fxtwitter.com", Some(id @ 200..=204)) => json_reply(thread_answer(id)),
            ("api.fxtwitter.com", Some(id @ 500..=501)) => json_reply(linked_thread_answer(id)),
            ("api.fxtwitter.com", Some(600)) => json_reply(article_answer()),
            ("blog.example.test", _) if path == "/post" => {
                site::Reply::html(article("A synthetic article"))
            }
            _ => site::Reply::status(404),
        }
    })
}

/// `serve` with every name resolved to `site`.
fn serve(fx: &Fixture, site: &Site) -> Server {
    Server::from_child(serve_command(fx, site).spawn().expect("spawn serve"))
}

fn serve_command(fx: &Fixture, site: &Site) -> Command {
    let mut command = fx.command();
    command
        .args(["serve", "--port", "0"])
        .env("KNOWMORETABS_TEST_RESOLVE", site.address.to_string())
        .env("KNOWMORETABS_TEST_TIMEOUT_MS", "400")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    command
}

/// A JSON POST from `origin`, as the extension's service worker sends it.
fn post_from(server: &Server, origin: &str, path: &str, body: &Value) -> Reply {
    let body = body.to_string();
    server.raw(
        "POST",
        path,
        &[
            format!("Host: {}", server.host()),
            format!("Origin: {origin}"),
            "Content-Type: application/json".to_owned(),
            format!("Content-Length: {}", body.len()),
        ],
        body.as_bytes(),
    )
}

fn bookmark(server: &Server, post: &str) -> Reply {
    post_from(
        server,
        &extension_origin(),
        "/api/x/bookmark",
        &json!({"post": post}),
    )
}

/// A bookmark with what the page showed of it.
fn bookmark_read(server: &Server, post: &str, read: &Value) -> Value {
    let reply = post_from(
        server,
        &extension_origin(),
        "/api/x/bookmark",
        &json!({"post": post, "read": read}),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    reply.json()
}

/// Saves while a background request is paused, proving that the reply,
/// another archive writer and another bookmark can all finish first.
fn bookmark_while_read_waits(
    server: &Server,
    post: &str,
    read: &Value,
    waiting: &mpsc::Receiver<()>,
    release: &mpsc::Sender<()>,
) -> Value {
    std::thread::scope(|scope| {
        let (saved, reply) = mpsc::channel();
        scope.spawn(move || {
            let _ = saved.send(bookmark_read(server, post, read));
        });
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("the background request started");
        let reply = reply.recv_timeout(Duration::from_secs(2));
        let (changed, change) = mpsc::channel();
        scope.spawn(move || {
            let _ = changed.send(server.post("/api/forget", &json!({"urls": [A]})));
        });
        let change = change.recv_timeout(Duration::from_secs(2));
        let (saved_next, next) = mpsc::channel();
        scope.spawn(move || {
            let _ = saved_next.send(bookmark_read(
                server,
                &status_of(610),
                &page_read(
                    "example_author",
                    610,
                    &[shown("example_author", 610, "Another saved post.")],
                    true,
                ),
            ));
        });
        let next = next.recv_timeout(Duration::from_secs(2));
        // Release the site before asserting, including on a regression.
        release.send(()).unwrap();
        let reply = reply.expect("bookmark replied while its background read was waiting");
        assert_eq!(
            change
                .expect("another writer acquired the archive lock")
                .status,
            200
        );
        assert_eq!(
            next.expect("another bookmark saved while the read waited")["value"],
            "added"
        );
        reply
    })
}

/// A post as the extension reads it from the page.
fn shown(handle: &str, id: u64, text: &str) -> Value {
    json!({"id": id.to_string(), "handle": handle, "name": "Example Author",
        "at": "2026-01-01T10:00:00Z", "lang": "en", "text": text, "text_complete": true,
        "alts": [], "media": [], "links": []})
}

/// A page read of the clicked post `clicked` by `author`: whole on the
/// post's own page, else one post in Home.
fn page_read(author: &str, clicked: u64, posts: &[Value], complete: bool) -> Value {
    let (place, reasons) = if complete {
        ("status", vec![])
    } else {
        ("timeline", vec!["timeline"])
    };
    json!({"where": place, "clicked": clicked.to_string(), "author": author,
        "complete": complete, "reasons": reasons, "posts": posts})
}

fn status_of(id: u64) -> String {
    format!("https://x.com/example_author/status/{id}")
}

/// Post `id` as shown, its text linking through `t.co` to `target`, or
/// with only a cut label when the page did not resolve it.
fn linking(id: u64, target: Option<&str>) -> Value {
    let mut post = shown(
        "example_author",
        id,
        &format!("Shown post {id}, with a link."),
    );
    let label = target.map_or("blog.example.test/po…", |target| {
        target.split_once("://").map_or(target, |(_, rest)| rest)
    });
    post["links"] = json!([{"href": format!("https://t.co/EXAMPLE{id}"), "label": label,
        "target": target}]);
    post
}

/// The X log's lines tying a page to the pages its thread links to.
fn ties(fx: &Fixture) -> Vec<Value> {
    log_lines(fx, "xbookmarks.jsonl")
        .into_iter()
        .filter(|line| line.get("links").is_some())
        .map(|line| json!({"url": line["url"], "links": line["links"]}))
        .collect()
}

/// `url`'s title in the intake log, once its background read has given it
/// one.
fn titled_in_time(fx: &Fixture, url: &str) -> Value {
    for _ in 0..200 {
        let title = log_lines(fx, "added.jsonl")
            .into_iter()
            .rev()
            .find(|line| line["url"] == url)
            .map(|line| line["title"].clone());
        if let Some(title @ Value::String(_)) = title {
            return title;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!(
        "{url} was never titled: {:?}",
        log_lines(fx, "content.jsonl")
    )
}

fn content_line(fx: &Fixture, url: &str) -> Value {
    log_lines(fx, "content.jsonl")
        .into_iter()
        .rev()
        .find(|line| line["url"] == url)
        .unwrap_or_else(|| panic!("no content line for {url}"))
}

fn log_lines(fx: &Fixture, name: &str) -> Vec<Value> {
    fs::read_to_string(fx.root.join("pages").join(name))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The X log's click lines.
fn clicks(fx: &Fixture) -> Vec<Value> {
    log_lines(fx, "xbookmarks.jsonl")
        .into_iter()
        .filter(|line| line.get("post").is_some())
        .collect()
}

fn kept_front(fx: &Fixture, url: &str) -> String {
    let file = fx
        .root
        .join("pages")
        .join("content")
        .join(format!("{}.md", kept_name(url)));
    fs::read_to_string(file).unwrap_or_default()
}

#[test]
fn a_bookmark_click_saves_the_post_once_and_marks_it_on_every_click() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);

    let reply = bookmark(
        &server,
        "https://twitter.com/example_author/status/100/photo/1?s=20",
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    let line = reply.json();
    assert_eq!(
        (
            &line["stage"],
            &line["url"],
            &line["value"],
            &line["content"]
        ),
        (&json!("done"), &json!(POST), &json!("added"), &json!("ok")),
        "{line}"
    );
    assert_eq!(
        (&line["read"], site.paths()),
        (
            &json!("none"),
            vec!["api.fxtwitter.com/2/thread/100".to_owned()]
        ),
        "without a page read, only the post's number was asked"
    );
    let front = kept_front(&fx, POST);
    assert!(front.contains("tier: \"x\""), "{front}");
    let intake = log_lines(&fx, "added.jsonl");
    assert!(intake.iter().all(|line| line["url"] == POST), "{intake:?}");
    assert_eq!(
        intake.last().unwrap()["title"],
        "Example Author (@example_author) on X"
    );
    let first = clicks(&fx);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(
        (&first[0]["url"], &first[0]["post"], &first[0]["posts"]),
        (&json!(POST), &json!(POST), &json!(["100"]))
    );
    let library = server.library();
    let saved = page(&library, POST);
    assert_eq!(saved["x"]["clicked"], json!([POST]), "{saved}");
    assert_eq!(saved["x"]["bookmarked_at"], first[0]["at"]);
    assert!(
        page(&library, A).get("x").is_none(),
        "only bookmarked pages"
    );

    // The same post again: known, one more click, no new intake line.
    let again = bookmark(&server, POST).json();
    assert_eq!(again["value"], "known", "{again}");
    assert_eq!(clicks(&fx).len(), 2);
    assert_eq!(log_lines(&fx, "added.jsonl").len(), intake.len());
    let marked = log_lines(&fx, "xbookmarks.jsonl").len();

    // Forgotten: nothing written, and the reply says so.
    let forget = server.post("/api/forget", &json!({"urls": [POST]}));
    assert_eq!(forget.status, 200, "{}", forget.text());
    let forgotten = bookmark(&server, POST);
    assert_eq!(forgotten.status, 200, "{}", forgotten.text());
    assert_eq!(forgotten.json()["value"], "forgotten");
    assert_eq!(log_lines(&fx, "xbookmarks.jsonl").len(), marked);
    assert_eq!(log_lines(&fx, "added.jsonl").len(), intake.len());
}

#[test]
fn a_private_post_is_saved_without_its_text() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let reply = bookmark(&server, PRIVATE);
    assert_eq!(reply.status, 200, "{}", reply.text());
    let line = reply.json();
    assert_eq!(
        (&line["value"], &line["content"]),
        (&json!("added"), &json!("behind_login")),
        "{line}"
    );
    page(&server.library(), PRIVATE);
}

#[test]
fn a_whole_thread_read_on_its_page_is_kept_signed_in_and_nothing_is_asked() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let posts: Vec<Value> = (300..=303)
        .map(|id| shown("example_author", id, &format!("Shown post {id}.")))
        .collect();
    let line = bookmark_read(
        &server,
        &status_of(301),
        &page_read("example_author", 301, &posts, true),
    );
    assert_eq!(
        (&line["value"], &line["content"], &line["read"]),
        (&json!("added"), &json!("ok"), &json!("kept")),
        "{line}"
    );
    assert!(site.paths().is_empty(), "nothing asked: {:?}", site.paths());
    let front = kept_front(&fx, &status_of(301));
    assert!(front.contains("tier: \"signed_in\""), "{front}");
    assert!(front.contains("extractor: \"x-page 1\""), "{front}");
    assert!(front.contains("access: \"signed_in\""), "{front}");
    let at: Vec<usize> = (300..=303)
        .map(|id| front.find(&format!("Shown post {id}.")).unwrap())
        .collect();
    assert!(at.is_sorted(), "the page's order: {front}");
    assert_eq!(
        content_line(&fx, &status_of(301))["reason"],
        "4 posts as shown"
    );
    assert_eq!(clicks(&fx)[0]["posts"], json!(["300", "301", "302", "303"]));
    assert_eq!(
        log_lines(&fx, "added.jsonl").last().unwrap()["title"],
        "Example Author (@example_author) on X"
    );
}

#[test]
fn a_thread_shown_in_part_is_filled_from_the_public_thread_and_later_bookmarks_join_it() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let key = status_of(202);
    let one = |id: u64, text: &str| {
        page_read(
            "example_author",
            id,
            &[shown("example_author", id, text)],
            false,
        )
    };

    // The third post, from Home: the page read is written, then the
    // public thread's five posts are longer and replace it.
    let line = bookmark_read(
        &server,
        &key,
        &one(202, "Thread post 202, as Home shows it."),
    );
    assert_eq!(
        (&line["value"], &line["content"], &line["read"]),
        (&json!("added"), &json!("ok"), &json!("replaced")),
        "{line}"
    );
    assert_eq!(site.paths(), ["api.fxtwitter.com/2/thread/202"]);
    let reads: Vec<Value> = log_lines(&fx, "content.jsonl");
    assert_eq!(
        reads
            .iter()
            .map(|line| line["tier"].clone())
            .collect::<Vec<_>>(),
        [json!("signed_in"), json!("x")],
        "the page read, then the public thread"
    );
    let front = kept_front(&fx, &key);
    assert!(front.contains("tier: \"x\""), "{front}");
    let at: Vec<usize> = (200..=204)
        .map(|id| front.find(&format!("Thread post {id}, the whole")).unwrap())
        .collect();
    assert!(at.is_sorted(), "the thread's order: {front}");
    assert!(
        !front.contains("someone else"),
        "only the author's posts: {front}"
    );
    let thread = log_lines(&fx, "xbookmarks.jsonl").last().unwrap().clone();
    assert_eq!(thread.get("post"), None, "{thread}");
    assert_eq!(thread["posts"], json!(["200", "201", "202", "203", "204"]));

    // Another post of the thread, later: the same page, known, a click
    // more, and the shorter reads keep the thread's text.
    let again = bookmark_read(&server, &status_of(204), &one(204, "Thread post 204."));
    assert_eq!(
        (&again["url"], &again["value"], &again["read"]),
        (&json!(key), &json!("known"), &json!("kept")),
        "{again}"
    );
    assert_eq!(kept_front(&fx, &key), front, "the longer text stands");
    let kept = content_line(&fx, &key);
    assert_eq!(
        kept["reason"], "page read added no text; public read added no text",
        "{kept}"
    );
    let marked = clicks(&fx);
    assert_eq!(
        (&marked[1]["url"], &marked[1]["post"]),
        (&json!(key), &json!(status_of(204)))
    );
    let library = server.library();
    assert_eq!(
        page(&library, &key)["x"]["clicked"],
        json!([status_of(204), key])
    );
    assert!(
        library["pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|page| page["url"] != status_of(204)),
        "one page for the thread"
    );
}

#[test]
fn an_x_articles_page_read_that_shows_no_text_is_filled_from_the_public_article() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let key = status_of(600);
    // The Article view on the post's own page: the post is named, and its
    // title and body are drawn apart from the post's text.
    let line = bookmark_read(
        &server,
        &key,
        &page_read(
            "example_author",
            600,
            &[shown("example_author", 600, "")],
            true,
        ),
    );
    assert_eq!(
        (&line["value"], &line["content"], &line["read"]),
        (&json!("added"), &json!("ok"), &json!("replaced")),
        "{line}"
    );
    assert_eq!(
        line.get("article"),
        None,
        "the article on X is no page of its own: {line}"
    );
    assert_eq!(site.paths(), ["api.fxtwitter.com/2/thread/600"]);
    let front = kept_front(&fx, &key);
    assert!(front.contains("tier: \"x\""), "{front}");
    for part in [
        "The opening paragraph of the article.",
        "#### A section",
        "- A point in a list.",
        "The closing paragraph of the article.",
    ] {
        assert!(front.contains(part), "{part}: {front}");
    }
    assert_eq!(
        log_lines(&fx, "added.jsonl").last().unwrap()["title"],
        "A synthetic article title"
    );
}

#[test]
fn a_waiting_x_article_cover_holds_neither_the_bookmark_reply_nor_the_archive_lock() {
    let fx = Fixture::new();
    archive(&fx);
    let key = status_of(600);
    let mut answer: Value = serde_json::from_slice(&article_answer()).unwrap();
    // HTTP matches the stand-in's transport; HTTPS would stall in TLS
    // before the stand-in could see the request.
    answer["status"]["article"]["cover_media"] = json!({"media_info": {
        "original_img_url": "http://pbs.twimg.com/media/synthetic-cover.png"}});
    answer["thread"][0] = answer["status"].clone();
    let answer = serde_json::to_vec(&answer).unwrap();
    let (started, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let site = Site::start(move |host, path, _| match (host, path) {
        ("api.fxtwitter.com", "/2/thread/600") => json_reply(answer.clone()),
        ("pbs.twimg.com", "/media/synthetic-cover.png") => {
            started.send(()).unwrap();
            let _ = released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(15));
            let picture = image::RgbImage::from_pixel(320, 240, image::Rgb([40, 80, 120]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            picture
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            site::Reply {
                status: 200,
                headers: vec![("Content-Type", "image/png".into())],
                body: bytes.into_inner(),
                stall: None,
            }
        }
        _ => site::Reply::status(404),
    });
    let server = Server::from_child(
        serve_command(&fx, &site)
            .env("KNOWMORETABS_TEST_TIMEOUT_MS", "30000")
            .spawn()
            .expect("spawn serve"),
    );
    let reply = bookmark_while_read_waits(
        &server,
        &key,
        &page_read(
            "example_author",
            600,
            &[shown("example_author", 600, "")],
            true,
        ),
        &waiting,
        &release,
    );
    assert_eq!(
        (&reply["content"], &reply["read"]),
        (&json!("ok"), &json!("replaced"))
    );
    assert_eq!(
        page(&server.library(), &key)["title"],
        "A synthetic article title"
    );
    assert!(kept_front(&fx, &key).contains("The closing paragraph of the article."));
    // The background step still stores the cover through the ordinary
    // image path, and the Library serves that JPEG.
    for _ in 0..100 {
        let lines: Vec<Value> = log_lines(&fx, "images.jsonl")
            .into_iter()
            .filter(|line| line["url"] == key)
            .collect();
        if lines.iter().any(|line| line["status"] == "ok") {
            // The replaced page read's image is never tried: its line would
            // say the page has none while the cover waits.
            assert_eq!(lines.len(), 1, "one image attempt: {lines:?}");
            let jpeg = fs::read(
                fx.root
                    .join("pages/images")
                    .join(format!("{}.jpg", kept_name(&key))),
            )
            .unwrap();
            assert_eq!(
                image::guess_format(&jpeg).unwrap(),
                image::ImageFormat::Jpeg
            );
            assert_eq!(server.served(&key)["image"]["status"], "ok");
            assert_eq!(
                server.get(&format!("/api/image/{}", kept_name(&key))).body,
                jpeg
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the Article cover was never kept");
}

#[test]
fn a_protected_thread_keeps_its_page_read_and_says_the_public_read_added_nothing() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let read = page_read(
        "example_private",
        401,
        &[shown(
            "example_private",
            401,
            "Only followers see this thread.",
        )],
        false,
    );
    let line = bookmark_read(&server, PRIVATE, &read);
    assert_eq!(
        (&line["content"], &line["read"]),
        (&json!("ok"), &json!("kept")),
        "{line}"
    );
    assert_eq!(site.paths(), ["api.fxtwitter.com/2/thread/401"]);
    let kept = content_line(&fx, PRIVATE);
    assert_eq!(
        kept["reason"],
        "1 post as shown; thread may continue; public read added no text"
    );
    assert_eq!(
        (&kept["tier"], &kept["access"]),
        (&json!("signed_in"), &json!("signed_in")),
        "the log says what the stored file says: {kept}"
    );
    let front = kept_front(&fx, PRIVATE);
    assert!(front.contains("Only followers see this thread."), "{front}");
    assert!(front.contains("tier: \"signed_in\""), "{front}");
}

#[test]
fn a_read_the_shell_refuses_still_saves_the_bookmark_through_the_public_thread() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let many: Vec<Value> = (1..=51)
        .map(|id| shown("example_author", id, "x"))
        .collect();
    for read in [
        page_read("example_author", 100, &many, true),
        page_read(
            "example_author",
            999,
            &[shown("example_author", 999, "Not it.")],
            true,
        ),
        json!("not a read"),
    ] {
        let line = bookmark_read(&server, POST, &read);
        assert_eq!(
            (&line["content"], &line["read"]),
            (&json!("ok"), &json!("refused")),
            "{line}"
        );
    }
    assert_eq!(
        site.paths(),
        vec!["api.fxtwitter.com/2/thread/100".to_owned(); 3]
    );
    assert!(
        log_lines(&fx, "content.jsonl")
            .iter()
            .all(|line| line["tier"] == "x"),
        "no page read was kept"
    );
    assert_eq!(clicks(&fx)[0]["posts"], json!(["100"]));
}

#[test]
fn two_saves_at_once_in_one_thread_make_one_page_and_two_casings_one_key() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let posts = [
        shown("example_author", 300, "Shown post 300."),
        shown("example_author", 301, "Shown post 301."),
    ];
    let start = std::sync::Barrier::new(2);
    let lines: Vec<Value> = std::thread::scope(|scope| {
        let saves: Vec<_> = [300, 301]
            .map(|id| {
                let (server, start, posts) = (&server, &start, &posts);
                scope.spawn(move || {
                    start.wait();
                    bookmark_read(
                        server,
                        &status_of(id),
                        &page_read("example_author", id, posts, true),
                    )
                })
            })
            .into_iter()
            .collect();
        saves.into_iter().map(|save| save.join().unwrap()).collect()
    });
    assert_eq!(lines[0]["url"], lines[1]["url"], "{lines:?}");
    let mut values: Vec<&Value> = lines.iter().map(|line| &line["value"]).collect();
    values.sort_by_key(ToString::to_string);
    assert_eq!(values, [&json!("added"), &json!("known")]);

    let cased = bookmark(&server, "https://x.com/Example_Author/status/100").json();
    let lower = bookmark(&server, POST).json();
    assert_eq!(
        (&cased["url"], &cased["value"], &lower["value"]),
        (&json!(POST), &json!("added"), &json!("known"))
    );
}

#[test]
fn an_x_log_that_cannot_be_read_costs_the_marks_and_not_the_library() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    assert_eq!(bookmark(&server, POST).json()["value"], "added");
    let log = fx.root.join("pages").join("xbookmarks.jsonl");
    fs::remove_file(&log).unwrap();
    fs::create_dir(&log).unwrap();
    let library = server.library();
    assert!(page(&library, POST).get("x").is_none(), "no marks");
}

#[test]
fn the_routes_refuse_what_is_not_a_post_and_every_origin_but_the_extensions() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    for post in [
        "https://x.com/i/status/100",
        "https://x.com/example_author",
        "https://example.org/example_author/status/100",
        "https://x.com/bad-handle/status/100",
        "",
    ] {
        let reply = bookmark(&server, post);
        assert_eq!(reply.status, 400, "{post}: {}", reply.text());
    }
    let shapeless = post_from(&server, &extension_origin(), "/api/x/bookmark", &json!({}));
    assert_eq!(shapeless.status, 400, "{}", shapeless.text());
    for origin in [
        "https://x.com",
        "https://example.org",
        "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        for path in ["/api/x/bookmark", "/api/x/health", "/api/tags"] {
            let reply = post_from(&server, origin, path, &json!({"post": POST}));
            assert_eq!(reply.status, 403, "{origin} {path}: {}", reply.text());
        }
    }
    let body = format!("post={POST}");
    let form = server.raw(
        "POST",
        "/api/x/bookmark",
        &[
            format!("Host: {}", server.host()),
            "Content-Type: application/x-www-form-urlencoded".to_owned(),
            format!("Content-Length: {}", body.len()),
        ],
        body.as_bytes(),
    );
    assert_eq!(form.status, 415, "{}", form.text());
    assert!(
        !fx.root.join("pages").join("xbookmarks.jsonl").exists(),
        "nothing written"
    );
    assert!(site.paths().is_empty(), "nothing asked: {:?}", site.paths());

    // The extension counts as the Library itself on the other routes.
    let created = fx.run(&["tags", "--create", "Harness"]);
    assert!(created.status.success(), "{}", stderr(&created));
    let tagged = post_from(
        &server,
        &extension_origin(),
        "/api/tags",
        &json!({"urls": [A], "add": ["Harness"]}),
    );
    assert_eq!(tagged.status, 200, "{}", tagged.text());
}

fn doctor_line(fx: &Fixture) -> Value {
    let output = fx.run(&["--json", "doctor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["tier"] == "extension")
        .cloned()
        .unwrap_or_else(|| panic!("no extension line: {report}"))
}

fn health(state: &str) -> Value {
    json!({"state": state, "since": "2026-10-27T09:00:00Z", "version": "0.2.0", "views": 12,
        "posts": 140, "controls": 140, "intents": 3, "confirmed": 3, "dropped": 0,
        "last_dropped": null})
}

#[test]
fn the_extensions_health_report_is_kept_and_doctor_says_it() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    assert_eq!(doctor_line(&fx)["state"], "missing");

    let report = |body: &Value| post_from(&server, &extension_origin(), "/api/x/health", body);
    let ok = report(&health("ok"));
    assert_eq!((ok.status, ok.json()), (200, json!({})), "{}", ok.text());
    let kept: Value =
        serde_json::from_slice(&fs::read(fx.root.join("extension.json")).unwrap()).unwrap();
    assert_eq!(kept["state"], "ok");
    assert!(kept["reported_at"].is_string(), "{kept}");
    assert_eq!(doctor_line(&fx)["state"], "ready");

    let blind = report(&health("blind"));
    assert_eq!(blind.status, 200, "{}", blind.text());
    let line = doctor_line(&fx);
    assert_eq!(line["state"], "degraded", "{line}");
    assert_eq!(
        line["detail"],
        "sees posts on X but no bookmark button since 2026-10-27"
    );

    let mut with_url = health("ok");
    with_url["url"] = json!(POST);
    let mut bad_version = health("ok");
    bad_version["version"] = json!("latest");
    for bad in [health("lost"), with_url, bad_version] {
        let reply = report(&bad);
        assert_eq!(reply.status, 400, "{bad}: {}", reply.text());
    }
    assert_eq!(
        doctor_line(&fx)["state"],
        "degraded",
        "the last good report stands"
    );
}

#[test]
fn bookmarks_dropped_while_the_library_was_down_degrade_doctor() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let report = |body: &Value| post_from(&server, &extension_origin(), "/api/x/health", body);
    let last = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
    let mut dropped = health("ok");
    dropped["dropped"] = json!(3);
    dropped["last_dropped"] = json!(last.to_string());
    let kept = report(&dropped);
    assert_eq!(kept.status, 200, "{}", kept.text());
    let written: Value =
        serde_json::from_slice(&fs::read(fx.root.join("extension.json")).unwrap()).unwrap();
    assert_eq!(
        (&written["dropped"], &written["last_dropped"]),
        (&json!(3), &dropped["last_dropped"]),
        "{written}"
    );
    let line = doctor_line(&fx);
    assert_eq!(line["state"], "degraded", "{line}");
    assert_eq!(
        line["detail"],
        format!(
            "3 bookmarks on X were not saved because knowmoretabs was not answering; last {}",
            last.strftime("%Y-%m-%d")
        )
    );
    assert_eq!(
        line["hint"],
        "keep `just up` running while you bookmark on X"
    );

    let mut bad = Vec::new();
    for (count, last) in [
        (json!(-1), dropped["last_dropped"].clone()),
        (json!(1.5), dropped["last_dropped"].clone()),
        (json!(1_000_001), dropped["last_dropped"].clone()),
        (json!(3), json!(null)),
        (json!(0), dropped["last_dropped"].clone()),
        (json!(3), json!("yesterday")),
    ] {
        let mut body = health("ok");
        body["dropped"] = count;
        body["last_dropped"] = last;
        bad.push(body);
    }
    let mut missing = health("ok");
    missing.as_object_mut().unwrap().remove("dropped");
    bad.push(missing);
    for body in bad {
        let reply = report(&body);
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
    }

    let ok = report(&health("ok"));
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(doctor_line(&fx)["state"], "ready", "no drops, ready again");
}

#[test]
fn the_first_link_off_x_is_its_own_page_tied_to_the_post_and_read_in_the_background() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let posts = [
        shown("example_author", 300, "Shown post 300."),
        linking(301, Some("https://x.com/example_other/status/9")),
        linking(302, Some("http://blog.example.test/post?utm_source=x")),
        linking(303, Some("https://video.example.test/watch")),
    ];
    let line = bookmark_read(
        &server,
        &status_of(301),
        &page_read("example_author", 301, &posts, true),
    );
    assert_eq!(
        (&line["value"], &line["article"]),
        (&json!("added"), &json!({"url": ARTICLE, "value": "added"})),
        "the first link off X, under its cleaned key: {line}"
    );
    assert_eq!(
        ties(&fx),
        [json!({"url": status_of(301), "links": [ARTICLE]})]
    );
    assert_eq!(titled_in_time(&fx, ARTICLE), "A synthetic article");
    let read = content_line(&fx, ARTICLE);
    assert_eq!(
        (&read["status"], &read["tier"]),
        (&json!("ok"), &json!("web"))
    );
    assert!(
        site.paths()
            .iter()
            .all(|path| !path.contains("video.example.test")),
        "one article per bookmark: {:?}",
        site.paths()
    );
    let library = server.library();
    assert_eq!(
        page(&library, &status_of(301))["x"]["links"],
        json!([ARTICLE])
    );
    assert!(
        page(&library, ARTICLE).get("x").is_none(),
        "the article was not bookmarked itself"
    );
}

#[test]
fn a_waiting_article_holds_neither_the_bookmark_reply_nor_the_archive_lock() {
    let fx = Fixture::new();
    archive(&fx);
    let (started, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let site = Site::start(move |_, _, _| {
        started.send(()).unwrap();
        let _ = released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(15));
        site::Reply::html(article("A synthetic article"))
    });
    let server = Server::from_child(
        serve_command(&fx, &site)
            .env("KNOWMORETABS_TEST_TIMEOUT_MS", "30000")
            .spawn()
            .expect("spawn serve"),
    );
    let reply = bookmark_while_read_waits(
        &server,
        &status_of(340),
        &page_read("example_author", 340, &[linking(340, Some(ARTICLE))], true),
        &waiting,
        &release,
    );
    assert_eq!(reply["article"], json!({"url": ARTICLE, "value": "added"}));
    assert_eq!(titled_in_time(&fx, ARTICLE), "A synthetic article");
}

#[test]
fn a_link_the_page_left_unresolved_is_found_in_the_public_thread() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let read = page_read("example_author", 501, &[linking(501, None)], false);
    let line = bookmark_read(&server, &status_of(501), &read);
    assert_eq!(
        (&line["read"], &line["article"]),
        (
            &json!("replaced"),
            &json!({"url": ARTICLE, "value": "added"})
        ),
        "{line}"
    );
    assert_eq!(
        ties(&fx),
        [json!({"url": status_of(501), "links": [ARTICLE]})]
    );
    assert_eq!(titled_in_time(&fx, ARTICLE), "A synthetic article");
}

#[test]
fn an_article_already_kept_is_tied_and_a_forgotten_one_is_neither_tied_nor_restored() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let whole =
        |id: u64, target: &str| page_read("example_author", id, &[linking(id, Some(target))], true);
    let known = bookmark_read(&server, &status_of(310), &whole(310, A));
    assert_eq!(
        known["article"],
        json!({"url": A, "value": "known"}),
        "{known}"
    );
    let forgotten = bookmark_read(&server, &status_of(320), &whole(320, HIDDEN));
    assert_eq!(
        forgotten["article"],
        json!({"url": HIDDEN, "value": "forgotten"}),
        "{forgotten}"
    );
    assert_eq!(ties(&fx), [json!({"url": status_of(310), "links": [A]})]);
    let intake = log_lines(&fx, "added.jsonl");
    assert!(
        intake
            .iter()
            .all(|line| line["url"] != A && line["url"] != HIDDEN),
        "{intake:?}"
    );
    let state = fs::read_to_string(fx.root.join("library.json")).unwrap();
    assert!(state.contains(HIDDEN), "still forgotten: {state}");
}

#[test]
fn no_link_off_x_that_add_would_take_makes_no_article() {
    let fx = Fixture::new();
    archive(&fx);
    let site = x_api();
    let server = serve(&fx, &site);
    let posts = [
        linking(330, Some("https://x.com/example_other/status/9")),
        linking(331, Some("https://pic.x.com/EXAMPLE")),
        linking(332, Some("https://pbs.twimg.com/media/EXAMPLE.jpg")),
        linking(333, Some("http://192.168.0.10/admin")),
        linking(334, None),
    ];
    let line = bookmark_read(
        &server,
        &status_of(330),
        &page_read("example_author", 330, &posts, true),
    );
    assert_eq!(line["value"], "added", "{line}");
    assert_eq!(line.get("article"), None, "{line}");
    assert_eq!(ties(&fx), [] as [Value; 0]);
    assert_eq!(
        site.paths(),
        ["api.fxtwitter.com/2/thread/330"],
        "the unresolved link asked the public thread; no link was fetched"
    );
    let intake = log_lines(&fx, "added.jsonl");
    assert!(
        intake.iter().all(|line| line["url"] == status_of(330)),
        "only the post's page: {intake:?}"
    );
}

#[test]
fn a_plain_text_link_is_the_article_kept_as_written_and_titled_by_its_file_name() {
    const SCRIPT: &str = "http://files.example.test/tools/install.sh";
    let fx = Fixture::new();
    archive(&fx);
    let site = Site::start(|host, path, _| match (host, path) {
        ("files.example.test", "/tools/install.sh") => site::Reply {
            status: 200,
            headers: vec![("Content-Type", "text/plain".into())],
            body: b"#!/bin/sh\n<script>alert(1)</script>\necho \"$HOME\"\n".to_vec(),
            stall: None,
        },
        ("blog.example.test", "/post") => site::Reply::html(article("A synthetic article")),
        _ => site::Reply::status(404),
    });
    let server = serve(&fx, &site);
    let posts = [linking(350, Some(SCRIPT)), linking(351, Some(ARTICLE))];
    let line = bookmark_read(
        &server,
        &status_of(350),
        &page_read("example_author", 350, &posts, true),
    );
    assert_eq!(
        line["article"],
        json!({"url": SCRIPT, "value": "added"}),
        "{line}"
    );
    assert_eq!(titled_in_time(&fx, SCRIPT), "install.sh");
    let read = content_line(&fx, SCRIPT);
    assert_eq!(
        (&read["status"], &read["extractor"]),
        (&json!("ok"), &json!("plain-text"))
    );
    assert!(kept_front(&fx, SCRIPT).contains("<script>alert(1)</script>\necho \"$HOME\"\n"));
    let library = server.library();
    assert_eq!(
        page(&library, &status_of(350))["x"]["links"],
        json!([SCRIPT])
    );
    assert_eq!(page(&library, SCRIPT)["title"], "install.sh");
}
