//! The line `enrich` writes to `pages/metadata.jsonl`: one per fetch
//! attempt, appended through `jsonl`.
//!
//! slice: enrich
//! why: `enrich` owns this file and `tag --prompt` reads it through
//!      `metadata`, the contract's reader, which this module deliberately
//!      does not touch. The written shape is declared here, apart from the
//!      read one, so that a field added for writing can never quietly change
//!      what the pinned 7b/7c reader accepts; the round trip test holds the
//!      two together.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Serialize;

/// One fetch attempt, in the shape `metadata::Record` reads. Absent fields
/// are not written.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    pub url: String,
    pub fetched_at: Timestamp,
    pub status: Outcome,
    /// Why a page was skipped, failed, or is taken to be behind a login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// Every `og:*` meta tag without its prefix. The contract names
    /// `title`, `description`, `type` and `site_name`; the rest are room to
    /// grow, and readers ignore them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub og: BTreeMap<String, String>,
    /// Every `twitter:*` meta tag without its prefix.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub twitter: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsonld_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<Github>,
}

/// The contract's `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    BehindLogin,
    Skipped,
    Error,
}

/// A public repository's own page: its topics and the start of its README.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Github {
    pub topics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
}

impl Line {
    pub fn new(url: &str, status: Outcome) -> Self {
        let now = Timestamp::now();
        Self {
            url: url.to_owned(),
            // Whole seconds, as the contract shows: the date is what counts.
            fetched_at: Timestamp::from_second(now.as_second()).unwrap_or(now),
            status,
            reason: None,
            final_url: None,
            http_status: None,
            title: None,
            description: None,
            lang: None,
            canonical: None,
            og: BTreeMap::new(),
            twitter: BTreeMap::new(),
            jsonld_types: Vec::new(),
            github: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::jsonl::Appender;
    use crate::metadata::{self, Status};

    fn page(url: &str) -> Line {
        let mut line = Line::new(url, Outcome::Ok);
        line.title = Some("A page".into());
        line.description = Some("About it".into());
        line.og.insert("type".into(), "article".into());
        line.og.insert("site_name".into(), "Example".into());
        line.og
            .insert("image".into(), "https://a.test/i.png".into());
        line.twitter.insert("card".into(), "summary".into());
        line.github = Some(Github {
            topics: vec!["rust".into()],
            readme: Some("Read me".into()),
        });
        line
    }

    fn raw_lines(root: &Path) -> Vec<String> {
        fs::read_to_string(metadata::path(root))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn what_is_written_is_what_the_contract_reader_reads() {
        let dir = tempfile::tempdir().unwrap();
        let mut appender = Appender::open(dir.path(), metadata::path(dir.path())).unwrap();
        appender
            .append(&Line::new("https://a.test/", Outcome::Error).with_reason("timeout"))
            .unwrap();
        appender.append(&page("https://b.test/")).unwrap();
        appender.append(&page("https://a.test/")).unwrap();
        appender
            .append(&Line::new("https://c.test/", Outcome::BehindLogin))
            .unwrap();

        let read = metadata::read(dir.path()).unwrap();
        assert_eq!((read.pages.len(), read.unreadable), (3, 0));
        let a = &read.pages["https://a.test/"];
        assert_eq!(a.status, Status::Ok, "the latest line wins");
        assert_eq!(a.title.as_deref(), Some("A page"));
        assert_eq!(a.best_description(), Some("About it"));
        let og = a.og.as_ref().unwrap();
        assert_eq!(og.kind.as_deref(), Some("article"));
        assert_eq!(og.site_name.as_deref(), Some("Example"));
        let github = a.github.as_ref().unwrap();
        assert_eq!(github.topics, ["rust"]);
        assert_eq!(github.readme.as_deref(), Some("Read me"));
        assert_eq!(read.pages["https://c.test/"].status, Status::BehindLogin);

        let first: serde_json::Value = serde_json::from_str(&raw_lines(dir.path())[0]).unwrap();
        let keys: Vec<&str> = first
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["fetched_at", "reason", "status", "url"]);
        assert!(
            !first["fetched_at"].as_str().unwrap().contains('.'),
            "whole seconds: {}",
            first["fetched_at"]
        );
    }
}
