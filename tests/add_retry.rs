//! `knowmoretabs add --retry` through the real binary: only the stages
//! named run, only where they failed, and what succeeded is left as it was.
//! Pages and their image are read from a local server that stands in for
//! every site, answering as each test sets it, so nothing reaches the
//! internet.

mod common;

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use common::add::{add, article, at_site, intake, lines};
use common::site::{Reply, Site};
use common::{Fixture, assert_success, fingerprint, stderr, stdout};
use serde_json::{Value, json};

const A: &str = "http://a.test/one";
const PAGE: &str = "/one";
/// The page's image, on a host of its own so it waits for no page.
const IMAGE: &str = "http://cdn.test/pic.png";

/// A site whose page and image answer with the status each test sets: 200
/// is the article naming the image, and the image itself.
struct Stub {
    site: Site,
    page: Arc<AtomicU16>,
    image: Arc<AtomicU16>,
}

impl Stub {
    fn start() -> Self {
        let (page, image) = (Arc::new(AtomicU16::new(200)), Arc::new(AtomicU16::new(200)));
        let png = png();
        let site = Site::start({
            let (page, image) = (Arc::clone(&page), Arc::clone(&image));
            move |host, path, _| match (host, path) {
                ("a.test", PAGE) => match page.load(Ordering::SeqCst) {
                    200 => Reply::html(article("One note").replace(
                        "</head>",
                        &format!("<meta property=\"og:image\" content=\"{IMAGE}\"></head>"),
                    )),
                    status => Reply::status(status),
                },
                ("cdn.test", "/pic.png") => match image.load(Ordering::SeqCst) {
                    200 => Reply {
                        headers: vec![("Content-Type", "image/png".into())],
                        body: png.clone(),
                        ..Reply::status(200)
                    },
                    status => Reply::status(status),
                },
                _ => Reply::status(404),
            }
        });
        Self { site, page, image }
    }

    fn page(&self, status: u16) {
        self.page.store(status, Ordering::SeqCst);
    }

    fn image(&self, status: u16) {
        self.image.store(status, Ordering::SeqCst);
    }

    /// How many requests reached the page and the image.
    fn asked(&self) -> (usize, usize) {
        let seen = self.site.seen();
        let count = |host: &str| seen.iter().filter(|s| s.host == host).count();
        (count("a.test"), count("cdn.test"))
    }
}

/// A picture big enough to keep.
fn png() -> Vec<u8> {
    let picture = image::RgbImage::from_fn(320, 240, |x, y| {
        image::Rgb([
            u8::try_from(x % 256).unwrap(),
            u8::try_from(y % 256).unwrap(),
            128,
        ])
    });
    let mut bytes = Cursor::new(Vec::new());
    picture
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn retry(fx: &Fixture, stub: &Stub, url: &str, args: &[&str]) -> Output {
    let mut all = vec![url, "--json"];
    for stage in args {
        all.extend(["--retry", stage]);
    }
    add(fx, &stub.site, &all)
}

/// The text read again after a successful one, with no image: the way a
/// page that once read well comes to fail.
fn refetch_text(fx: &Fixture, stub: &Stub) {
    let args = [
        "content",
        "--url",
        A,
        "--refetch",
        "--no-images",
        "--no-browser",
    ];
    assert_success(&at_site(fx, &stub.site, &args));
}

fn log(fx: &Fixture, name: &str) -> PathBuf {
    fx.root.join("pages").join(name)
}

/// The latest line a log holds for `url`.
fn latest(fx: &Fixture, name: &str, url: &str) -> Value {
    std::fs::read_to_string(log(fx, name))
        .unwrap()
        .lines()
        .rev()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|line| line["url"] == url)
        .unwrap()
}

/// Each line's stage and state.
fn steps(lines: &[Value]) -> Vec<(String, String)> {
    lines
        .iter()
        .map(|l| {
            (
                l["stage"].as_str().unwrap().into(),
                l["state"].as_str().unwrap().into(),
            )
        })
        .collect()
}

