//! Domain filtering and added page checks for the library contract.
//!
//! slice: library, add
//! why: Keeping URL boundary cases beside their owner leaves the production
//!      builder easier to navigate without duplicating fixtures or helpers.

use super::*;
use crate::content_test::{saved, state};
use crate::jsonl::Appender;

#[test]
fn domains_use_url_authority_and_keep_nonlocal_urls() {
    for raw in [
        "file:///tmp/a",
        "HTTP://LOCALHOST:8080/a",
        "http://127.0.0.1/a",
        "http://localhost./",
        // The same machine, spelled differently.
        "http://[::1]:8080/a",
        "http://127.0.0.2/a",
        "http://0.0.0.0:3000/a",
        "http://app.localhost/a",
    ] {
        assert_eq!(public_domain(raw), None, "{raw}");
    }
    for (raw, domain) in [
        ("https://user:pass@WWW.Example.test:8443/a", "example.test"),
        ("http://localhost.example.test/a", "localhost.example.test"),
        ("https://example.test/localhost", "example.test"),
        ("http://[2001:db8::1]:80/", "[2001:db8::1]"),
        ("about:blank", ""),
        ("not a URL", ""),
    ] {
        assert_eq!(public_domain(raw).as_deref(), Some(domain));
    }
}

#[test]
fn an_added_page_carries_its_own_add_time_and_a_captured_page_none() {
    const CAPTURED: &str = "https://captured.test/a";
    const FIRST: &str = "https://first.test/b";
    const SECOND: &str = "https://second.test/c";
    let dir = tempfile::tempdir().unwrap();
    saved(dir.path(), &[CAPTURED]);
    let mut log = Appender::open(dir.path(), intake::path(dir.path())).unwrap();
    for (url, at) in [
        (FIRST, "2026-10-08T09:00:00Z"),
        (SECOND, "2026-10-09T10:00:00Z"),
    ] {
        log.append(&intake::Line::new(url, at.parse().unwrap(), None))
            .unwrap();
    }
    let loaded = load(&Archive::open(dir.path()).unwrap()).unwrap();
    let library = build(
        &loaded.snapshots,
        &BTreeMap::new(),
        &state(&[]),
        &HashMap::new(),
        Shape::Serve,
    );
    let pages = &serde_json::to_value(&library).unwrap()["pages"];
    let added: HashMap<&str, Option<&str>> = pages
        .as_array()
        .unwrap()
        .iter()
        .map(|page| (page["url"].as_str().unwrap(), page["added_at"].as_str()))
        .collect();
    assert_eq!(
        added,
        HashMap::from([
            (CAPTURED, None),
            (FIRST, Some("2026-10-08T09:00:00Z")),
            (SECOND, Some("2026-10-09T10:00:00Z")),
        ])
    );
}
