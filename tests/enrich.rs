//! `knowmoretabs enrich` through the real binary against a local HTTP server
//! that stands in for every site: what is fetched and what never is, what a
//! page's head becomes, redirects, login walls, charsets, caps, timeouts,
//! pacing, and a run interrupted halfway. Nothing here touches the internet:
//! a debug build resolves every name to the test server.

#![cfg(debug_assertions)]

mod common;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Fixture, assert_success, stderr, stdout, write_snapshot};
use serde_json::Value;

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Seen {
    at: Instant,
    host: String,
    path: String,
    headers: HashMap<String, String>,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Wait this long before answering at all.
    stall: Option<Duration>,
}

impl Reply {
    fn html(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type", "text/html; charset=utf-8".into())],
            body: body.into(),
            stall: None,
        }
    }

    fn redirect(status: u16, location: &str) -> Self {
        Self {
            status,
            headers: vec![
                ("Location", location.to_owned()),
                ("Set-Cookie", "session=abc; Path=/".to_owned()),
            ],
            body: Vec::new(),
            stall: None,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/html".into())],
            body: b"<title>nope</title>".to_vec(),
            stall: None,
        }
    }
}

type Route = dyn Fn(&str, &str, u16) -> Reply + Send + Sync;

/// A server on an ephemeral loopback port that answers for every host name,
/// one request per connection, and remembers each request.
struct Site {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Site {
    fn start(route: impl Fn(&str, &str, u16) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let route: Arc<Route> = Arc::new(route);
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (route, log) = (Arc::clone(&route), Arc::clone(&log));
                std::thread::spawn(move || serve(stream, &*route, &log, address.port()));
            }
        });
        Self { address, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn paths(&self) -> Vec<String> {
        self.seen()
            .iter()
            .map(|s| format!("{}{}", s.host, s.path))
            .collect()
    }
}

fn serve(stream: TcpStream, route: &Route, log: &Mutex<Vec<Seen>>, port: u16) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let host = headers
        .get("host")
        .map(|h| h.split(':').next().unwrap_or("").to_owned())
        .unwrap_or_default();
    log.lock().unwrap().push(Seen {
        at: Instant::now(),
        host: host.clone(),
        path: path.clone(),
        headers,
    });
    let reply = route(&host, &path, port);
    if let Some(stall) = reply.stall {
        std::thread::sleep(stall);
    }
    let mut out = stream;
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    // The client may stop reading once it has the head; that is the point.
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&reply.body);
}

fn page(title: &str) -> String {
    format!("<html><head><title>{title}</title></head><body>body</body></html>")
}

/// Every URL in one snapshot, in tab order; the newest sighting is first.
fn library(fx: &Fixture, urls: &[&str]) {
    let tabs: Vec<(i32, &str, &str)> = urls
        .iter()
        .enumerate()
        .map(|(i, url)| (i32::try_from(i).unwrap() + 1, *url, "Tab title"))
        .collect();
    write_snapshot(
        &fx.root,
        "2026-09-01-000000Z",
        "2026-09-01T00:00:00Z",
        &tabs,
    );
}

fn enrich(fx: &Fixture, site: &Site, args: &[&str]) -> Output {
    enrich_command(fx, site, args).output().unwrap()
}

fn enrich_command(fx: &Fixture, site: &Site, args: &[&str]) -> Command {
    let mut cmd = fx.command();
    cmd.arg("enrich")
        .args(args)
        .env("KNOWMORETABS_TEST_RESOLVE", site.address.to_string());
    cmd
}

fn lines(fx: &Fixture) -> Vec<Value> {
    fs::read_to_string(fx.root.join("pages").join("metadata.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is a JSON record"))
        .collect()
}

/// The latest record for each URL, as the contract reads the file.
fn records(fx: &Fixture) -> HashMap<String, Value> {
    lines(fx)
        .into_iter()
        .map(|record| (record["url"].as_str().unwrap().to_owned(), record))
        .collect()
}

