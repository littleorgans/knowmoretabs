//! `knowmoretabs search`: the pages added one at a time, local documents
//! among them, that a query finds, by keyword or ranked by the model
//! service, read only.
//!
//! slice: sources
//! why: An agent researching with a separate archive needs what it added
//!      back by what it says, not only by its title, and must never be
//!      handed the owner's browsing. So search reads only the intake log,
//!      the pins log, the library state (forgotten pages stay hidden) and
//!      what each page's text is: a local document's pinned bytes, read
//!      live through the resolver, and a web page's kept text. It never
//!      reads a snapshot or the History record, takes no lock, creates
//!      nothing and writes nothing. The keyword rule is the Library's own
//!      (every word, lowercased, somewhere in the page), and with the model
//!      service its order comes first, then the keyword matches it has not
//!      indexed, as the Library merges them; the service is asked only on
//!      its read only rank route. Every local result says whether its text
//!      is current, behind its checkout or out of reach, and why, so an
//!      answer is never silently stale or silently missing.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::capture::Log;
use crate::error::Error;
use crate::model_service::ModelService;
use crate::page_text::{self, PageText, Reader};
use crate::{out, sources};

pub const SCHEMA_VERSION: u32 = 1;
/// Ranked pages kept, as the Library shows them (`RANK_TOP` in `web/app.js`).
pub const RANK_TOP: usize = 50;
/// The most of a page's text a result quotes, in characters.
const SNIPPET_CHARS: usize = 200;
/// How much of the quote comes before the first word found.
const SNIPPET_LEAD: usize = 60;

/// A page search can find, with what it says.
struct Entry {
    url: String,
    title: String,
    page: PageText,
}

/// One result, as `--json` writes it.
#[derive(Serialize)]
struct Hit<'a> {
    url: &'a str,
    title: &'a str,
    #[serde(flatten)]
    page: &'a PageText,
    matched: Vec<&'static str>,
    snippet: Option<String>,
    ranked: bool,
}

impl Entry {
    /// Which of the title, the key and the text hold a word, when every
    /// word is in one of them; `None` when the page does not match.
    fn matched(&self, words: &[String]) -> Option<Vec<&'static str>> {
        let title = self.title.to_lowercase();
        let url = self.url.to_lowercase();
        let text = self.text().map(str::to_lowercase);
        let fields = [
            ("title", Some(&title)),
            ("url", Some(&url)),
            ("text", text.as_ref()),
        ];
        let mut matched = BTreeSet::new();
        for word in words {
            let found: Vec<_> = fields
                .iter()
                .filter(|(_, hay)| hay.is_some_and(|hay| hay.contains(word.as_str())))
                .map(|(name, _)| *name)
                .collect();
            if found.is_empty() {
                return None;
            }
            matched.extend(found);
        }
        Some(
            ["title", "url", "text"]
                .into_iter()
                .filter(|f| matched.contains(f))
                .collect(),
        )
    }

    fn text(&self) -> Option<&str> {
        self.page.text.as_ref().map(|text| text.body.as_str())
    }

    fn result(&self, words: &[String], ranked: bool) -> Value {
        json!(Hit {
            url: &self.url,
            title: &self.title,
            page: &self.page,
            matched: self.matched(words).unwrap_or_default(),
            snippet: self.text().and_then(|text| snippet(text, words)),
            ranked,
        })
    }
}

/// `knowmoretabs search`. `model` is the service on `--model-port`, when
/// its key file holds a key.
pub fn command(
    root: &Path,
    query: &str,
    checkouts: &Path,
    model: Option<&ModelService>,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let entries = entries(root, checkouts)?;
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    let ranked = match model.filter(|_| !words.is_empty()) {
        None => None,
        Some(service) => match service.rank(root, query) {
            Ok(keys) => Some(keys),
            Err(why) => {
                log.warn(&format!(
                    "the model service did not rank: {why}; search is by keyword"
                ));
                None
            }
        },
    };
    let hits = pick(&entries, &words, ranked.as_deref());
    if json {
        out::json(&json!({
            "schema_version": SCHEMA_VERSION,
            "query": query,
            "ranking": if ranked.is_some() { "model" } else { "keyword" },
            "results": hits
                .iter()
                .map(|(entry, ranked)| entry.result(&words, *ranked))
                .collect::<Vec<_>>(),
        }));
    } else if !log.quiet {
        for (entry, _) in &hits {
            let state = match entry.page.state {
                Some(sources::State::Fresh) | None => String::new(),
                Some(state) => format!("  [{}: {}]", state.word(), entry.page.why.unwrap_or("")),
            };
            out::line(&format!("{}  {}{state}", entry.url, entry.title));
        }
    }
    Ok(())
}

/// The ranked pages search may show, in the service's order, at most
/// [`RANK_TOP`], then the pages it has not indexed that the words match;
/// without a ranking, every match in library order. Each with whether the
/// service ranked it. `web/app.js` `pick`.
fn pick<'a>(
    entries: &'a [Entry],
    words: &[String],
    ranked: Option<&[String]>,
) -> Vec<(&'a Entry, bool)> {
    let Some(ranked) = ranked else {
        return entries
            .iter()
            .filter(|entry| entry.matched(words).is_some())
            .map(|entry| (entry, false))
            .collect();
    };
    let place: HashMap<&str, usize> = ranked
        .iter()
        .enumerate()
        .rev()
        .map(|(at, key)| (key.as_str(), at))
        .collect();
    let mut first: Vec<&Entry> = entries
        .iter()
        .filter(|e| place.contains_key(e.url.as_str()))
        .collect();
    first.sort_by_key(|entry| place[entry.url.as_str()]);
    first
        .into_iter()
        .take(RANK_TOP)
        .map(|entry| (entry, true))
        .chain(
            entries
                .iter()
                .filter(|e| !place.contains_key(e.url.as_str()) && e.matched(words).is_some())
                .map(|entry| (entry, false)),
        )
        .collect()
}

