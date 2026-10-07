//! Domain filtering checks for the library contract.
//!
//! slice: library
//! why: Keeping URL boundary cases beside their owner leaves the production
//!      builder easier to navigate without duplicating fixtures or helpers.

use super::*;

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
