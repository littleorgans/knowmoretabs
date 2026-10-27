//! The X log, `pages/xbookmarks.jsonl`: one line per bookmark click on X,
//! naming the library page it saved, and the fold the Library shows.
//!
//! slice: x-bookmarks
//! why: Whether a page came from a bookmark on X is not something its key,
//!      its intake line or its text says, and a page already in the library
//!      gets no new intake line, so the clicks are kept in a log of their
//!      own, append only like every attempt log. It never decides whether
//!      a page exists: `added.jsonl` and the snapshots still do, so a torn
//!      or unknown line costs only the mark it carried, and a log that
//!      cannot be read at all costs only every mark: the Library still
//!      opens. Each line names the page by the key the library holds it
//!      under, and the fold keeps per page when it was last bookmarked,
//!      which posts were clicked, every post number its thread was read
//!      with, so a later bookmark anywhere in that thread finds it, and the
//!      pages it links to, so the Library ties the article to the post.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::archive::{Archive, Lock};
use crate::capture::Log;
use crate::error::Error;
use crate::jsonl::{self, Appender};
use crate::metadata;

pub const FILE: &str = "xbookmarks.jsonl";
pub const SCHEMA_VERSION: u32 = 1;

pub fn path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(FILE)
}

/// One line. A line with `post` is a bookmark click; a later reading of
/// the thread adds what it learned as a line without one: its post
/// numbers, or the library pages it links to.
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
    /// Keys of library pages the thread links to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

impl Line {
    /// A click on post `post` saved as page `url`, read with the thread's
    /// post numbers `posts`, the clicked one among them.
    pub fn click(url: &str, at: Timestamp, post: &str, posts: Vec<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            at,
            post: Some(post.to_owned()),
            posts,
            links: Vec::new(),
        }
    }

    /// What a later reading of page `url`'s thread learned: its post
    /// numbers.
    pub fn thread(url: &str, at: Timestamp, posts: Vec<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            at,
            post: None,
            posts,
            links: Vec::new(),
        }
    }

    /// Page `url`'s thread links to the library page `key`.
    pub fn link(url: &str, at: Timestamp, key: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            at,
            post: None,
            posts: Vec::new(),
            links: vec![key.to_owned()],
        }
    }
}

/// Appends `line` for a caller holding the archive lock.
pub fn append(root: &Path, line: &Line, lock: &Lock) -> Result<(), Error> {
    Appender::open_locked(root, path(root), lock)?.append_locked(line, lock)
}

/// Appends `line` in a hold of the archive lock of its own.
pub fn learned(root: &Path, line: &Line) -> Result<(), Error> {
    let archive = Archive::open(root)?;
    let lock = archive.lock(|| {})?;
    append(root, line, &lock)
}

/// What the log says of one page, as the Library is served it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mark {
    /// The latest click's time.
    pub bookmarked_at: Timestamp,
    /// The clicked posts, the latest click first, each once.
    pub clicked: Vec<String>,
    /// Every post number the page's thread was read with, each once; the
    /// save's to resolve, not the Library's.
    #[serde(skip)]
    pub thread: Vec<String>,
    /// The library pages the thread links to, each once.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

/// The fold of the whole log.
#[derive(Debug, Default)]
pub struct Marks {
    /// Each bookmarked page's mark, by its key.
    pub pages: HashMap<String, Mark>,
    /// Lines this build could not read, skipped.
    pub unreadable: usize,
}

/// The whole log folded; a missing log is none. A line this build cannot
/// read is counted and skipped, as every log's reader does.
pub fn read(root: &Path) -> Result<Marks, Error> {
    Ok(fold(&jsonl::text(&path(root), "read the X log")?))
}

/// Every page's mark by its key, and none when the log cannot be read:
/// the X log never decides whether the Library opens. What was skipped,
/// or why there are no marks, is said on `log`.
pub fn for_library(root: &Path, log: Log) -> HashMap<String, Mark> {
    match read(root) {
        Ok(marks) => {
            if let Some(note) = jsonl::unreadable_note(marks.unreadable, &path(root)) {
                log.warn(&note);
            }
            marks.pages
        }
        Err(err) => {
            log.warn(&format!("{err}; no page shows its bookmark on X"));
            HashMap::new()
        }
    }
}

