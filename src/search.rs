//! `knowmoretabs search`: the pages added one at a time, local documents
//! among them, that a query finds, by keyword or ranked by the model
//! service, read only.
//!
//! slice: sources
//! why: An agent researching with a separate archive needs what it added
//!      back by what it says, not only by its title, and must never be
//!      handed the owner's browsing. So search reads only the intake log,
//!      the pins log, the content status log, the library state (forgotten pages stay hidden) and
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
use crate::cli::{SearchArgs, SearchMode};
use crate::digest::sha256_hex;
use crate::error::{self, Error};
use crate::library;
use crate::model_rank::{Candidate, Hit as RankedHit, SNIPPET_CHARS};
use crate::model_service::{Failure, ModelService};
use crate::page_text::{self, PageText, Reader};
use crate::{content_store, model::Tab};
use crate::{out, sources};

pub const SCHEMA_VERSION: u32 = 1;
/// How much of the quote comes before the first word found.
const SNIPPET_LEAD: usize = 60;

/// A page search can find, with what it says.
struct Entry {
    url: String,
    title: String,
    page: PageText,
    tags: Vec<String>,
    added_at: Option<jiff::Timestamp>,
    capture_status: Option<content_store::Status>,
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

    fn result(&self, words: &[String], rank: usize, ranked: Option<&RankedHit>) -> Value {
        let mut value = json!(Hit {
            url: &self.url,
            title: &self.title,
            page: &self.page,
            matched: self.matched(words).unwrap_or_default(),
            snippet: self.text().and_then(|text| snippet(text, words)),
            ranked: ranked.is_some(),
        });
        let text = self.text();
        let status = match text {
            Some(body) if body.trim().is_empty() => "empty",
            Some(_) => "readable",
            None => "unavailable",
        };
        let reason = if status == "unavailable" {
            Some(self.page.why.unwrap_or("no text kept for it"))
        } else {
            None
        };
        let extra = json!({
            "id": sha256_hex(self.url.as_bytes()), "rank": rank,
            "content_sha256": text.map(|body| sha256_hex(body.as_bytes())),
            "tags": self.tags, "added_at": self.added_at, "capture_status": self.capture_status,
            "readability": {"status": status, "reason": reason},
            "section": ranked.and_then(|h| h.section.as_deref()),
            "chunk": ranked.and_then(|h| h.chunk.as_ref()),
            "snippet_range": ranked.and_then(|h| h.snippet_range.as_ref()),
            "score": ranked.map(|h| h.score),
            "sources": ranked.map(|h| json!(h.sources)).unwrap_or(json!({})),
            "semantic_source": ranked.map(|h| &h.semantic_source),
            "chunk_status": ranked.map(|h| &h.chunk_status),
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        if let Some(quote) = ranked.and_then(|h| h.snippet.as_ref()) {
            value["snippet"] = json!(quote);
        }
        value
    }
}

/// Normalized filter values also form part of the cursor identity.
#[derive(Serialize)]
struct Filters {
    tags: BTreeSet<String>,
    domains: BTreeSet<String>,
    kinds: BTreeSet<String>,
    after: Option<jiff::Timestamp>,
    before: Option<jiff::Timestamp>,
}

impl Filters {
    fn new(args: &SearchArgs) -> Result<Self, Error> {
        if args.after.zip(args.before).is_some_and(|(a, b)| a > b) {
            return Err(Error::Usage(
                "--after must not be later than --before".into(),
            ));
        }
        let domains = args
            .domain
            .iter()
            .map(|host| {
                let parsed = url::Url::parse(&format!("https://{host}"))
                    .map_err(|_| Error::Usage("--domain requires a host".into()))?;
                if url::Host::parse(host).is_err() {
                    return Err(Error::Usage("--domain requires a host".into()));
                }
                Ok(library::public_domain(parsed.as_str()).unwrap_or_else(|| host.to_lowercase()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            tags: args.tag.iter().map(|tag| library::fold(tag)).collect(),
            domains,
            kinds: args
                .kind
                .iter()
                .map(|kind| json!(kind).as_str().unwrap().to_owned())
                .collect(),
            after: args.after,
            before: args.before,
        })
    }

    fn allows(&self, tab: &Tab, tags: &[String]) -> bool {
        self.tags
            .iter()
            .all(|tag| tags.iter().any(|t| library::fold(t) == *tag))
            && (self.domains.is_empty()
                || (page_text::Kind::of(&tab.url) == page_text::Kind::Web
                    && library::public_domain(&tab.url)
                        .is_some_and(|host| self.domains.contains(&host))))
            && (self.kinds.is_empty()
                || self
                    .kinds
                    .contains(json!(page_text::Kind::of(&tab.url)).as_str().unwrap()))
            && self
                .after
                .is_none_or(|after| tab.added_at.is_some_and(|date| date >= after))
            && self
                .before
                .is_none_or(|before| tab.added_at.is_some_and(|date| date < before))
    }
}

/// Search ranks the complete eligible set, appends the literal tail, then pages once.
pub fn command(
    root: &Path,
    args: &SearchArgs,
    checkouts: &Path,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let filters = Filters::new(args)?;
    let (entries, warnings) = entries(root, checkouts, &filters, log)?;
    let query = args.query.join(" ");
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    let mut fallback = Vec::new();
    let ranked = if words.is_empty() || args.mode == SearchMode::Keyword {
        None
    } else {
        let service = args
            .model
            .model_port
            .ok_or(Failure::Off)
            .and_then(|port| ModelService::new(port, args.model.model_secret_file.as_deref()));
        let among: Vec<_> = entries
            .iter()
            .map(|entry| Candidate {
                key: &entry.url,
                body: entry.text(),
                content_sha256: entry.text().map(|text| sha256_hex(text.as_bytes())),
            })
            .collect();
        match service.and_then(|service| service.rank(root, &query, &among)) {
            Ok(ranked) => {
                fallback.extend(ranked.fallback.iter().cloned());
                Some(ranked)
            }
            Err(why) => {
                if why != Failure::Off {
                    why.warn(log);
                }
                fallback.push(why.fallback());
                None
            }
        }
    };
    let ranking = if ranked.is_some() { "model" } else { "keyword" };
    let ordered = pick(&entries, &words, ranked.as_ref().map(|r| r.keys.as_slice()));
    let details: HashMap<_, _> = ranked
        .as_ref()
        .map(|r| r.hits.iter().map(|h| (h.key.as_str(), h)).collect())
        .unwrap_or_default();
    let results: Vec<_> = ordered
        .iter()
        .enumerate()
        .map(|(at, (entry, _))| {
            entry.result(&words, at + 1, details.get(entry.url.as_str()).copied())
        })
        .collect();
    let identity = json!({"query": query, "filters": filters,
        "mode": format!("{:?}", args.mode), "ranking": ranking, "fallback": fallback, "results": results});
    let digest = sha256_hex(identity.to_string().as_bytes());
    let offset = cursor_offset(args.cursor.as_deref(), &digest, results.len())?;
    let count = args
        .limit
        .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let end = offset.saturating_add(count).min(results.len());
    let next = (end < results.len()).then(|| format!("v1:{end}:{digest}"));
    if json {
        out::json(&json!({
            "schema_version": SCHEMA_VERSION, "query": query, "ranking": ranking,
            "warnings": warnings, "fallback": fallback,
            "total": results.len(), "returned": end - offset, "limit": args.limit,
            "truncated": next.is_some(), "next_cursor": next, "results": &results[offset..end],
        }));
    } else if !log.quiet {
        for (entry, _) in &ordered[offset..end] {
            let state = match entry.page.state {
                Some(sources::State::Fresh) | None => String::new(),
                Some(state) => format!("  [{}: {}]", state.word(), entry.page.why.unwrap_or("")),
            };
            out::line(&format!("{}  {}{state}", entry.url, entry.title));
        }
    }
    Ok(())
}

fn cursor_offset(cursor: Option<&str>, digest: &str, total: usize) -> Result<usize, Error> {
    let Some(cursor) = cursor else { return Ok(0) };
    let parts: Vec<_> = cursor.split(':').collect();
    let invalid = || Error::Usage("invalid search cursor".into());
    if parts.len() != 3
        || parts[0] != "v1"
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || parts[1].is_empty()
        || !parts[1].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let offset = parts[1].parse::<usize>().map_err(|_| invalid())?;
    if parts[2] != digest || offset > total {
        return Err(Error::StaleCursor);
    }
    Ok(offset)
}

/// Complete model order followed by unindexed literal matches in intake order.
/// Uncapped merge vectors are shared with the web consumer, which owns its cap.
fn pick<'a>(
    entries: &'a [Entry],
    words: &[String],
    ranked: Option<&[String]>,
) -> Vec<(&'a Entry, bool)> {
    let Some(ranked) = ranked else {
        return entries
            .iter()
            .filter(|e| e.matched(words).is_some())
            .map(|e| (e, false))
            .collect();
    };
    let by_key: HashMap<_, _> = entries.iter().map(|e| (e.url.as_str(), e)).collect();
    let mut seen = BTreeSet::new();
    let mut hits = Vec::new();
    for key in ranked {
        if let Some(entry) = by_key.get(key.as_str())
            && seen.insert(key.as_str())
        {
            hits.push((*entry, true));
        }
    }
    hits.extend(
        entries
            .iter()
            .filter(|e| !seen.contains(e.url.as_str()) && e.matched(words).is_some())
            .map(|e| (e, false)),
    );
    hits
}

/// Filters metadata before invoking the reader or the service.
fn entries(
    root: &Path,
    checkouts: &Path,
    filters: &Filters,
    log: Log,
) -> Result<(Vec<Entry>, Vec<Value>), Error> {
    let (added, state) = page_text::added(root)?;
    let spellings = state.spellings(false);
    let mut reader = Reader::new(root, Some(checkouts));
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let capture = match content_store::read(root) {
        Ok(capture) => {
            if let Some(note) = capture.unreadable_note(&content_store::log_path(root)) {
                log.warn(&note);
                warnings.push(error::render_json(&Error::ContentLog(note)));
            }
            Some(capture)
        }
        Err(err) => {
            log.warn(&err.to_string());
            warnings.push(error::render_json(&err));
            None
        }
    };
    for tab in added {
        let tags = state.page_tags(&tab.url, &spellings);
        if !filters.allows(&tab, &tags) {
            continue;
        }
        match reader.text(&tab.url) {
            Ok(page) => entries.push(Entry {
                capture_status: if page.kind == page_text::Kind::Web {
                    capture
                        .as_ref()
                        .and_then(|c| c.pages.get(&tab.url))
                        .map(|line| line.status)
                } else {
                    None
                },
                page,
                tags,
                added_at: tab.added_at,
                url: tab.url,
                title: tab.title,
            }),
            Err(err) => {
                log.warn(&format!("skipping {}: {err}", tab.url));
                let mut warning = error::render_json(&err);
                warning["url"] = json!(tab.url);
                warnings.push(warning);
            }
        }
    }
    Ok((entries, warnings))
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

    use serde::Deserialize;

    use crate::page_text::{Kind, Text};

    fn entry(url: &str, title: &str, text: Option<&str>) -> Entry {
        Entry {
            url: url.to_owned(),
            title: title.to_owned(),
            tags: Vec::new(),
            added_at: None,
            capture_status: None,
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

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/search_vectors.json")).unwrap()
    }

    fn text_of(value: &Value) -> &str {
        value.as_str().unwrap()
    }

    #[test]
    fn the_keyword_rule_matches_the_shared_vectors() {
        for case in vectors()["match"].as_array().unwrap() {
            let page = entry(text_of(&case["url"]), text_of(&case["title"]), None);
            let fields: Option<Vec<&str>> = Deserialize::deserialize(&case["fields"]).unwrap();
            assert_eq!(
                page.matched(&words(text_of(&case["query"]))),
                fields,
                "{case}"
            );
        }
    }

    #[test]
    fn the_rank_merge_matches_the_shared_vectors() {
        let vectors = vectors();
        let pages: Vec<Entry> = vectors["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| entry(text_of(&p["url"]), text_of(&p["title"]), None))
            .collect();
        for case in vectors["pick"].as_array().unwrap() {
            let ranked: Option<Vec<String>> = Deserialize::deserialize(&case["ranked"]).unwrap();
            let shows: Vec<(String, bool)> = Deserialize::deserialize(&case["shows"]).unwrap();
            let picked: Vec<(String, bool)> =
                pick(&pages, &words(text_of(&case["query"])), ranked.as_deref())
                    .into_iter()
                    .map(|(e, ranked)| (e.url.clone(), ranked))
                    .collect();
            assert_eq!(picked, shows, "{case}");
        }
    }

    /// The Library holds no page text, so this part of the rule is search's own.
    #[test]
    fn the_text_is_a_field_too() {
        let page = entry(
            "https://a.test/zeppelin",
            "Night trains",
            Some("Airship timetables"),
        );
        assert_eq!(page.matched(&words("airship")), Some(vec!["text"]));
        assert_eq!(
            page.matched(&words("night airship")),
            Some(vec!["title", "text"])
        );
        assert_eq!(page.matched(&words("airship ferry")), None);
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
