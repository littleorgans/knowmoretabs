//! Regression checks for `fetch`.
//!
//! slice: enrich, content
//! why: A loopback server and synthetic resolvers exercise the guarded GET, its redirect hop checks and its refusals without reaching any real site.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;

use super::*;
use crate::metadata_fetch::HEAD_CAP;

#[cfg(not(debug_assertions))]
#[test]
fn release_hooks_are_absent() {
    assert_eq!(test_address(), None);
    assert_eq!(test_timeout(), None);
}

#[test]
fn the_pacer_spaces_one_host_and_leaves_others_alone() {
    let pacer = Arc::new(Pacer::new(Duration::from_millis(200)));
    let start = Instant::now();
    pacer.wait("a.test", start + TIMEOUT);
    pacer.wait("b.test", start + TIMEOUT);
    assert!(start.elapsed() < Duration::from_millis(150));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let pacer = Arc::clone(&pacer);
            std::thread::spawn(move || pacer.wait("a.test", start + TIMEOUT))
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400),
        "three requests to one host need two intervals, took {elapsed:?}"
    );
}

/// Waits for `a.test` on a simulated clock, which only `sleep` moves,
/// and returns when the request was released.
fn released(pacer: &Pacer, clock: &Cell<Instant>, sleep: impl FnMut(Duration)) -> Instant {
    assert!(pacer.wait_on("a.test", clock.get() + TIMEOUT, || clock.get(), sleep));
    clock.get()
}

/// A sleep on `clock` that wakes `late` after it should.
fn oversleep(clock: &Cell<Instant>, late: Duration) -> impl FnMut(Duration) + '_ {
    move |pause| clock.set(clock.get() + pause + late)
}

#[test]
fn a_late_wakeup_never_brings_the_next_request_closer() {
    let interval = Duration::from_millis(200);
    let pacer = Pacer::new(interval);
    let clock = Cell::new(Instant::now());
    let first = released(&pacer, &clock, oversleep(&clock, Duration::ZERO));
    let second = released(
        &pacer,
        &clock,
        oversleep(&clock, Duration::from_millis(150)),
    );
    let third = released(&pacer, &clock, oversleep(&clock, Duration::ZERO));
    assert_eq!(second - first, Duration::from_millis(350));
    assert_eq!(third - second, interval, "paced from the late release");
}

#[test]
fn a_waiter_overtaken_while_asleep_waits_a_full_interval_after() {
    let interval = Duration::from_millis(200);
    let pacer = Pacer::new(interval);
    let clock = Cell::new(Instant::now());
    let first = released(&pacer, &clock, oversleep(&clock, Duration::ZERO));
    let mut ahead = None;
    let behind = released(&pacer, &clock, |pause| {
        if ahead.is_none() {
            // Asleep with the earlier slot booked: the waiter booked after
            // it goes first, then this one wakes 50 ms after that.
            ahead = Some(released(&pacer, &clock, oversleep(&clock, Duration::ZERO)));
            clock.set(clock.get() + Duration::from_millis(50));
        } else {
            clock.set(clock.get() + pause);
        }
    });
    let ahead = ahead.unwrap();
    assert_eq!(ahead - first, 2 * interval);
    assert_eq!(behind - ahead, interval, "paced from the one ahead");
}

#[test]
fn slowing_down_doubles_one_host_interval_up_to_the_cap() {
    let pacer = Pacer::new(Duration::from_secs(1));
    let interval = |host: &str| {
        pacer
            .hosts
            .lock()
            .unwrap()
            .get(host)
            .map(|pace| pace.interval)
    };
    pacer.slow_down("a.test");
    assert_eq!(interval("a.test"), Some(Duration::from_secs(2)));
    for _ in 0..5 {
        pacer.slow_down("a.test");
    }
    assert_eq!(interval("a.test"), Some(MAX_PACE));
    assert_eq!(interval("b.test"), None);
    pacer.wait("b.test", Instant::now() + TIMEOUT);
    assert_eq!(interval("b.test"), Some(Duration::from_secs(1)));
}

#[test]
fn the_queue_bound_covers_every_worker_at_the_slowest_pace() {
    let workers = u32::try_from(crate::targets::WORKERS).unwrap();
    assert!(QUEUE >= MAX_PACE * workers);
}

#[test]
fn reading_stops_at_the_end_of_the_head_or_the_cap() {
    let head = format!("<head><script>{}</script></HEAD>", "x".repeat(200_000));
    let page = format!("{head}<body>{}</body>", "y".repeat(5_000_000));
    let read = read_head(page.as_bytes(), HEAD_CAP, true).unwrap();
    assert!(
        read.len() >= head.len() && read.len() < head.len() + 64 * 1024,
        "read {} bytes of a {}-byte head",
        read.len(),
        head.len()
    );
    assert_eq!(
        read_head(page.as_bytes(), HEAD_CAP, false).unwrap().len(),
        HEAD_CAP
    );
    let body = "z".repeat(HEAD_CAP + 10);
    assert_eq!(
        read_head(body.as_bytes(), HEAD_CAP, true).unwrap().len(),
        HEAD_CAP
    );
}

