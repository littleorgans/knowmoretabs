//! Typed detailed ranking and passage validation at the signed service boundary.
//!
//! slice: sources
//! why: Search may quote a semantic passage only when its reference and exact
//!      bytes agree with the current reader, and must reject incomplete orders.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::digest::sha256_hex;
use crate::model_service::Failure;

pub const SNIPPET_CHARS: usize = 200;

#[derive(Serialize)]
pub struct Candidate<'a> {
    pub key: &'a str,
    pub content_sha256: Option<String>,
    #[serde(skip)]
    pub body: Option<&'a str>,
}

#[derive(Serialize)]
pub struct Request<'a> {
    pub schema_version: u32,
    pub query: &'a str,
    pub among: &'a [Candidate<'a>],
    pub snippet_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fallback {
    pub stage: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_pages: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub struct Source {
    pub score: f64,
    pub rank: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub hash: String,
    pub page_hash: String,
    pub score: f64,
}

#[derive(Serialize, Deserialize)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

#[derive(Serialize, Deserialize)]
pub struct Hit {
    pub key: String,
    pub score: f64,
    pub sources: BTreeMap<String, Source>,
    pub semantic_source: String,
    pub chunk_status: String,
    pub section: Option<String>,
    pub chunk: Option<Chunk>,
    pub snippet: Option<String>,
    pub snippet_range: Option<Range>,
    pub content_sha256: Option<String>,
}

#[derive(Deserialize)]
pub struct Ranked {
    pub schema_version: u32,
    pub keys: Vec<String>,
    pub hits: Vec<Hit>,
    pub fallback: Vec<Fallback>,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Ranked {
    pub fn validate(&self, among: &[Candidate<'_>]) -> Result<(), Failure> {
        let invalid = Failure::InvalidRank;
        if self.schema_version != 1 {
            return Err(Failure::UnsupportedRank);
        }
        let candidates: HashMap<_, _> = among.iter().map(|c| (c.key, c)).collect();
        let mut seen = BTreeSet::new();
        let mut source_ranks: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
        if self.keys.len() != self.hits.len() {
            return Err(invalid);
        }
        for (key, hit) in self.keys.iter().zip(&self.hits) {
            let candidate = candidates.get(key.as_str()).ok_or(invalid)?;
            if key != &hit.key || !seen.insert(key) || !hit.score.is_finite() {
                return Err(invalid);
            }
            if !matches!(hit.semantic_source.as_str(), "A" | "B" | "C")
                || !matches!(
                    hit.chunk_status.as_str(),
                    "ready" | "missing" | "stale" | "unavailable"
                )
                || !hit.sources.contains_key("text")
            {
                return Err(invalid);
            }
            for (name, source) in &hit.sources {
                if !matches!(name.as_str(), "text" | "keyword" | "image")
                    || !source.score.is_finite()
                    || source.rank > self.hits.len()
                    || (name == "text"
                        && (source.rank == 0
                            || source.input.as_deref() != Some(&hit.semantic_source)))
                    || (name != "text" && source.input.is_some())
                    || (name == "keyword" && (source.rank > 0) != (source.score > 0.0))
                    || (source.rank > 0
                        && !source_ranks.entry(name).or_default().insert(source.rank))
                {
                    return Err(invalid);
                }
            }
            match (
                &hit.chunk,
                &hit.snippet_range,
                &hit.snippet,
                &hit.content_sha256,
            ) {
                (Some(chunk), Some(range), Some(snippet), Some(hash)) => {
                    let body = candidate.body.ok_or(invalid)?;
                    let span = body.get(chunk.start..chunk.end).ok_or(invalid)?;
                    let expected: String = span.chars().take(SNIPPET_CHARS).collect();
                    if hit.semantic_source != "C"
                        || hit.chunk_status != "ready"
                        || hit.section.is_none()
                        || !chunk.score.is_finite()
                        || !digest(&chunk.id)
                        || !digest(&chunk.hash)
                        || !digest(&chunk.page_hash)
                        || chunk.start >= chunk.end
                        || range.start != chunk.start
                        || range.end > chunk.end
                        || range.start >= range.end
                        || body.get(range.start..range.end) != Some(snippet.as_str())
                        || snippet != &expected
                        || candidate.content_sha256.as_ref() != Some(hash)
                        || sha256_hex(body.as_bytes()) != *hash
                    {
                        return Err(invalid);
                    }
                }
                (None, None, None, None)
                    if hit.section.is_none()
                        && hit.semantic_source != "C"
                        && hit.chunk_status != "ready" => {}
                _ => return Err(invalid),
            }
        }
        for reason in &self.fallback {
            if reason.stage == "rank" && reason.code == "rank_response_too_large" {
                return Err(Failure::TooLarge);
            }
            if reason.stage != "chunks"
                || !matches!(
                    reason.code.as_str(),
                    "chunks_missing" | "chunks_stale" | "chunks_unavailable"
                )
                || !reason
                    .affected_pages
                    .is_some_and(|n| n > 0 && n <= self.hits.len())
            {
                return Err(invalid);
            }
        }
        Ok(())
    }
}
