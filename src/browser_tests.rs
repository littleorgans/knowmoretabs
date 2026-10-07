//! Synthetic checks of browser readiness, launch, attach, tab ownership, loading
//! and page validation.
//!
//! slice: content
//! why: Keeping the browser contracts beside one another leaves its runtime
//!      lifecycle readable without changing the checks or their private access.

use std::collections::BTreeSet;

use serde_json::json;

use super::*;
use crate::content_store::{Access, Status};
use crate::content_test::{PEER_ROUTE, Peer};
use crate::tools::Table;

#[test]
fn readiness_is_the_chosen_binary_unless_the_owner_said_no() {
    let mut table = Table::default();
    table
        .browsers
        .insert(platform::CHROME, PathBuf::from("/opt/Google Chrome"));
    assert_eq!(
        Readiness::check(&table, platform::CHROME, false),
        Readiness::Ready(PathBuf::from("/opt/Google Chrome"))
    );
    assert_eq!(
        Readiness::check(&table, platform::CHROME, true),
        Readiness::Off
    );
    assert_eq!(
        Readiness::check(&table, platform::BRAVE, false),
        Readiness::Missing("no brave binary found".to_owned())
    );
    assert_eq!(
        Readiness::check(&table, "arc", false),
        Readiness::Missing("unknown browser \"arc\"".to_owned())
    );
    assert_eq!(name(Path::new("/opt/Google Chrome")), "Google Chrome");
}

#[test]
fn the_launch_uses_a_scratch_profile_the_relay_and_no_udp_around_it() {
    let flags: Vec<String> = flags(Path::new("/scratch/knowmoretabs-browser-x"), 4321)
        .into_iter()
        .map(|flag| flag.into_string().unwrap())
        .collect();
    for expected in [
        "--headless",
        "--remote-debugging-port=0",
        "--user-data-dir=/scratch/knowmoretabs-browser-x",
        "--disable-background-networking",
        "--proxy-server=socks5://127.0.0.1:4321",
        "--proxy-bypass-list=<-loopback>",
        "--webrtc-ip-handling-policy=disable_non_proxied_udp",
    ] {
        assert!(flags.iter().any(|flag| flag == expected), "{expected}");
    }
    let features = flags
        .iter()
        .find_map(|flag| flag.strip_prefix("--disable-features="))
        .unwrap();
    assert!(features.split(',').any(|f| f == "PreconnectToSearch"));
    assert!(features.split(',').any(|f| f == "OptimizationHints"));
    assert!(
        !flags.iter().any(|flag| flag.starts_with("--profile")),
        "never the owner's profile"
    );
    assert_eq!(flags.last().map(String::as_str), Some("about:blank"));
}

#[test]
fn the_endpoint_file_needs_a_port_and_the_browser_route() {
    assert_eq!(
        endpoint("9222\n/devtools/browser/abc-123\n"),
        Some((9222, "/devtools/browser/abc-123".to_owned()))
    );
    for bad in [
        "",
        "9222\n",
        "0\n/devtools/browser/x",
        "70000\n/devtools/browser/x",
        "9222\n/devtools/page/x",
    ] {
        assert_eq!(endpoint(bad), None, "{bad:?}");
    }
}

fn event(frame: &str, loader: &str, name: &str) -> cdp::Lifecycle {
    cdp::Lifecycle {
        frame: frame.to_owned(),
        loader: loader.to_owned(),
        name: name.to_owned(),
    }
}

/// Waits on `events` in order, then on nothing: the deadline. Returns
/// whether the page loaded, and every deadline asked for.
fn heard(events: Vec<cdp::Lifecycle>) -> (bool, Vec<Instant>) {
    let mut lifecycle = Lifecycle::new("F".to_owned(), "L1".to_owned());
    let mut events = events.into_iter();
    let mut asked = Vec::new();
    let deadline = Instant::now() + LOAD_WAIT;
    let loaded = lifecycle
        .wait(
            |until| {
                asked.push(until);
                Ok(events.next())
            },
            deadline,
        )
        .unwrap();
    assert!(asked.iter().all(|&until| until == deadline), "never reset");
    (loaded, asked)
}