fn done(lines: &[Value]) -> &Value {
    let last = lines.last().unwrap();
    assert_eq!(last["stage"], "done");
    last
}

#[test]
fn a_content_retry_reads_the_text_and_never_a_kept_image() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    let first = add(&fx, &stub.site, &[A, "--json"]);
    assert_eq!(
        (
            done(&lines(&first))["content"].clone(),
            done(&lines(&first))["image"].clone()
        ),
        (json!("ok"), json!("ok"))
    );
    stub.page(500);
    refetch_text(&fx, &stub);
    assert_eq!(latest(&fx, "content.jsonl", A)["status"], "error");
    stub.page(200);
    let (images, added) = (fingerprint(&fx.root.join("pages")), intake(&fx));
    let image_log = std::fs::read(log(&fx, "images.jsonl")).unwrap();
    let asked = stub.asked();

    let output = retry(&fx, &stub, A, &["content"]);
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(
        lines[1..4],
        [
            json!({"stage": "library", "state": "done", "value": "known"}),
            json!({"stage": "content", "state": "running", "tier": "web"}),
            json!({"stage": "content", "state": "done", "status": "ok", "tier": "web",
                "http_status": 200, "title": "One note"}),
        ]
    );
    assert_eq!(
        done(&lines),
        &json!({"stage": "done", "state": "done", "url": A, "value": "known",
            "content": "ok", "image": "ok"})
    );
    assert_eq!(stub.asked(), (asked.0 + 1, asked.1), "no image request");
    assert_eq!(std::fs::read(log(&fx, "images.jsonl")).unwrap(), image_log);
    let kept = |all: &[(PathBuf, Vec<u8>, std::time::SystemTime)]| {
        all.iter()
            .filter(|(path, ..)| path.starts_with(fx.root.join("pages").join("images")))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(kept(&fingerprint(&fx.root.join("pages"))), kept(&images));
    assert_eq!(intake(&fx), added, "no intake line");

    // A plain add in the same state fetches the kept image again.
    stub.page(500);
    refetch_text(&fx, &stub);
    stub.page(200);
    let asked = stub.asked();
    assert_success(&add(&fx, &stub.site, &[A, "--json"]));
    assert_eq!(stub.asked(), (asked.0 + 1, asked.1 + 1));
}

#[test]
fn a_retry_of_what_succeeded_sends_nothing() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    assert_success(&add(&fx, &stub.site, &[A, "--json"]));
    let (pages, asked) = (fingerprint(&fx.root.join("pages")), stub.asked());
    for stages in [&["content"][..], &["image"], &["content", "image"]] {
        let output = retry(&fx, &stub, A, stages);
        assert_success(&output);
        let lines = lines(&output);
        assert!(
            !lines
                .iter()
                .any(|l| l["state"] == "running" && l["stage"] == "content"),
            "{stages:?}"
        );
        assert_eq!(
            (&done(&lines)["content"], &done(&lines)["image"]),
            (&json!("ok"), &json!("ok"))
        );
    }
    assert_eq!(stub.asked(), asked, "no request");
    assert_eq!(
        fingerprint(&fx.root.join("pages")),
        pages,
        "nothing written"
    );
}