fn fold(text: &str) -> Marks {
    let (mut lines, unreadable) = jsonl::records::<Line>(text);
    let mut learned: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    for line in &lines {
        let (thread, linked) = learned.entry(line.url.clone()).or_default();
        union(thread, &line.posts);
        union(linked, &line.links);
    }
    // Latest first; a stable sort keeps the later of two equal times first.
    lines.reverse();
    lines.sort_by_key(|line| std::cmp::Reverse(line.at));
    let mut pages: HashMap<String, Mark> = HashMap::new();
    for line in lines {
        let Some(post) = line.post else { continue };
        let mark = pages.entry(line.url).or_insert_with_key(|url| {
            let (thread, linked) = learned.remove(url).unwrap_or_default();
            Mark {
                bookmarked_at: line.at,
                clicked: Vec::new(),
                thread,
                links: linked,
            }
        });
        if !mark.clicked.contains(&post) {
            mark.clicked.push(post);
        }
    }
    Marks { pages, unreadable }
}

/// Adds to `all` each of `more` it does not hold yet, in order.
fn union(all: &mut Vec<String>, more: &[String]) {
    for item in more {
        if !all.contains(item) {
            all.push(item.clone());
        }
    }
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
        serde_json::to_string(&Line::click(PAGE, at(when), post, vec![id.to_owned()])).unwrap()
    }

    #[test]
    fn a_click_line_says_the_page_the_post_and_its_number() {
        assert_eq!(
            serde_json::to_value(Line::click(
                PAGE,
                at("2026-10-27T09:00:00Z"),
                PAGE,
                vec!["102".to_owned()]
            ))
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
        let marks = fold(&text).pages;
        assert_eq!(
            marks[PAGE],
            Mark {
                bookmarked_at: at("2026-10-27T10:00:00Z"),
                clicked: vec![OTHER.to_owned(), PAGE.to_owned()],
                thread: vec!["102".to_owned(), "103".to_owned()],
                links: Vec::new(),
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
        assert_eq!(marks.unreadable, 2, "the torn and the other schema line");
        let marks = marks.pages;
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert_eq!(marks[PAGE].bookmarked_at, at("2026-10-27T09:00:00Z"));
    }

    #[test]
    fn a_pages_thread_is_every_post_its_clicks_and_later_reads_named_each_once() {
        let thread = |posts: &[&str]| {
            serde_json::to_string(&Line::thread(
                PAGE,
                at("2026-10-27T09:00:02Z"),
                posts.iter().map(|id| (*id).to_owned()).collect(),
            ))
            .unwrap()
        };
        let text = [
            thread(&["100"]),
            line(PAGE, "2026-10-27T09:00:00Z"),
            thread(&["101", "102", "103"]),
        ]
        .join("\n");
        let mark = &fold(&text).pages[PAGE];
        assert_eq!(mark.thread, ["100", "102", "101", "103"]);
        assert_eq!(mark.clicked, [PAGE], "a thread line is no click");
        assert_eq!(
            serde_json::to_value(mark).unwrap()["thread"],
            serde_json::Value::Null,
            "the Library is not served the thread"
        );
    }

    #[test]
    fn a_pages_links_are_every_page_its_link_lines_named_each_once_and_served() {
        const ARTICLE: &str = "https://blog.example.test/post";
        let link = |key: &str| {
            serde_json::to_string(&Line::link(PAGE, at("2026-10-27T09:00:02Z"), key)).unwrap()
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&link(ARTICLE)).unwrap(),
            serde_json::json!({"schema_version": 1, "url": PAGE, "at": "2026-10-27T09:00:02Z",
                "links": [ARTICLE]})
        );
        let text = [
            line(PAGE, "2026-10-27T09:00:00Z"),
            link(ARTICLE),
            link(ARTICLE),
            link("https://video.example.test/watch"),
        ]
        .join("\n");
        let mark = &fold(&text).pages[PAGE];
        assert_eq!(mark.clicked, [PAGE], "a link line is no click");
        assert_eq!(mark.links, [ARTICLE, "https://video.example.test/watch"]);
        assert_eq!(
            serde_json::to_value(mark).unwrap()["links"],
            serde_json::json!([ARTICLE, "https://video.example.test/watch"])
        );
        let alone = fold(&line(PAGE, "2026-10-27T09:00:00Z")).pages;
        assert!(
            serde_json::to_value(&alone[PAGE])
                .unwrap()
                .get("links")
                .is_none(),
            "none served when none"
        );
    }

    #[test]
    fn a_log_that_cannot_be_read_is_no_marks() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            for_library(root.path(), Log::default()).is_empty(),
            "no log"
        );
        std::fs::create_dir_all(path(root.path())).unwrap();
        assert!(read(root.path()).is_err(), "a directory is not a log");
        let quiet = Log {
            quiet: true,
            verbose: false,
        };
        assert!(for_library(root.path(), quiet).is_empty());
    }
}
