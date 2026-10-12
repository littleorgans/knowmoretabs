//! The key a new page is kept under: its address less the parts that never
//! name a different page.
//!
//! slice: add
//! why: One page reached by a trailing slash, an anchor or a link that says
//!      where the click came from is still one page, and should be one key.
//!      A key is the exact stored string, so the rule only shapes keys as
//!      they are minted; no stored key is rewritten. It removes exactly those
//!      parts and keeps every other byte, so case, encoding, order, `www.`
//!      and the scheme stay as written. A fragment that routes an app page
//!      (`#/`, `#!`) names the page and stays. Anything that is not a plain
//!      web address is left as it is, so the rule applies to every legacy key
//!      too. This is the one place the rule lives.

use std::borrow::Cow;

use url::{Url, form_urlencoded};

use crate::guard;

/// Query names that only say where a click came from; any name starting
/// `utm_` is one too. Compared lowercased, after form decoding.
const TRACKING: [&str; 17] = [
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "ttclid",
    "twclid",
    "gbraid",
    "wbraid",
    "yclid",
    "mc_cid",
    "mc_eid",
    "igshid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "vero_id",
    "vero_conv",
];

/// The key a new page is kept under; `raw` itself, borrowed, when no rule
/// applies. Idempotent.
pub fn clean(raw: &str) -> Cow<'_, str> {
    let Some(path_at) = path_start(raw) else {
        return Cow::Borrowed(raw);
    };
    let (head, rest) = raw.split_at(path_at);
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let mut key = String::with_capacity(raw.len() + 1);
    key.push_str(head);
    key.push_str(match path.trim_end_matches('/') {
        "" => "/",
        path => path,
    });
    if let Some(query) = query.and_then(clean_query) {
        key.push('?');
        key.push_str(&query);
    }
    if let Some(route) = fragment.filter(|f| f.starts_with(['/', '!'])) {
        key.push('#');
        key.push_str(route);
    }
    if key == raw {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(key)
    }
}

/// Where the path starts when `raw` is a web address in plain form,
/// `scheme://authority path [?query] [#fragment]`: one the URL parser takes
/// as written rather than repairs.
fn path_start(raw: &str) -> Option<usize> {
    if raw.contains(|c: char| c == '\\' || c.is_ascii_control()) || raw.trim() != raw {
        return None;
    }
    let url = Url::parse(raw).ok().filter(guard::is_web)?;
    let (scheme, after) = raw.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(url.scheme()) {
        return None;
    }
    let authority = after.find(['/', '?', '#']).unwrap_or(after.len());
    (authority > 0).then_some(scheme.len() + "://".len() + authority)
}

/// The query less its tracking parameters, byte for byte when it has none;
/// `None` when nothing is left of it.
fn clean_query(query: &str) -> Option<Cow<'_, str>> {
    let tracking = |segment: &str| {
        form_urlencoded::parse(segment.as_bytes())
            .next()
            .is_some_and(|(name, _)| {
                let name = name.to_ascii_lowercase();
                name.starts_with("utm_") || TRACKING.contains(&name.as_str())
            })
    };
    if !query.split('&').any(tracking) {
        return Some(Cow::Borrowed(query));
    }
    let kept: Vec<&str> = query
        .split('&')
        .filter(|segment| !segment.is_empty() && !tracking(segment))
        .collect();
    (!kept.is_empty()).then(|| Cow::Owned(kept.join("&")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_follows_the_three_rules() {
        for (raw, key) in [
            // Trailing slashes; the root keeps one.
            ("https://a.test/x/", "https://a.test/x"),
            ("https://a.test/x//", "https://a.test/x"),
            ("https://a.test", "https://a.test/"),
            ("https://a.test/", "https://a.test/"),
            ("https://a.test//", "https://a.test/"),
            ("https://a.test/x/?id=1", "https://a.test/x?id=1"),
            ("https://a.test?id=1", "https://a.test/?id=1"),
            // The fragment, unless it routes an app page.
            ("https://a.test/x#top", "https://a.test/x"),
            ("https://a.test/x#", "https://a.test/x"),
            ("https://a.test/x#a?utm_source=n", "https://a.test/x"),
            ("https://a.test/x/#/doc/2", "https://a.test/x#/doc/2"),
            ("https://a.test/#!/inbox", "https://a.test/#!/inbox"),
            // Tracking parameters.
            ("https://a.test/x?utm_source=n", "https://a.test/x"),
            (
                "https://a.test/x?utm_source=n&id=7&fbclid=q&utm_medium=m",
                "https://a.test/x?id=7",
            ),
            (
                "https://a.test/x?utm%5Fsource=n&id=7",
                "https://a.test/x?id=7",
            ),
            (
                "https://a.test/x?UTM_Campaign=n&id=7",
                "https://a.test/x?id=7",
            ),
            ("https://a.test/x?Fbclid=n", "https://a.test/x"),
            (
                "https://a.test/x?&utm_source=n&id=7&",
                "https://a.test/x?id=7",
            ),
            (
                "https://a.test/x?id=1&&utm_source=n",
                "https://a.test/x?id=1",
            ),
            ("https://a.test/x?utm_source=n&", "https://a.test/x"),
            ("https://a.test/x/?gclid=n#top", "https://a.test/x"),
            // Everything else stays as written.
            (
                "https://a.test/x?ref=n&source=n&s=n&si=n&utm=n&xutm_source=n",
                "https://a.test/x?ref=n&source=n&s=n&si=n&utm=n&xutm_source=n",
            ),
            (
                "https://a.test/x?b=%41&a=1+2&&c",
                "https://a.test/x?b=%41&a=1+2&&c",
            ),
            ("https://a.test/x?", "https://a.test/x?"),
            ("https://a.test/item?id=7", "https://a.test/item?id=7"),
            ("https://a.test/a%2F", "https://a.test/a%2F"),
            ("https://a.test/a%23b", "https://a.test/a%23b"),
            ("HTTPS://A.test/X/", "HTTPS://A.test/X"),
            ("http://www.a.test/x/", "http://www.a.test/x"),
            (
                "https://u:p@a.test:8443/x/?utm_source=n",
                "https://u:p@a.test:8443/x",
            ),
            // Not a plain web address: left as it is.
            ("ftp://a.test/x/", "ftp://a.test/x/"),
            ("chrome://settings/", "chrome://settings/"),
            ("file:///tmp/x/", "file:///tmp/x/"),
            ("about:blank#x", "about:blank#x"),
            ("https:\\\\a.test\\x/", "https:\\\\a.test\\x/"),
            ("http:a.test/x/", "http:a.test/x/"),
            ("https:///a.test/x/", "https:///a.test/x/"),
            ("https://a.test/x/\t", "https://a.test/x/\t"),
            ("https://a.test/x\t/", "https://a.test/x\t/"),
            (" https://a.test/x/", " https://a.test/x/"),
            ("not a url/", "not a url/"),
            ("", ""),
        ] {
            assert_eq!(clean(raw), key, "{raw:?}");
            assert_eq!(clean(key), key, "{raw:?}: idempotent");
        }
    }

    #[test]
    fn an_address_no_rule_touches_is_borrowed() {
        for raw in ["https://a.test/x?id=7", "https://a.test/", "chrome://x/"] {
            assert!(matches!(clean(raw), Cow::Borrowed(_)), "{raw}");
        }
    }
}