#[test]
fn an_image_retry_ignores_signed_in_and_the_text() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.image(500);
    let first = add(&fx, &stub.site, &[A, "--json"]);
    assert_eq!(done(&lines(&first))["image"], "error");
    stub.page(500);
    refetch_text(&fx, &stub);
    stub.image(200);
    let asked = stub.asked();

    // The text failed and is not named: it stands. Remote debugging is off
    // in the fixture's Chrome, and the image never reaches for it.
    let output = add(
        &fx,
        &stub.site,
        &[A, "--retry", "image", "--signed-in", "--json"],
    );
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(
        (&lines[2]["stage"], &lines[2]["state"], &lines[2]["status"]),
        (&json!("content"), &json!("done"), &json!("error"))
    );
    assert_eq!(
        lines[3..],
        [
            json!({"stage": "image", "state": "running"}),
            json!({"stage": "image", "state": "done", "status": "ok"}),
            json!({"stage": "done", "state": "done", "url": A, "value": "known",
                "content": "error", "image": "ok"}),
        ]
    );
    assert_eq!(stub.asked(), (asked.0, asked.1 + 1), "the image alone");
}

#[test]
fn both_stages_retry_in_order_when_the_text_fails_again() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.image(500);
    assert_success(&add(&fx, &stub.site, &[A, "--json"]));
    stub.page(500);
    refetch_text(&fx, &stub);
    stub.image(200);

    let output = retry(&fx, &stub, A, &["content", "image"]);
    assert_success(&output);
    let lines = lines(&output);
    let steps = steps(&lines);
    let steps: Vec<(&str, &str)> = steps
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        steps[2..],
        [
            ("content", "running"),
            ("content", "done"),
            ("image", "running"),
            ("image", "done"),
            ("done", "done")
        ]
    );
    assert_eq!(
        (&lines[3]["status"], &lines[5]["status"]),
        (&json!("error"), &json!("ok"))
    );
    assert_eq!(latest(&fx, "content.jsonl", A)["attempt"], 2);
}

#[test]
fn a_browser_out_of_reach_still_lets_the_image_retry() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.image(500);
    assert_success(&add(&fx, &stub.site, &[A, "--json"]));
    stub.page(403);
    refetch_text(&fx, &stub);
    assert_eq!(latest(&fx, "content.jsonl", A)["status"], "blocked");
    stub.image(200);
    let text = std::fs::read(log(&fx, "content.jsonl")).unwrap();

    let output = add(
        &fx,
        &stub.site,
        &[A, "--retry", "content,image", "--signed-in", "--json"],
    );
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(
        lines[2..4],
        [
            json!({"stage": "content", "state": "running", "tier": "signed_in"}),
            json!({"stage": "content", "state": "done", "status": "off", "tier": "signed_in"}),
        ]
    );
    assert_eq!(
        (&done(&lines)["content"], &done(&lines)["image"]),
        (&json!("off"), &json!("ok"))
    );
    assert_eq!(
        std::fs::read(log(&fx, "content.jsonl")).unwrap(),
        text,
        "byte for byte"
    );
}

#[test]
fn an_image_first_fetched_after_the_text_is_not_retried_in_the_same_run() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.page(500);
    let first = add(&fx, &stub.site, &[A, "--json"]);
    assert_eq!(done(&lines(&first))["image"], "unknown", "no image line");
    stub.page(200);
    stub.image(500);

    let output = retry(&fx, &stub, A, &["content", "image"]);
    assert_success(&output);
    let lines = lines(&output);
    assert_eq!(
        (&done(&lines)["content"], &done(&lines)["image"]),
        (&json!("ok"), &json!("error"))
    );
    let image = std::fs::read_to_string(log(&fx, "images.jsonl")).unwrap();
    assert_eq!(image.lines().count(), 1);
    assert_eq!(latest(&fx, "images.jsonl", A)["attempt"], 1);
    let titles: Vec<Value> = intake(&fx)
        .into_iter()
        .map(|line| line["title"].clone())
        .collect();
    assert_eq!(
        titles,
        [Value::Null, json!("One note")],
        "the title the retry read joins the page"
    );
}