/// Every added page the library lists and has not forgotten, with its
/// text: a document's from its pin, a web page's as kept.
fn entries(root: &Path, checkouts: &Path) -> Result<Vec<Entry>, Error> {
    let (added, _) = page_text::added(root)?;
    let mut reader = Reader::new(root, Some(checkouts));
    added
        .into_iter()
        .map(|tab| {
            Ok(Entry {
                page: reader.text(&tab.url)?,
                url: tab.url,
                title: tab.title,
            })
        })
        .collect()
}

/// About [`SNIPPET_CHARS`] of `text` around the first query word it holds,
/// whitespace made single spaces; `None` when it holds none.
fn snippet(text: &str, words: &[String]) -> Option<String> {
    // Lowercasing can change a character's length, so each lowercased byte
    // remembers where its character starts in `text`.
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        for l in c.to_lowercase() {
            lower.push(l);
            origin.resize(lower.len(), at);
        }
    }
    let found = words.iter().find_map(|word| lower.find(word.as_str()))?;
    let at = origin[found];
    let before: Vec<(usize, char)> = text[..at].char_indices().collect();
    let start = before
        .len()
        .checked_sub(SNIPPET_LEAD)
        .map_or(0, |i| before[i].0);
    let window: String = text[start..].chars().take(SNIPPET_CHARS).collect();
    let mut quote = window.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        quote.insert(0, '…');
    }
    if text[start..].chars().nth(SNIPPET_CHARS).is_some() {
        quote.push('…');
    }
    Some(quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::page_text::{Kind, Text};

    fn entry(url: &str, title: &str, text: Option<&str>) -> Entry {
        Entry {
            url: url.to_owned(),
            title: title.to_owned(),
            page: PageText {
                kind: Kind::Web,
                state: None,
                why: None,
                source: Value::Null,
                text: text.map(|body| Text {
                    title: None,
                    body: body.to_owned(),
                }),
            },
        }
    }

    fn words(query: &str) -> Vec<String> {
        query
            .to_lowercase()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect()
    }

    #[test]
    fn every_word_must_be_somewhere_and_each_field_is_named() {
        let page = entry(
            "https://a.test/zeppelin",
            "Night trains",
            Some("Airship timetables"),
        );
        assert_eq!(
            page.matched(&words("NIGHT zeppelin")),
            Some(vec!["title", "url"])
        );
        assert_eq!(page.matched(&words("airship")), Some(vec!["text"]));
        assert_eq!(page.matched(&words("night ferry")), None);
        assert_eq!(page.matched(&[]), Some(vec![]));
        assert_eq!(
            entry("https://a.test/", "", None).matched(&words("x")),
            None
        );
    }

    #[test]
    fn ranked_pages_come_first_in_the_services_order_then_unindexed_matches() {
        let pages = [
            entry("https://a.test/1", "zeppelin one", None),
            entry("https://a.test/2", "other", None),
            entry("https://a.test/3", "zeppelin three", None),
            entry("https://a.test/4", "zeppelin four", None),
        ];
        let words = words("zeppelin");
        let ranked = [
            "https://a.test/2".to_owned(),
            "https://a.test/1".to_owned(),
            "https://gone.test/".to_owned(),
        ];
        let order: Vec<_> = pick(&pages, &words, Some(&ranked))
            .into_iter()
            .map(|(e, ranked)| (e.url.as_str(), ranked))
            .collect();
        assert_eq!(
            order,
            [
                ("https://a.test/2", true),
                ("https://a.test/1", true),
                ("https://a.test/3", false),
                ("https://a.test/4", false),
            ]
        );
        let keyword: Vec<_> = pick(&pages, &words, None)
            .into_iter()
            .map(|(e, _)| e.url.as_str())
            .collect();
        assert_eq!(
            keyword,
            ["https://a.test/1", "https://a.test/3", "https://a.test/4"]
        );
    }

    #[test]
    fn a_snippet_quotes_around_the_first_word_found() {
        let text = format!(
            "{}The ZEPPELIN left at dawn.\n\nIt flew {}",
            "a ".repeat(100),
            "b ".repeat(200)
        );
        let quote = snippet(&text, &words("missing zeppelin")).unwrap();
        assert!(quote.starts_with('…') && quote.ends_with('…'), "{quote}");
        assert!(
            quote.contains("The ZEPPELIN left at dawn. It flew"),
            "{quote}"
        );
        assert!(quote.chars().count() <= SNIPPET_CHARS + 2);
        assert_eq!(
            snippet("İstanbul zeppelin", &words("zeppelin")).as_deref(),
            Some("İstanbul zeppelin")
        );
        assert_eq!(snippet("nothing here", &words("zeppelin")), None);
    }
}
