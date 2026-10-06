//! What `enrich` keeps of a page: its head, read through the guarded
//! fetcher and turned into one metadata line, success or not.
//!
//! slice: enrich
//! why: The fetcher is shared by every network command and knows nothing
//!      of what a response means, while the 7b contract pins exactly what an
//!      `enrich` line records for each outcome: a 401 is behind a login, a
//!      page that is only a sign-in form is too, anything not HTML is
//!      skipped, and a public GitHub repository's own page is read past its
//!      head for its topics and README. That reading lives here, beside the
//!      caps that keep it to the head.

use url::Url;

use crate::fetch::{self, Fetcher, Refusal, Response};
use crate::github;
use crate::head::{self, Head};
use crate::metadata_writer::{Line, Outcome};

/// `YouTube`'s meta tags start about 0.7 MB into its pages.
pub const HEAD_CAP: usize = 3 * 1024 * 1024;
/// Longest meta value kept; a description is a sentence or two.
const VALUE_CAP: usize = 2000;

/// Fetches one page's head and says what came of it. Never fails: a
/// failure is a record too.
pub fn fetch(fetcher: &Fetcher, raw: &str) -> Line {
    match fetcher.get(raw, fetch::ACCEPT_HTML) {
        Ok(response) => read_page(raw, response),
        Err(refusal) => refused(raw, refusal),
    }
}

fn refused(raw: &str, refusal: Refusal) -> Line {
    let (status, reason) = match refusal {
        Refusal::InvalidUrl => (Outcome::Error, "not a valid URL".to_owned()),
        Refusal::NotWeb => (Outcome::Skipped, "redirected away from the web".to_owned()),
        Refusal::PrivateNetwork { redirected: false } => {
            (Outcome::Skipped, "private network".to_owned())
        }
        Refusal::PrivateNetwork { redirected: true } => (
            Outcome::Skipped,
            "redirected to a private network".to_owned(),
        ),
        Refusal::PrivateAddress => (Outcome::Skipped, "private network address".to_owned()),
        Refusal::Forgotten => (Outcome::Skipped, "forgotten page, not fetched".to_owned()),
        Refusal::TokenOrSearch => (
            Outcome::Skipped,
            "token or search URL, not fetched".to_owned(),
        ),
        Refusal::Login(url) => {
            let mut record =
                Line::new(raw, Outcome::BehindLogin).with_reason("redirected to a login page");
            record.final_url = Some(url.to_string());
            return record;
        }
        Refusal::InvalidRedirect { status } => {
            (Outcome::Error, format!("HTTP {status} to an invalid URL"))
        }
        Refusal::TooManyRedirects => (Outcome::Error, "too many redirects".to_owned()),
        Refusal::Failed(reason) => (Outcome::Error, reason),
    };
    Line::new(raw, status).with_reason(reason)
}

fn read_page(raw: &str, mut response: Response) -> Line {
    let url = response.url.clone();
    let status = response.status;
    let with_response = |mut record: Line| {
        record.final_url = Some(url.to_string());
        record.http_status = Some(status);
        record
    };
    if status == 401 {
        return with_response(Line::new(raw, Outcome::BehindLogin).with_reason("HTTP 401"));
    }
    if !(200..300).contains(&status) {
        return with_response(Line::new(raw, Outcome::Error).with_reason(format!("HTTP {status}")));
    }
    let mime = response.mime();
    if !mime.is_empty() && !mime.contains("html") {
        return with_response(
            Line::new(raw, Outcome::Skipped).with_reason(format!("not HTML ({mime})")),
        );
    }
    if !response.is_identity() {
        return with_response(
            Line::new(raw, Outcome::Error).with_reason("unsupported content encoding"),
        );
    }
    let repo = github::is_repo_page(&url);
    let cap = HEAD_CAP;
    let bytes = match response.read(cap, !repo) {
        Ok(bytes) => bytes,
        Err(reason) => return with_response(Line::new(raw, Outcome::Error).with_reason(reason)),
    };
    let text = match head::decode(
        &bytes,
        head::charset_param(&response.content_type).as_deref(),
    ) {
        Ok(text) => text,
        Err(label) => {
            return with_response(
                Line::new(raw, Outcome::Error).with_reason(format!("unsupported charset {label}")),
            );
        }
    };
    if text.len() > HEAD_CAP {
        return with_response(
            Line::new(raw, Outcome::Error).with_reason("decoded head exceeds 3 MB"),
        );
    }
    let found = head::scan(&text);
    if is_sign_in_page(&found) {
        let mut record =
            with_response(Line::new(raw, Outcome::BehindLogin).with_reason("sign-in page"));
        record.title.clone_from(&found.title);
        return record;
    }
    let mut record = with_response(Line::new(raw, Outcome::Ok));
    fill(&mut record, &found, &url);
    if repo {
        record.github = github::repo_data(&text);
    }
    let empty = record.title.is_none()
        && record.description.is_none()
        && record.og.is_empty()
        && record.twitter.is_empty()
        && record.github.is_none();
    if empty && bytes.len() >= cap {
        return with_response(
            Line::new(raw, Outcome::Error).with_reason("no </head> in the first 3 MB"),
        );
    }
    record
}