const ARTICLE: &str = r#"<!doctype html><html lang="en"><head>
<meta charset="utf-8">
<title>An  article &amp; more</title>
<meta name="description" content="What the article is about.">
<meta property="og:title" content="OG title"><meta property="og:type" content="article">
<meta property="og:site_name" content="Example"><meta name="twitter:title" content="Tw title">
<link rel="canonical" href="/canonical">
<script type="application/ld+json">{"@type":["Article","https://schema.org/NewsArticle"]}</script>
</head><body><meta name="description" content="not this"></body></html>"#;

const REPO: &str = r#"<html><head><title>GitHub - owner/repo: A tool</title>
<meta name="description" content="A tool. Contribute to owner/repo development by creating an account on GitHub.">
</head><body><react-app><script type="application/json" data-target="react-app.embeddedData">
{"payload":{"topics":[{"name":"rust"},{"name":"cli"}],
"overview":{"richText":"<article><h1>repo<\/h1><p>A small tool that keeps tabs.<\/p><pre><code>cargo install repo<\/code><\/pre><\/article>"}}}
</script></react-app></body></html>"#;

fn routes(host: &str, path: &str, port: u16) -> Reply {
    match (host, path) {
        ("a.test", "/article") => {
            let mut reply = Reply::html(ARTICLE);
            reply
                .headers
                .push(("Set-Cookie", "tracker=1; Path=/".into()));
            reply
        }
        ("a.test", "/moved") => Reply::redirect(301, "/article-2#frag"),
        ("a.test", "/article-2") => Reply::html(page("Moved here")),
        ("b.test", "/app") => Reply::redirect(302, "https://b.test/login?next=/app"),
        ("c.test", "/latin") => Reply {
            status: 200,
            headers: vec![("Content-Type", "text/html; charset=ISO-8859-1".into())],
            body: b"<head><title>Caf\xe9 \x93menu\x94</title></head>".to_vec(),
            stall: None,
        },
        ("c.test", "/meta-latin") => Reply {
            status: 200,
            headers: vec![("Content-Type", "text/html".into())],
            body: b"<meta charset=windows-1252><title>na\xefve</title>".to_vec(),
            stall: None,
        },
        ("c.test", "/sjis") => Reply {
            status: 200,
            headers: vec![("Content-Type", "text/html; charset=Shift_JIS".into())],
            body: b"<title>\x93\xfa\x96\x7b</title>".to_vec(),
            stall: None,
        },
        ("d.test", "/no-head-end") => Reply::html(
            "<html><head><title>Never closed</title><meta name=description content=\"Still read\">",
        ),
        ("github.com", "/owner/repo") => Reply::html(REPO),
        ("e.test", "/giant") => {
            // Two and a half megabytes of inline script before the tags, as
            // on a video site, then a body nobody should read.
            let filler = "x".repeat(2_500_000);
            Reply::html(format!(
                "<html><head><script>var a='{filler}';</script><title>Late title</title>\
                 <meta name=description content=\"Found after the script\"></head><body>{}</body></html>",
                "y".repeat(4_000_000)
            ))
        }
        ("e.test", "/beyond-cap") => {
            let filler = "x".repeat(3_300_000);
            Reply::html(format!(
                "<html><head><script>var a='{filler}';</script><title>Too late</title></head>"
            ))
        }
        ("f.test", "/paper.pdf") => Reply {
            status: 200,
            headers: vec![("Content-Type", "application/pdf".into())],
            body: b"%PDF-1.7".to_vec(),
            stall: None,
        },
        ("g.test", "/gone") => Reply::status(404),
        ("g.test", "/members") => Reply::status(401),
        ("h.test", "/inward") => Reply::redirect(302, &format!("http://127.0.0.1:{port}/secret")),
        ("h.test", "/intranet") => Reply::redirect(302, "http://wiki/secret"),
        ("i.test", "/dashboard") => Reply::html("<head><title>Sign in to Example</title></head>"),
        ("j.test", "/slow") => Reply {
            stall: Some(Duration::from_secs(5)),
            ..Reply::html(page("Too slow"))
        },
        // A loop that moves host every hop, so pacing does not slow it down.
        (host, "/loop") => {
            let n: u32 = host
                .trim_start_matches('k')
                .trim_end_matches(".test")
                .parse()
                .unwrap_or(0);
            Reply::redirect(302, &format!("http://k{}.test/loop", n + 1))
        }
        _ => Reply::html(page(&format!("{host}{path}"))),
    }
}