#[test]
fn head_boundaries_inside_markup_do_not_truncate_the_response() {
    for prefix in [
        "<script>const x = '</head>';",
        "<!-- </head>",
        "<meta content='</head>",
        "<script>const x = '</scripture></head>';",
    ] {
        let close = if prefix.starts_with("<script") {
            "</script>"
        } else if prefix.starts_with("<!--") {
            "-->"
        } else {
            "'>"
        };
        let page = format!(
            "<head>{prefix}{}{close}<title>Found</title></head>{}",
            "x".repeat(100_000),
            "y".repeat(100_000)
        );
        let bytes = read_head(page.as_bytes(), HEAD_CAP, true).unwrap();
        assert_eq!(
            head::scan(&String::from_utf8(bytes).unwrap())
                .title
                .as_deref(),
            Some("Found"),
            "{prefix}"
        );
    }
}

/// A server that accepts, reads the request, and says nothing.
fn silent_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            held.push(stream);
        }
    });
    address
}

/// A server that answers every request with an empty 204.
fn answering_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = stream.write_all(
                b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
        }
    });
    address
}

#[test]
fn a_host_queue_longer_than_the_timeout_does_not_spend_it() {
    let fetcher = Fetcher::with(
        Duration::from_millis(300),
        Some(answering_server()),
        Duration::from_millis(600),
    );
    for _ in 0..2 {
        let status = fetcher
            .get("http://queue.test/", ACCEPT_HTML)
            .map(|r| r.status);
        assert_eq!(status, Ok(204));
    }
}

#[test]
fn the_caller_chooses_the_accept_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut accept = Vec::new();
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("accept:") {
                accept.push(value.trim().to_owned());
            }
            line.clear();
        }
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n");
        accept
    });
    let fetcher = Fetcher::with(Duration::from_secs(5), Some(address), PACE);
    let response = fetcher.get("http://json.test/", "application/json").ok();
    assert_eq!(response.map(|r| r.status), Some(204));
    assert_eq!(server.join().unwrap(), ["application/json"]);
}

#[test]
fn a_server_that_never_answers_is_a_timeout() {
    let fetcher = Fetcher::with(Duration::from_millis(300), Some(silent_server()), PACE);
    let start = Instant::now();
    let refusal = fetcher.get("http://slow.test/", ACCEPT_HTML).err();
    assert_eq!(refusal, Some(Refusal::Failed("timeout".to_owned())));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_server_that_stalls_inside_the_head_is_a_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
            line.clear();
        }
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 100000\r\n\r\n<head><title>x",
        );
        std::thread::sleep(Duration::from_secs(3));
    });
    let fetcher = Fetcher::with(Duration::from_millis(400), Some(address), PACE);
    let Ok(mut response) = fetcher.get("http://stall.test/", ACCEPT_HTML) else {
        panic!("the head of the response arrived");
    };
    assert_eq!(response.read(HEAD_CAP, true), Err("timeout".to_owned()));
}

#[test]
fn without_a_test_address_loopback_names_are_refused_at_the_connect() {
    // `localhost` resolves to loopback everywhere; the resolver, not the
    // URL rules, is what is exercised when the URL check is bypassed.
    let fetcher = Fetcher::with(Duration::from_secs(5), None, PACE);
    let config = Config::builder().build();
    let err = PublicOnly { test_address: None }
        .resolve(
            &"http://localhost:9/".parse().unwrap(),
            &config,
            NextTimeout {
                after: ureq::unversioned::transport::time::Duration::NotHappening,
                reason: ureq::Timeout::Resolve,
            },
        )
        .unwrap_err();
    assert_eq!(refusal(&err), Refusal::PrivateAddress);
    assert_eq!(
        fetcher.get("http://localhost:9/", ACCEPT_HTML).err(),
        Some(Refusal::PrivateNetwork { redirected: false })
    );
}

#[test]
fn a_connection_is_refused_where_a_request_would_be() {
    let fetcher = Fetcher::with(Duration::from_secs(5), None, PACE);
    let deadline = Instant::now() + Duration::from_secs(5);
    for (host, raw) in [
        ("127.0.0.1", "http://127.0.0.1:9/"),
        ("localhost", "http://localhost:9/"),
        ("::1", "http://[::1]:9/"),
        ("printer.local", "http://printer.local:9/"),
        ("intranet", "http://intranet:9/"),
    ] {
        let refused = connect_public(host, 9, deadline, None).err();
        assert_eq!(refused, Some(Refusal::PrivateNetwork { redirected: false }));
        assert_eq!(refused, fetcher.get(raw, ACCEPT_HTML).err(), "{host}");
    }
    // The name rule passed over: what `localhost` resolves to is refused.
    let url = Url::parse("http://localhost:9/").unwrap();
    assert_eq!(resolve_public(&url, 9).err(), Some(Refusal::PrivateAddress));
    let public: SocketAddr = "93.184.216.34:80".parse().unwrap();
    let inward: SocketAddr = "10.0.0.1:80".parse().unwrap();
    assert!(all_public(&[public]));
    assert!(
        !all_public(&[public, inward]),
        "one inward address among public ones"
    );
}