fn fill(record: &mut Line, found: &Head, url: &Url) {
    let cap = |value: &str| value.chars().take(VALUE_CAP).collect::<String>();
    record.title = found.title.as_deref().map(cap);
    record.description = found.meta("description").map(|d| cap(&head::collapse(d)));
    record.lang.clone_from(&found.lang);
    record.canonical = found
        .canonical
        .as_deref()
        .and_then(|href| url.join(href).ok())
        .map(|canonical| canonical.to_string());
    for (key, value) in &found.meta {
        let (map, name) = if let Some(name) = key.strip_prefix("og:") {
            (&mut record.og, name)
        } else if let Some(name) = key.strip_prefix("twitter:") {
            (&mut record.twitter, name)
        } else {
            continue;
        };
        if !name.is_empty() && map.len() < 32 {
            map.entry(name.to_owned())
                .or_insert_with(|| cap(&head::collapse(value)));
        }
    }
    record.jsonld_types = jsonld_types(&found.jsonld);
}

/// Every `@type` in the page's JSON-LD, `@graph` and nesting included, in
/// the order met and without the schema.org prefix.
fn jsonld_types(blocks: &[String]) -> Vec<String> {
    fn walk(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                let types = match map.get("@type") {
                    Some(serde_json::Value::Array(items)) => items.iter().collect(),
                    Some(one) => vec![one],
                    None => Vec::new(),
                };
                for name in types.into_iter().filter_map(serde_json::Value::as_str) {
                    let name = ["https://schema.org/", "http://schema.org/"]
                        .iter()
                        .fold(name.trim(), |n, prefix| n.strip_prefix(prefix).unwrap_or(n));
                    if !name.is_empty() && found.len() < 16 && !found.iter().any(|f| f == name) {
                        found.push(name.to_owned());
                    }
                }
                map.values().for_each(|v| walk(v, found));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for block in blocks {
        // Some pages wrap the JSON in an HTML comment or CDATA.
        let text = block
            .trim()
            .trim_start_matches("<!--")
            .trim_end_matches("-->")
            .trim_start_matches("//<![CDATA[")
            .trim_end_matches("//]]>");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            walk(&value, &mut found);
        }
    }
    found
}

/// A page that is only a sign-in form: a title that says so and nothing
/// that describes the page itself.
fn is_sign_in_page(found: &Head) -> bool {
    let Some(title) = &found.title else {
        return false;
    };
    let title = title.to_lowercase();
    let says_sign_in = ["sign in", "signin", "sign-in", "log in", "login", "log-in"]
        .iter()
        .any(|phrase| has_word(&title, phrase));
    says_sign_in && found.meta("description").is_none() && found.meta("og:description").is_none()
}

fn has_word(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_title_without_a_description_is_a_sign_in_page() {
        let page = |html: &str| is_sign_in_page(&head::scan(html));
        assert!(page("<title>Sign in - Example</title>"));
        assert!(page("<title>Login | App</title>"));
        assert!(!page(
            "<title>Sign in</title><meta name=description content=\"A real page\">"
        ));
        assert!(!page(
            "<title>Why login forms fail</title><meta property=og:description content=x>"
        ));
        assert!(!page("<title>Blogin' about loginess</title>"));
    }

    #[test]
    fn json_ld_types_are_collected_through_graphs_and_arrays() {
        let blocks = vec![
            r#"{"@context":"https://schema.org","@graph":[{"@type":"WebPage"},{"@type":["Article","https://schema.org/NewsArticle"],"author":{"@type":"Person"}}]}"#.to_owned(),
            "<!--{\"@type\":\"WebPage\"}-->".to_owned(),
            "not json".to_owned(),
        ];
        assert_eq!(
            jsonld_types(&blocks),
            ["WebPage", "Article", "NewsArticle", "Person"]
        );
    }

    #[test]
    fn refusals_keep_the_contract_reasons() {
        let line = |refusal| {
            let line = refused("http://x.test/", refusal);
            (line.status, line.reason)
        };
        assert_eq!(
            line(Refusal::PrivateAddress),
            (Outcome::Skipped, Some("private network address".to_owned()))
        );
        assert_eq!(
            line(Refusal::PrivateNetwork { redirected: false }),
            (Outcome::Skipped, Some("private network".to_owned()))
        );
    }
}