const FORGOTTEN: &str = "http://z.test/forgotten";

fn forget(fx: &Fixture, url: &str) {
    fs::write(
        fx.root.join("library.json"),
        format!(r#"{{"schema_version":1,"forgotten":["{url}"]}}"#),
    )
    .unwrap();
}

#[test]
fn each_head_becomes_a_record() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://a.test/article",
            "http://a.test/moved",
            "http://c.test/latin",
            "http://c.test/meta-latin",
            "http://c.test/sjis",
            "http://d.test/no-head-end",
            "http://github.com/owner/repo",
            "http://f.test/paper.pdf",
            "http://g.test/gone",
            "http://g.test/members",
            "http://i.test/dashboard",
            "http://k.test/loop",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    assert_eq!(site.seen().iter().filter(|r| r.path == "/loop").count(), 11);
    let records = records(&fx);

    let article = &records["http://a.test/article"];
    assert_eq!(article["status"], "ok");
    assert_eq!(article["http_status"], 200);
    assert_eq!(article["title"], "An article & more");
    assert_eq!(article["description"], "What the article is about.");
    assert_eq!(article["lang"], "en");
    assert_eq!(article["canonical"], "http://a.test/canonical");
    assert_eq!(article["og"]["type"], "article");
    assert_eq!(article["og"]["site_name"], "Example");
    assert_eq!(article["twitter"]["title"], "Tw title");
    assert_eq!(
        article["jsonld_types"],
        serde_json::json!(["Article", "NewsArticle"])
    );
    assert!(article["fetched_at"].as_str().unwrap().ends_with('Z'));

    let moved = &records["http://a.test/moved"];
    assert_eq!(moved["status"], "ok");
    assert_eq!(moved["final_url"], "http://a.test/article-2");
    assert_eq!(moved["title"], "Moved here");

    assert_eq!(records["http://c.test/latin"]["title"], "Café “menu”");
    assert_eq!(records["http://c.test/meta-latin"]["title"], "naïve");
    assert_eq!(records["http://c.test/sjis"]["status"], "error");
    assert_eq!(
        records["http://c.test/sjis"]["reason"],
        "unsupported charset Shift_JIS"
    );

    let unclosed = &records["http://d.test/no-head-end"];
    assert_eq!(unclosed["status"], "ok");
    assert_eq!(unclosed["title"], "Never closed");
    assert_eq!(unclosed["description"], "Still read");

    let repo = &records["http://github.com/owner/repo"];
    assert_eq!(repo["status"], "ok");
    assert_eq!(repo["github"]["topics"], serde_json::json!(["rust", "cli"]));
    assert_eq!(
        repo["github"]["readme"],
        "repo\nA small tool that keeps tabs."
    );

    assert_eq!(records["http://f.test/paper.pdf"]["status"], "skipped");
    assert_eq!(
        records["http://f.test/paper.pdf"]["reason"],
        "not HTML (application/pdf)"
    );
    assert_eq!(records["http://g.test/gone"]["status"], "error");
    assert_eq!(records["http://g.test/gone"]["reason"], "HTTP 404");
    assert_eq!(records["http://g.test/gone"]["http_status"], 404);
    assert_eq!(records["http://g.test/members"]["status"], "behind_login");
    let dashboard = &records["http://i.test/dashboard"];
    assert_eq!(dashboard["status"], "behind_login");
    assert_eq!(dashboard["reason"], "sign-in page");
    assert_eq!(
        records["http://k.test/loop"]["reason"],
        "too many redirects"
    );
}

