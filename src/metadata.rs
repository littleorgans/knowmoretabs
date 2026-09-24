//! The reader of `pages/metadata.jsonl`, the page metadata `enrich` fetches.
//!
//! slice: triage
//! why: `enrich` writes this file and `tag --prompt` reads it; the two are
//!      built apart, so they meet in this one module and its pinned shape
//!      (the 7b/7c contract). The file is append-only with one line per
//!      fetch attempt, so the latest line for a URL wins, a field this build
//!      does not know is ignored, and a line it cannot read (a torn tail
//!      after a crash) costs that line, not the prompt. This module never
//!      writes the file.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Deserialize;

use crate::error::Error;

pub const DIR: &str = "pages";
pub const FILE: &str = "metadata.jsonl";

pub fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// One fetch attempt. Every field but `url`, `fetched_at` and `status` is
/// optional; `skipped` and `error` lines carry a `reason`. The whole pinned
/// shape is declared, read or not, so the contract lives in one place.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub url: String,
    pub fetched_at: Timestamp,
    pub status: Status,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub final_url: Option<String>,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub canonical: Option<String>,
    #[serde(default)]
    pub og: Option<OpenGraph>,
    #[serde(default)]
    pub twitter: Option<Twitter>,
    #[serde(default)]
    pub jsonld_types: Vec<String>,
    #[serde(default)]
    pub github: Option<GitHub>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    BehindLogin,
    Skipped,
    Error,
    /// A status a newer `enrich` wrote: read as "no metadata".
    #[serde(other)]
    Unknown,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenGraph {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub site_name: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Twitter {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GitHub {
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub readme: Option<String>,
}

impl Record {
    /// The description a reader would pick: the page's own, then Open
    /// Graph's, then Twitter's. Blank counts as absent.
    pub fn best_description(&self) -> Option<&str> {
        [
            self.description.as_deref(),
            self.og.as_ref().and_then(|og| og.description.as_deref()),
            self.twitter.as_ref().and_then(|t| t.description.as_deref()),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|text| !text.is_empty())
    }
}

#[derive(Debug, Default)]
pub struct Metadata {
    /// The latest line for each URL.
    pub pages: HashMap<String, Record>,
    /// Lines that were not a record this build can read.
    pub unreadable: usize,
}

/// The whole file; a missing file is no metadata.
pub fn read(root: &Path) -> Result<Metadata, Error> {
    let path = path(root);
    let text = match fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Metadata::default()),
        Err(err) => return Err(Error::io("read page metadata", &path)(err)),
    };
    Ok(parse(&text))
}

fn parse(text: &str) -> Metadata {
    let mut metadata = Metadata::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Record>(line) {
            Ok(record) => {
                metadata.pages.insert(record.url.clone(), record);
            }
            Err(_) => metadata.unreadable += 1,
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_line_wins_unknown_fields_are_ignored_and_torn_lines_are_counted() {
        let text = concat!(
            r#"{"url":"https://a.test/","fetched_at":"2026-09-24T10:00:00Z","status":"error","reason":"timeout"}"#,
            "\n",
            r#"{"url":"https://b.test/","fetched_at":"2026-09-24T10:00:00Z","status":"behind_login"}"#,
            "\n\n",
            r#"{"url":"https://a.test/","fetched_at":"2026-09-24T11:00:00Z","status":"ok","title":"A","#,
            r#""og":{"type":"website","description":"From og","future":1},"jsonld_types":["WebSite"],"#,
            r#""github":{"topics":["mcp"],"readme":"Read me"},"added_later":{"x":1}}"#,
            "\n",
            r#"{"url":"https://c.test/","fetched_at":"2026-09-24T10:00:00Z","status":"parked"}"#,
            "\n",
            r#"{"url":"https://d.test/","fetched_at":"2026-09-2"#,
        );
        let metadata = parse(text);
        assert_eq!(metadata.unreadable, 1);
        assert_eq!(metadata.pages.len(), 3);
        let a = &metadata.pages["https://a.test/"];
        assert_eq!(a.status, Status::Ok);
        assert_eq!(a.title.as_deref(), Some("A"));
        assert_eq!(a.best_description(), Some("From og"));
        assert_eq!(a.og.as_ref().unwrap().kind.as_deref(), Some("website"));
        assert_eq!(a.github.as_ref().unwrap().topics, ["mcp"]);
        assert_eq!(
            metadata.pages["https://b.test/"].status,
            Status::BehindLogin
        );
        assert_eq!(metadata.pages["https://c.test/"].status, Status::Unknown);
    }

    #[test]
    fn a_blank_description_falls_through() {
        let record: Record = serde_json::from_str(
            r#"{"url":"u","fetched_at":"2026-09-24T10:00:00Z","status":"ok","description":"  ","twitter":{"description":"T"}}"#,
        )
        .unwrap();
        assert_eq!(record.best_description(), Some("T"));
    }
}