#[test]
fn retries_count_toward_unavailable_which_stands() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.page(500);
    let mut ended = vec![done(&lines(&add(&fx, &stub.site, &[A, "--json"])))["content"].clone()];
    for _ in 0..3 {
        ended.push(done(&lines(&retry(&fx, &stub, A, &["content"])))["content"].clone());
    }
    assert_eq!(ended, ["error", "error", "unavailable", "unavailable"]);
    let asked = stub.asked();
    assert_success(&retry(&fx, &stub, A, &["content"]));
    assert_eq!(stub.asked(), asked, "unavailable is never read again");

    let (fx, stub) = (Fixture::new(), Stub::start());
    stub.image(500);
    let mut ended = vec![done(&lines(&add(&fx, &stub.site, &[A, "--json"])))["image"].clone()];
    for _ in 0..3 {
        ended.push(done(&lines(&retry(&fx, &stub, A, &["image"])))["image"].clone());
    }
    assert_eq!(ended, ["error", "error", "unavailable", "unavailable"]);
    let asked = stub.asked();
    assert_success(&retry(&fx, &stub, A, &["image"]));
    assert_eq!(stub.asked(), asked);
}

#[test]
fn a_content_retry_leaves_a_final_status_standing() {
    let site = Site::start(|_, path, _| match path {
        "/thin" => {
            Reply::html("<html><head><title>T</title></head><body><p>Short.</p></body></html>")
        }
        "/blocked" => Reply::status(403),
        _ => Reply::status(404),
    });
    let stub = Stub {
        site,
        page: Arc::default(),
        image: Arc::default(),
    };
    let fx = Fixture::new();
    let thin = "http://a.test/thin";
    assert_success(&add(&fx, &stub.site, &[thin, "--no-content"]));
    let args = ["content", "--url", thin, "--no-browser"];
    assert_success(&at_site(&fx, &stub.site, &args));
    for (url, status) in [
        (thin, "thin"),
        ("http://a.test/blocked", "blocked"),
        ("http://a.test/gone", "not_found"),
    ] {
        if url != thin {
            assert_success(&add(&fx, &stub.site, &[url, "--json"]));
        }
        assert_eq!(latest(&fx, "content.jsonl", url)["status"], status);
        let asked = stub.site.seen().len();
        let output = retry(&fx, &stub, url, &["content"]);
        assert_success(&output);
        assert_eq!(done(&lines(&output))["content"], status, "{url}");
        assert_eq!(stub.site.seen().len(), asked, "{url}: no request");
    }
}

#[test]
fn a_retry_never_adds_a_page() {
    let (fx, stub) = (Fixture::new(), Stub::start());
    let output = retry(&fx, &stub, A, &["content"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        lines(&output),
        [
            json!({"stage": "library", "state": "running"}),
            json!({"stage": "library", "state": "done", "value": "refused",
                "reason": "not_in_library"}),
            json!({"stage": "done", "state": "done", "url": A, "value": "refused",
                "content": null, "image": null}),
        ]
    );
    assert!(!fx.root.exists(), "nothing written");
    let output = add(&fx, &stub.site, &[A, "--retry", "image"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("not retried"),
        "{}",
        stderr(&output)
    );
    assert_eq!(stub.asked(), (0, 0));

    let other = "http://a.test/other";
    assert_success(&add(&fx, &stub.site, &[other, "--no-content"]));
    let added = intake(&fx);
    assert_eq!(retry(&fx, &stub, A, &["image"]).status.code(), Some(1));
    assert_eq!(intake(&fx), added);

    assert_success(&fx.run(&["forget", other]));
    let output = retry(&fx, &stub, other, &["content"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(lines(&output)[1]["value"], "forgotten");
    assert_eq!(intake(&fx), added);
}

#[test]
fn a_retry_with_a_title_or_without_content_is_a_usage_error() {
    let fx = Fixture::new();
    for args in [
        &[A, "--retry", "content", "--no-content"][..],
        &[A, "--retry", "content", "--title", "T"],
        &[A, "--retry", "nonsense"],
    ] {
        let output = fx.run(&[&["add"][..], args].concat());
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&output), "");
    }
    assert!(!fx.root.exists());
}