#[test]
fn nothing_is_sent_that_the_rules_keep_home() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    let kept_home = [
        FORGOTTEN,
        "http://192.168.1.1/router",
        "http://nas.local/share",
        "https://www.google.com/search?q=secret+plans",
        "http://l.test/callback?code=abc&state=xyz",
        "chrome://settings/",
    ];
    let mut urls = vec![
        "http://a.test/moved",
        "http://b.test/app",
        "http://h.test/inward",
        "http://h.test/intranet",
        "http://m.test/users/sign_in",
    ];
    urls.extend(kept_home);
    library(&fx, &urls);
    forget(&fx, FORGOTTEN);
    let output = enrich(&fx, &site, &[]);
    assert_success(&output);
    let records = records(&fx);

    let app = &records["http://b.test/app"];
    assert_eq!(app["status"], "behind_login");
    assert_eq!(app["reason"], "redirected to a login page");
    for inward in ["http://h.test/inward", "http://h.test/intranet"] {
        assert_eq!(records[inward]["status"], "skipped", "{inward}");
        assert_eq!(
            records[inward]["reason"], "redirected to a private network",
            "{inward}"
        );
    }
    let login = &records["http://m.test/users/sign_in"];
    assert_eq!(login["status"], "behind_login");
    assert_eq!(login["reason"], "login page, not fetched");
    for kept in kept_home {
        assert!(!records.contains_key(kept), "{kept} was recorded");
    }

    let paths = site.paths();
    for never in [
        "z.test/forgotten",
        "l.test/callback?code=abc&state=xyz",
        "m.test/users/sign_in",
        "b.test/login?next=/app",
        "127.0.0.1/secret",
        "wiki/secret",
    ] {
        assert!(!paths.iter().any(|p| p == never), "{never} was requested");
    }
    assert!(
        !paths.iter().any(|p| p.contains("google")),
        "a search page was requested"
    );
    assert!(paths.iter().any(|p| p == "a.test/article-2"), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("#frag")));
    // No cookie was sent, though every redirect set one; no referrer, and a
    // user agent that says what it is.
    for seen in site.seen() {
        assert!(!seen.headers.contains_key("cookie"), "{seen:?}");
        assert!(!seen.headers.contains_key("referer"), "{seen:?}");
        assert!(seen.headers["user-agent"].contains("knowmoretabs/"));
    }

    let out = stdout(&output);
    assert!(
        out.starts_with("enriched 5 pages in "),
        "the login page is recorded, not fetched, and counts: {out}"
    );
    assert!(
        out.contains("1 ok, 2 behind a login, 2 skipped, 0 errors; appended to "),
        "{out}"
    );
    assert!(
        out.contains("not fetched: 0 already fetched, 1 not a web page, 1 forgotten, 2 private network, 1 search results, 1 token in URL"),
        "{out}"
    );
}

#[test]
fn the_head_is_read_to_three_megabytes_and_no_further() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(&fx, &["http://e.test/giant", "http://e.test/beyond-cap"]);
    let output = enrich(&fx, &site, &[]);
    assert_success(&output);
    let records = records(&fx);
    let giant = &records["http://e.test/giant"];
    assert_eq!(giant["status"], "ok");
    assert_eq!(giant["title"], "Late title");
    assert_eq!(giant["description"], "Found after the script");
    let beyond = &records["http://e.test/beyond-cap"];
    assert_eq!(beyond["status"], "error");
    assert_eq!(beyond["reason"], "no </head> in the first 3 MB");
}

