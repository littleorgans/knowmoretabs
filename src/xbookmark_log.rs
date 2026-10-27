//! The X log, `pages/xbookmarks.jsonl`: one line per bookmark click on X,
//! naming the library page it saved, and the fold the Library shows.
//!
//! slice: x-bookmarks
//! why: Whether a page came from a bookmark on X is not something its key,
//!      its intake line or its text says, and a page already in the library
//!      gets no new intake line, so the clicks are kept in a log of their
//!      own, append only like every attempt log. It never decides whether
//!      a page exists: `added.jsonl` and the snapshots still do, so a torn
//!      or unknown line costs only the mark it carried. Each line names the
//!      page by the key the library holds it under, and the fold keeps per
//!      page when it was last bookmarked and which posts were clicked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::archive::Lock;
use crate::error::Error;
use crate::jsonl::{self, Appender};
use crate::metadata;

pub const FILE: &str = "xbookmarks.jsonl";
pub const SCHEMA_VERSION: u32 = 1;

pub fn path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(FILE)
}

/// One line. A line with `post` is a bookmark click; a later reading of
/// the thread adds what it learned as a line without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    #[serde(deserialize_with = "jsonl::schema::<_, SCHEMA_VERSION>")]
    pub schema_version: u32,
    /// The library page's key.
    pub url: String,
    pub at: Timestamp,
    /// The clicked post's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    /// Post numbers of the page's thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub posts: Vec<String>,
}

impl Line {
    /// A click on post `post`, number `id`, saved as page `url`.
    pub fn click(url: &str, at: Timestamp, post: &str, id: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            at,
            post: Some(post.to_owned()),
            posts: vec![id.to_owned()],
        }
    }
}

/// Appends `line` for a caller holding the archive lock.
pub fn append(root: &Path, line: &Line, lock: &Lock) -> Result<(), Error> {
    Appender::open_locked(root, path(root), lock)?.append_locked(line, lock)
}

/// What the log says of one page, as the Library is served it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mark {
    /// The latest click's time.
    pub bookmarked_at: Timestamp,
    /// The clicked posts, the latest click first, each once.
    pub clicked: Vec<String>,
}

/// Every page's mark by its key; a missing log is none. A line this build
/// cannot read is skipped, as every log's reader does.
pub fn marks(root: &Path) -> Result<HashMap<String, Mark>, Error> {
    Ok(fold(&jsonl::text(&path(root), "read the X log")?))
}

fn fold(text: &str) -> HashMap<String, Mark> {
    let (mut lines, _) = jsonl::records::<Line>(text);
    // Latest first; a stable sort keeps the later of two equal times first.
    lines.reverse();
    lines.sort_by_key(|line| std::cmp::Reverse(line.at));
    let mut marks: HashMap<String, Mark> = HashMap::new();
    for line in lines {
        let Some(post) = line.post else { continue };
        let mark = marks.entry(line.url).or_insert_with(|| Mark {
            bookmarked_at: line.at,
            clicked: Vec::new(),
        });
        if !mark.clicked.contains(&post) {
            mark.clicked.push(post);
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://x.com/example_author/status/102";
    const OTHER: &str = "https://x.com/example_author/status/103";

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn line(post: &str, when: &str) -> String {
        let id = post.rsplit('/').next().unwrap();
        serde_json::to_string(&Line::click(PAGE, at(when), post, id)).unwrap()
    }

    #[test]
    fn a_click_line_says_the_page_the_post_and_its_number() {
        assert_eq!(
            serde_json::to_value(Line::click(PAGE, at("2026-10-27T09:00:00Z"), PAGE, "102"))
                .unwrap(),
            serde_json::json!({"schema_version": 1, "url": PAGE, "at": "2026-10-27T09:00:00Z",
                "post": PAGE, "posts": ["102"]})
        );
    }

    #[test]
    fn the_latest_click_dates_the_page_and_leads_its_clicked_posts() {
        let text = [
            line(PAGE, "2026-10-27T09:00:00Z"),
            line(OTHER, "2026-10-27T10:00:00Z"),
            line(PAGE, "2026-10-27T08:00:00Z"),
        ]
        .join("\n");
        let marks = fold(&text);
        assert_eq!(
            marks[PAGE],
            Mark {
                bookmarked_at: at("2026-10-27T10:00:00Z"),
                clicked: vec![OTHER.to_owned(), PAGE.to_owned()],
            }
        );
    }

    #[test]
    fn a_torn_or_other_schema_line_is_skipped_and_a_line_without_a_post_is_no_click() {
        let thread = format!(
            r#"{{"schema_version":1,"url":"{OTHER}","at":"2026-10-27T11:00:00Z","posts":["1","2"]}}"#
        );
        let text = [
            line(PAGE, "2026-10-27T09:00:00Z"),
            format!(r#"{{"schema_version":2,"url":"{PAGE}","at":"2026-10-27T12:00:00Z","post":"{PAGE}"}}"#),
            thread,
            r#"{"schema_version":1,"url":"#.to_owned(),
        ]
        .join("\n");
        let marks = fold(&text);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert_eq!(marks[PAGE].bookmarked_at, at("2026-10-27T09:00:00Z"));
    }
}