#[test]
fn loading_is_heard_for_the_navigations_frame_and_document_only() {
    let (loaded, asked) = heard(vec![
        event("F", "BLANK", "networkIdle"),
        event("CHILD", "L1", "load"),
        event("F", "L1", "load"),
        event("F", "L1", "networkIdle"),
        event("F", "L1", "unreached"),
    ]);
    assert!(loaded);
    assert_eq!(asked.len(), 4, "done at load and idle");

    let (loaded, _) = heard(vec![
        event("F", "L1", "load"),
        event("F", "L2", "init"),
        event("F", "L1", "networkIdle"),
    ]);
    assert!(!loaded, "a script navigation starts a new document");

    let (loaded, asked) = heard(vec![event("F", "L1", "load")]);
    assert!(loaded, "loaded but never idle is read at the deadline");
    assert_eq!(asked.len(), 2);
}

fn read(url: &str, mime: &str, status: u16, html: Option<String>) -> Read {
    Read {
        url: url.to_owned(),
        mime: mime.to_owned(),
        status,
        html,
    }
}

/// Why a page read back as `read` is not rendered, `passing` when it
/// would be tried again, or the status it was read with.
fn verdict(read: Read) -> String {
    let fetcher = Fetcher::new(&BTreeSet::new());
    let navigated = Url::parse("https://a.test/page").unwrap();
    match after(
        &fetcher,
        Tier::Headless,
        "https://a.test/page",
        &navigated,
        read,
        false,
    ) {
        Ok(Ok(Outcome::NotRendered(why))) => why,
        Ok(Ok(Outcome::Refused(refusal))) => refusal.reason(),
        Ok(Ok(Outcome::Read(capture))) => format!("read {:?}", capture.line.status),
        Ok(Ok(Outcome::SignIn(line))) => format!("sign-in {:?}", line.status),
        Ok(Err(Broken(why))) => format!("broken {why}"),
        Err(_) => "passing".to_owned(),
    }
}

#[test]
fn a_read_page_is_checked_where_it_ended_then_status_then_type_then_size() {
    let html = || Some(format!("<main><p>{}</p></main>", "Text. ".repeat(400)));
    let page = "https://a.test/page";
    assert_eq!(verdict(read(page, "text/html", 200, html())), "read Ok");
    assert_eq!(
        verdict(read("https://a.test/login", "text/html", 200, html())),
        "sign-in BehindLogin"
    );
    assert_eq!(
        verdict(read(
            "chrome-error://chromewebdata/",
            "text/html",
            200,
            html()
        )),
        "redirected away from the web"
    );
    assert_eq!(
        verdict(read("http://192.168.1.1/", "text/html", 200, html())),
        "redirected to a private network"
    );
    assert_eq!(
        verdict(read(page, "application/pdf", 404, None)),
        "HTTP 404",
        "status before type"
    );
    assert_eq!(verdict(read(page, "text/html", 503, html())), "passing");
    assert_eq!(
        verdict(read(page, "text/html", 0, html())),
        "no HTTP status"
    );
    assert_eq!(
        verdict(read(page, "application/pdf", 200, None)),
        "not HTML (application/pdf)"
    );
    assert_eq!(
        verdict(read(page, "text/html", 200, None)),
        "page too large"
    );
    // Under the cap in UTF-16 units, as the expression counts, and over
    // it in UTF-8 bytes, as Rust decodes it.
    let wide = "é".repeat(BODY_CAP / 2 + 1);
    assert!(wide.encode_utf16().count() <= BODY_CAP && wide.len() > BODY_CAP);
    assert_eq!(
        verdict(read(page, "text/html", 200, Some(wide))),
        "page too large"
    );
}

#[test]
fn a_page_that_ends_on_a_sign_in_screen_says_where() {
    let fetcher = Fetcher::new(&BTreeSet::new());
    let navigated = Url::parse("https://a.test/page").unwrap();
    let landed = read("https://a.test/account?next=/page", "text/html", 200, None);
    let Ok(Ok(Outcome::SignIn(line))) = after(
        &fetcher,
        Tier::Headless,
        "https://a.test/page",
        &navigated,
        landed,
        false,
    ) else {
        panic!("a sign-in screen is read as one");
    };
    assert_eq!(line.status, Status::BehindLogin);
    assert_eq!(line.tier, Some(Tier::Headless));
    assert_eq!(
        line.final_url.as_deref(),
        Some("https://a.test/account?next=/page")
    );
}