#[test]
fn dry_run_says_what_and_why_and_sends_nothing() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://a.test/article",
            "http://m.test/login",
            FORGOTTEN,
            "http://10.0.0.1/",
            "http://l.test/reset?token=abc",
        ],
    );
    forget(&fx, FORGOTTEN);
    let output = enrich(&fx, &site, &["--dry-run"]);
    assert_success(&output);
    let out = stdout(&output);
    for expected in [
        "would fetch 2 pages from 1 site:",
        "  http://a.test/article  (new)",
        "  http://m.test/login  (login page: recorded as behind_login, not fetched)",
        "would not fetch 3 pages:",
        "  http://z.test/forgotten  (forgotten)",
        "  http://10.0.0.1/  (private network)",
        "  http://l.test/reset?token=abc  (token in URL)",
        "not fetched: 0 already fetched, 1 forgotten, 1 private network, 1 token in URL",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in\n{out}");
    }
    assert!(site.seen().is_empty(), "a dry run sent {:?}", site.paths());
    assert!(!fx.root.join("pages").exists());

    let output = enrich(&fx, &site, &["--dry-run", "--json", "--limit", "1"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["fetch"].as_array().unwrap().len(), 1);
    assert_eq!(json["fetch"][0]["url"], "http://a.test/article");
    assert_eq!(json["more"], 1);
    assert_eq!(json["not_fetched"].as_array().unwrap().len(), 3);
    assert_eq!(json["counts"]["not_fetched"]["forgotten"], 1);
    assert!(site.seen().is_empty());
}

#[test]
fn limits_and_refetch_require_explicit_permission_for_every_repeat() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://a.test/article",
            "http://n1.test/",
            "http://g.test/gone",
            "http://n2.test/",
        ],
    );
    let output = enrich(&fx, &site, &["--limit", "2"]);
    assert_success(&output);
    assert_eq!(lines(&fx).len(), 2);
    assert!(stdout(&output).contains("2 pages left for another run"));

    assert_success(&enrich(&fx, &site, &[]));
    assert_eq!(lines(&fx).len(), 4);
    let requests = site.seen().len();

    // Even an error records an attempt; another request needs --refetch.
    let output = enrich(&fx, &site, &["--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["fetched"], 0);
    assert_eq!(json["errors"], serde_json::json!({}));
    assert_eq!(json["not_fetched"]["already fetched"], 4);
    assert_eq!(site.seen().len(), requests);

    let output = enrich(&fx, &site, &["--refetch", "--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["fetched"], 4);
    assert_eq!(json["ok"], 3);
    assert_eq!(lines(&fx).len(), 8, "append-only: nothing rewritten");
}

#[test]
fn one_host_gets_one_request_a_second_and_hosts_run_side_by_side() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://p.test/1",
            "http://p.test/2",
            "http://p.test/3",
            "http://q.test/1",
            "http://r.test/1",
            "http://s.test/1",
        ],
    );
    let start = Instant::now();
    assert_success(&enrich(&fx, &site, &[]));
    let elapsed = start.elapsed();
    let seen = site.seen();
    let mut same_host: Vec<Instant> = seen
        .iter()
        .filter(|s| s.host == "p.test")
        .map(|s| s.at)
        .collect();
    same_host.sort();
    assert_eq!(same_host.len(), 3);
    for pair in same_host.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(950),
            "two requests to one host {gap:?} apart"
        );
    }
    let first = seen.iter().map(|s| s.at).min().unwrap();
    for other in ["q.test", "r.test", "s.test"] {
        let at = seen.iter().find(|s| s.host == other).unwrap().at;
        assert!(
            at - first < Duration::from_millis(900),
            "{other} waited for p.test"
        );
    }
    assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
}

#[test]
fn a_page_that_never_answers_times_out_and_the_run_goes_on() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(&fx, &["http://j.test/slow", "http://a.test/article"]);
    let output = enrich_command(&fx, &site, &[])
        .env("KNOWMORETABS_TEST_TIMEOUT_MS", "500")
        .output()
        .unwrap();
    assert_success(&output);
    let records = records(&fx);
    assert_eq!(records["http://j.test/slow"]["status"], "error");
    assert_eq!(records["http://j.test/slow"]["reason"], "timeout");
    assert_eq!(records["http://a.test/article"]["status"], "ok");
    assert!(stdout(&output).contains("errors: 1 timeout"));
}

