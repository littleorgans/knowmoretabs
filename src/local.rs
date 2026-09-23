//! What counts as this machine: the one definition that `save` leaves out and
//! the library hides.
//!
//! slice: capture, library
//! why: A development server, or this tool's own library page, is a tab on
//!      this machine, not a page anyone can return to. Capture leaves such
//!      tabs out of a snapshot and the library hides the ones older snapshots
//!      still hold; two copies of the rule would drift apart and a tab would
//!      be saved by one and hidden by the other.

use url::{Host, Url};

/// `localhost` and its RFC 6761 subdomains, the loopback ranges, and the
/// unspecified addresses a development server binds to, under any spelling.
pub fn is_this_machine(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip.is_loopback() || ip.is_unspecified(),
        Host::Ipv6(ip) => ip.is_loopback() || ip.is_unspecified(),
    }
}

/// Whether a tab's URL points at this machine. An unparseable URL is not
/// treated as local: when in doubt, the tab is kept.
pub fn is_this_machine_url(raw: &str) -> bool {
    Url::parse(raw)
        .ok()
        .and_then(|url| url.host().map(|host| is_this_machine(&host)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_of_this_machine_is_local() {
        for raw in [
            "http://localhost:5173/",
            "HTTP://LOCALHOST:8080/a",
            "http://localhost./",
            "http://app.localhost/a",
            "http://127.0.0.1:7878/#pages",
            "http://127.0.0.2/a",
            "http://0.0.0.0:3000/a",
            "http://[::1]:8080/a",
            "http://[::]:8080/a",
        ] {
            assert!(is_this_machine_url(raw), "{raw}");
        }
    }

    #[test]
    fn other_hosts_and_odd_urls_are_kept() {
        for raw in [
            "https://example.test/localhost",
            "http://localhost.example.test/a",
            "http://192.168.1.10/router",
            "http://10.0.0.1/",
            "file:///tmp/a.pdf",
            "chrome://settings",
            "about:blank",
            "not a url",
        ] {
            assert!(!is_this_machine_url(raw), "{raw}");
        }
    }
}