#[test]
fn an_attached_run_speaks_only_to_the_tabs_it_opens_and_closes_each_one() {
    let peer = Peer::start(None);
    let browser = Browser::attach(peer.port, PEER_ROUTE, "Google Chrome".to_owned()).unwrap();
    assert_eq!(browser.tier(), Tier::SignedIn);
    let fetcher = Fetcher::new(&BTreeSet::new());
    for raw in ["https://a.test/one", "https://b.test/two"] {
        let url = Url::parse(raw).unwrap();
        let Ok(Outcome::Read(capture)) = browser.render(&fetcher, raw, &url, false) else {
            panic!("{raw} is read");
        };
        assert_eq!(
            (capture.line.status, capture.line.tier, capture.line.access),
            (Status::Ok, Some(Tier::SignedIn), Some(Access::SignedIn))
        );
    }
    let fails = Url::parse("https://c.test/fails").unwrap();
    assert!(matches!(
        browser.render(&fetcher, fails.as_str(), &fails, false),
        Ok(Outcome::NotRendered(why)) if why == "ERR_ABORTED"
    ));
    // A tab an error or a panic leaves behind is closed when dropped.
    drop(Tab::open(&browser).unwrap());
    drop(browser);

    let received: Vec<(String, Value, Option<String>)> = peer
        .received()
        .into_iter()
        .map(|message| {
            (
                message["method"].as_str().unwrap().to_owned(),
                message["params"].clone(),
                message["sessionId"].as_str().map(str::to_owned),
            )
        })
        .collect();
    let browser_call = |method: &str, params: Value| (method.to_owned(), params, None);
    let open = |n: u32| {
        vec![
            browser_call(
                "Target.createTarget",
                json!({"url": "about:blank", "hidden": true, "background": true}),
            ),
            browser_call(
                "Target.attachToTarget",
                json!({"targetId": format!("T{n}"), "flatten": true}),
            ),
        ]
    };
    let close = |n: u32| browser_call("Target.closeTarget", json!({"targetId": format!("T{n}")}));
    let page = |n: u32, url: &str, read: bool| {
        let session = Some(format!("S{n}"));
        let on_page = |method: &str, params: Value| (method.to_owned(), params, session.clone());
        let mut said = open(n);
        said.push(on_page("Page.enable", json!({})));
        said.push(on_page(
            "Page.setLifecycleEventsEnabled",
            json!({"enabled": true}),
        ));
        said.push(on_page("Page.navigate", json!({"url": url})));
        if read {
            said.push(on_page(
                "Runtime.evaluate",
                json!({"expression": cdp::EXTRACT, "returnByValue": true}),
            ));
        }
        said.push(close(n));
        said
    };
    let mut expected = vec![browser_call("Browser.getVersion", json!({}))];
    expected.extend(page(1, "https://a.test/one", true));
    expected.extend(page(2, "https://b.test/two", true));
    expected.extend(page(3, "https://c.test/fails", false));
    expected.extend(open(4));
    expected.push(close(4));
    assert_eq!(received, expected);
    for never in [
        "Browser.close",
        "Target.createBrowserContext",
        "Target.disposeBrowserContext",
        "Target.getTargets",
        "Target.setAutoAttach",
    ] {
        assert!(
            received.iter().all(|(method, ..)| method != never),
            "{never}"
        );
    }
}

#[test]
fn an_owner_who_is_not_running_or_does_not_allow_is_told_which() {
    let closed = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let attached = |port| Browser::attach(port, PEER_ROUTE, "Google Chrome".to_owned()).err();
    assert_eq!(attached(port), Some(Unavailable::NotRunning));
    // Denied: the browser closes the connection instead of answering.
    let refusing = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = refusing.local_addr().unwrap().port();
    let denier = std::thread::spawn(move || drop(refusing.accept().unwrap()));
    assert_eq!(attached(port), Some(Unavailable::NotAllowed));
    denier.join().unwrap();
    let peer = Peer::start(Some("Browser.getVersion"));
    assert_eq!(attached(peer.port), Some(Unavailable::NotAllowed));
    assert_eq!(peer.received().len(), 1, "nothing after the version");
}