#[test]
fn an_interrupted_run_leaves_whole_lines_and_the_next_run_finishes() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    let urls: Vec<String> = (1..=8)
        .map(|n| format!("http://slowhost.test/{n}"))
        .collect();
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    library(&fx, &refs);
    let mut child = enrich_command(&fx, &site, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while lines(&fx).len() < 3 {
        assert!(Instant::now() < deadline, "no progress");
        std::thread::sleep(Duration::from_millis(50));
    }
    interrupt(&mut child);
    let written = lines(&fx);
    assert!(written.len() >= 3 && written.len() < 8, "{}", written.len());

    let output = enrich(&fx, &site, &["--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["not_fetched"]["already fetched"], written.len());
    assert_eq!(records(&fx).len(), 8);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

/// Ctrl-C where there is one; a hard kill elsewhere, which is harsher.
fn interrupt(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(not(unix))]
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_torn_tail_from_an_earlier_crash_is_reported_and_left_behind() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(&fx, &["http://a.test/article", "http://n1.test/"]);
    let pages = fx.root.join("pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        pages.join("metadata.jsonl"),
        "{\"url\":\"http://n1.test/\",\"fetched_at\":\"2026-09-24T10:00:00Z\",\"status\":\"ok\"}\n{\"url\":\"http://a.te",
    )
    .unwrap();
    let output = enrich(&fx, &site, &[]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("1 line of") && stderr(&output).contains("could not be read"),
        "{}",
        stderr(&output)
    );
    let text = fs::read_to_string(pages.join("metadata.jsonl")).unwrap();
    let last = text.lines().last().unwrap();
    assert!(
        last.starts_with("{\"url\":\"http://a.test/article\""),
        "{last}"
    );
    assert_eq!(text.lines().count(), 3, "the tear stays on its own line");
}

#[test]
fn an_empty_or_absent_archive_has_nothing_to_fetch_and_creates_nothing() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    let output = enrich(&fx, &site, &[]);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "nothing to fetch\nnot fetched: 0 already fetched\n"
    );
    assert!(!fx.root.exists());
    assert!(site.seen().is_empty());
}

#[test]
fn redirects_cannot_spend_tokens_send_credentials_or_search() {
    let fx = Fixture::new();
    let site = Site::start(|_, path, _| match path {
        "/token" => Reply::redirect(302, "http://target.test/redeem?access_token=secret"),
        "/credentials" => Reply::redirect(302, "http://owner:secret@target.test/private"),
        "/search" => Reply::redirect(302, "http://target.test/search?q=private"),
        "/entry" => Reply::redirect(302, "http://target.test/%76erify-email/one-time"),
        _ => Reply::html(page("Must not be fetched")),
    });
    library(
        &fx,
        &[
            "http://a.test/token",
            "http://b.test/credentials",
            "http://c.test/search",
            "http://d.test/entry",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    assert_eq!(site.seen().len(), 4, "{:?}", site.paths());
    for seen in site.seen() {
        assert!(!seen.headers.contains_key("authorization"));
        assert!(!seen.headers.contains_key("cookie"));
    }
    let records = records(&fx);
    assert_eq!(records["http://a.test/token"]["status"], "skipped");
    assert_eq!(records["http://b.test/credentials"]["status"], "skipped");
    assert_eq!(records["http://c.test/search"]["status"], "skipped");
    assert_eq!(records["http://d.test/entry"]["status"], "behind_login");
}

#[test]
fn a_redirect_to_a_forgotten_page_stays_home() {
    let fx = Fixture::new();
    let site = Site::start(|_, _, _| Reply::redirect(302, FORGOTTEN));
    library(&fx, &["http://a.test/start", FORGOTTEN]);
    forget(&fx, FORGOTTEN);
    assert_success(&enrich(&fx, &site, &[]));
    assert_eq!(site.paths(), ["a.test/start"]);
    assert_eq!(records(&fx)["http://a.test/start"]["status"], "skipped");
}

#[test]
fn encoded_login_and_search_paths_stay_home() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://a.test/%6cogin",
            "http://b.test/%73earch?q=secret",
            "http://owner:password@c.test/",
            "http://d.test/?%74oken=x",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    assert!(site.seen().is_empty());
    assert_eq!(
        records(&fx)["http://a.test/%6cogin"]["status"],
        "behind_login"
    );
}

#[test]
fn hostile_responses_respect_caps_and_encodings() {
    let fx = Fixture::new();
    let site = Site::start(|host, path, _| {
        if host == "github.com" {
            return Reply::html(format!(
                "{}<title>Too late</title>",
                " ".repeat(3 * 1024 * 1024 + 1)
            ));
        }
        match path {
            "/encoded" => Reply {
                headers: vec![
                    ("Content-Type", "text/html".into()),
                    ("Content-Encoding", "gzip".into()),
                ],
                ..Reply::html("<title>Not actually decoded</title>")
            },
            "/expanded" => Reply {
                headers: vec![("Content-Type", "text/html; charset=windows-1252".into())],
                ..Reply::html(vec![0x80; 1_100_000])
            },
            _ => Reply {
                headers: vec![("X-Large", "a".repeat(70_000))],
                ..Reply::html(page("Too many headers"))
            },
        }
    });
    library(
        &fx,
        &[
            "http://github.com/owner/repo",
            "http://a.test/encoded",
            "http://b.test/expanded",
            "http://c.test/headers",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    for record in records(&fx).values() {
        assert_eq!(record["status"], "error", "{record}");
    }
}

#[test]
fn redirects_share_one_timeout_budget() {
    let fx = Fixture::new();
    let site = Site::start(|_, path, _| {
        let mut reply = if path == "/start" {
            Reply::redirect(302, "http://b.test/finish")
        } else {
            Reply::html(page("Late"))
        };
        reply.stall = Some(Duration::from_millis(200));
        reply
    });
    library(&fx, &["http://a.test/start"]);
    let output = enrich_command(&fx, &site, &[])
        .env("KNOWMORETABS_TEST_TIMEOUT_MS", "300")
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(records(&fx)["http://a.test/start"]["reason"], "timeout");
}

#[test]
fn proxy_environment_and_session_headers_are_ignored() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    let proxy = Site::start(routes);
    library(&fx, &["http://a.test/article", "http://a.test/moved"]);
    let mut cmd = enrich_command(&fx, &site, &[]);
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env(key, format!("http://owner:secret@{}", proxy.address));
    }
    cmd.env("NO_PROXY", "").env("no_proxy", "");
    assert_success(&cmd.output().unwrap());
    assert!(proxy.seen().is_empty());
    assert_eq!(site.seen().len(), 3);
    for seen in site.seen() {
        for key in ["authorization", "proxy-authorization", "cookie", "referer"] {
            assert!(!seen.headers.contains_key(key), "{key}");
        }
    }
}

#[test]
fn equivalent_hosts_share_pacing_and_unicode_hosts_use_idna() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://p.test:8080/1",
            "http://p.test.:8081/2",
            "http://bücher.test/page",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    let seen = site.seen();
    let mut times: Vec<Instant> = seen
        .iter()
        .filter(|r| r.host.trim_end_matches('.') == "p.test")
        .map(|r| r.at)
        .collect();
    times.sort();
    assert_eq!(times.len(), 2);
    assert!(times[1] - times[0] >= Duration::from_millis(950));
    assert!(seen.iter().any(|r| r.host == "xn--bcher-kva.test"));
}

#[test]
fn one_time_link_aliases_stay_home() {
    let fx = Fixture::new();
    let site = Site::start(routes);
    library(
        &fx,
        &[
            "http://a.test/callback?authCode=one-use",
            "http://b.test/resource?accessKey=secret",
            "http://c.test/password/reset/one-use",
            "http://d.test/verify_email/one-use",
            "http://e.test/confirmation/one-use",
        ],
    );
    assert_success(&enrich(&fx, &site, &[]));
    assert!(site.seen().is_empty(), "{:?}", site.paths());
}
