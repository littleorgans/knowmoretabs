//! The page text store: `pages/content.jsonl`, one line per capture
//! attempt, and `pages/content/<sha256>.md`, one markdown file per page.
//!
//! slice: content
//! why: Page text is the most private thing the archive keeps, and later
//!      readers (a keyword index, agents) must be able to trust it without
//!      knowing how it was captured. So the log is the record and a file is
//!      only ever replaced whole, under the archive lock and before its line,
//!      so a crash leaves at worst an orphan file the next attempt replaces.
//!      A line from a newer schema is not guessed at but counted, a status
//!      this build does not know reads as unknown, and front matter keys a
//!      later build added survive a refetch. An unchanged page is not
//!      rewritten, so its file keeps the date its text last changed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::archive;
use crate::content_fetch;
use crate::error::Error;
use crate::jsonl::{self, Keyed};
use crate::metadata;
use crate::targets::{Outcome, Recorded};

pub const LOG_FILE: &str = "content.jsonl";
pub const DIR: &str = "content";
/// The one version of the attempt line and front matter this build writes
/// and reads.
pub const SCHEMA_VERSION: u32 = 1;

pub fn log_path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(LOG_FILE)
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(DIR)
}

/// A page's file name: the lowercase hex SHA-256 of its exact URL.
pub fn file_name(url: &str) -> String {
    format!("{}.md", sha256_hex(url.as_bytes()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

/// What became of a page. Only `error`, and `not_html` for a type a later
/// build reads, is tried again on a plain run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Thin,
    /// The page is an image file: no text, its copy is the page's image.
    Media,
    EmptyShell,
    BehindLogin,
    Paywalled,
    Blocked,
    NotFound,
    NotHtml,
    Skipped,
    Error,
    Unavailable,
    /// A status a newer build wrote: read as final, never rewritten.
    #[serde(other)]
    Unknown,
}

impl Outcome for Status {
    fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Thin => "thin",
            Self::Media => "media",
            Self::EmptyShell => "empty_shell",
            Self::BehindLogin => "behind_login",
            Self::Paywalled => "paywalled",
            Self::Blocked => "blocked",
            Self::NotFound => "not_found",
            Self::NotHtml => "not_html",
            Self::Skipped => "skipped",
            Self::Error => "error",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }
}

/// The route that made an attempt: the generic web route, the X post API,
/// the GitHub API through `gh`, `YouTube` through yt-dlp, or a headless
/// browser rendering a page the web route read as thin or empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Web,
    X,
    Github,
    Youtube,
    Headless,
    #[serde(other)]
    Other,
}

/// Whose access a page was read with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Public,
    #[serde(other)]
    Other,
}

/// One attempt, written and read in this one shape. Absent fields are not
/// written; fields a newer build adds are ignored on reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    #[serde(deserialize_with = "jsonl::schema::<_, SCHEMA_VERSION>")]
    pub schema_version: u32,
    pub url: String,
    pub attempted_at: Timestamp,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    /// The SHA-256 of the markdown body, when a file was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Which run this is for the page: 1, then one more for each run that
    /// tries again after an `error`.
    #[serde(default = "jsonl::first_attempt")]
    pub attempt: u32,
}

/// Now, to the second, as a line records when it was attempted.
pub fn now() -> Timestamp {
    let now = Timestamp::now();
    Timestamp::from_second(now.as_second()).unwrap_or(now)
}

impl Line {
    pub fn new(url: &str, status: Status) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            attempted_at: now(),
            status,
            tier: None,
            extractor: None,
            extractor_version: None,
            reason: None,
            http_status: None,
            final_url: None,
            access: None,
            content_sha256: None,
            chars: None,
            lang: None,
            attempt: 1,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl Keyed for Line {
    fn key(&self) -> &str {
        &self.url
    }
}

pub type Log = jsonl::Latest<Line>;

/// The whole log; a missing file is an empty log.
pub fn read(root: &Path) -> Result<Log, Error> {
    jsonl::read(&log_path(root), "read the content log")
}

/// How the planner sees a page's latest line.
pub fn recorded(log: &Log, url: &str) -> Recorded {
    match log.pages.get(url) {
        None => Recorded::Nothing,
        Some(line) if line.status == Status::Error => Recorded::Failed,
        Some(line) if content_fetch::outdated(line) => Recorded::Outdated,
        Some(_) => Recorded::Final,
    }
}

/// Whether a kept text is the page, or all a page gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    Full,
    Thin,
}

/// Who made the captions a video's transcript was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Captions {
    Manual,
    Automatic,
}

/// A page's text and what the front matter says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub title: Option<String>,
    /// `dom_smoothie 0.18.2` or `full-page`.
    pub extractor: String,
    pub completeness: Completeness,
    pub chars: usize,
    /// A video's caption track; its language is the line's `lang`.
    pub captions: Option<Captions>,
    /// Ends with one newline.
    pub markdown: String,
}

/// The keys this build writes, in the order it writes them. Any other key
/// in an existing file is carried over when the file is replaced.
const KNOWN_KEYS: [&str; 12] = [
    "schema_version",
    "url",
    "final_url",
    "title",
    "fetched_at",
    "tier",
    "extractor",
    "access",
    "completeness",
    "lang",
    "captions",
    "chars",
];

/// A content file: front matter whose values are JSON (so YAML reads it
/// too, and no YAML parser is needed), then the markdown body.
pub fn render(line: &Line, page: &Page, carried: &BTreeMap<String, Value>) -> String {
    let to = |value: Value| Some(value);
    let known: [(&str, Option<Value>); 12] = [
        ("schema_version", to(SCHEMA_VERSION.into())),
        ("url", to(line.url.clone().into())),
        ("final_url", line.final_url.clone().map(Value::from)),
        ("title", page.title.clone().map(Value::from)),
        ("fetched_at", to(line.attempted_at.to_string().into())),
        ("tier", line.tier.and_then(|t| serde_json::to_value(t).ok())),
        ("extractor", to(page.extractor.clone().into())),
        (
            "access",
            line.access.and_then(|a| serde_json::to_value(a).ok()),
        ),
        (
            "completeness",
            to(match page.completeness {
                Completeness::Full => "full",
                Completeness::Thin => "thin",
            }
            .into()),
        ),
        ("lang", line.lang.clone().map(Value::from)),
        (
            "captions",
            page.captions.and_then(|c| serde_json::to_value(c).ok()),
        ),
        ("chars", to(page.chars.into())),
    ];
    let mut text = String::from("---\n");
    let extra = carried
        .iter()
        .filter(|(key, _)| !KNOWN_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), Some(value.clone())));
    for (key, value) in known.into_iter().chain(extra) {
        if let Some(value) = value {
            let _ = writeln!(text, "{key}: {value}");
        }
    }
    text.push_str("---\n");
    text.push_str(&page.markdown);
    text
}

/// The front matter and the body of a content file, or `None` when it has
/// no front matter. A front matter line that is not `key: <JSON>` is
/// dropped.
pub fn parse(text: &str) -> Option<(BTreeMap<String, Value>, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let end = if rest.starts_with("---\n") {
        0
    } else {
        rest.find("\n---\n")? + 1
    };
    let mut front = BTreeMap::new();
    for line in rest[..end].lines() {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(value) {
            front.insert(key.trim().to_owned(), value);
        }
    }
    Some((front, &rest[end + "---\n".len()..]))
}

/// Where a run writes: the log, and the directory of content files.
#[derive(Debug)]
pub struct Store(jsonl::Store<Line>);

impl Store {
    /// Opens the log and the content directory, both private, and removes
    /// what an interrupted run left staged there, all under one hold of the
    /// archive lock, so every write `content` makes holds it.
    pub fn open(root: &Path) -> Result<Self, Error> {
        jsonl::Store::open(root, log_path(root), dir(root)).map(Self)
    }

    /// Records one attempt: the page's file first, when there is text to
    /// keep, then its line, both under one hold of the archive lock. A file
    /// whose body is unchanged is left as it is. Returns the line written.
    pub fn record(&mut self, mut line: Line, page: Option<&Page>) -> Result<Line, Error> {
        self.0.record(|dir| {
            let Some(page) = page else {
                return Ok(line);
            };
            let hash = sha256_hex(page.markdown.as_bytes());
            let path = dir.join(file_name(&line.url));
            let existing = match fs::read(&path) {
                Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                Err(err) if err.kind() == ErrorKind::NotFound => None,
                Err(err) => return Err(Error::io("read", &path)(err)),
            };
            let previous = existing.as_deref().and_then(parse);
            let unchanged = previous
                .as_ref()
                .is_some_and(|(_, body)| sha256_hex(body.as_bytes()) == hash);
            line.content_sha256 = Some(hash);
            line.chars = Some(page.chars);
            if !unchanged {
                let carried = previous.map(|(front, _)| front).unwrap_or_default();
                archive::replace_file(&path, render(&line, page, &carried).as_bytes())?;
            }
            Ok(line)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(markdown: &str) -> Page {
        Page {
            title: Some("A \"quoted\" title".to_owned()),
            extractor: "dom_smoothie 0.18.2".to_owned(),
            completeness: Completeness::Full,
            chars: 42,
            captions: None,
            markdown: markdown.to_owned(),
        }
    }

    fn ok_line(url: &str) -> Line {
        let mut line = Line::new(url, Status::Ok);
        line.tier = Some(Tier::Web);
        line.access = Some(Access::Public);
        line.final_url = Some(format!("{url}final"));
        line.lang = Some("en".to_owned());
        line
    }

    #[test]
    fn a_file_is_named_by_the_hash_of_its_exact_url() {
        assert_eq!(
            file_name("https://a.test/"),
            format!("{}.md", sha256_hex(b"https://a.test/"))
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(file_name("https://a.test/"), file_name("https://a.test/#x"));
    }

    #[test]
    fn an_attempt_line_writes_what_it_has_and_reads_back() {
        let mut line = ok_line("https://a.test/");
        line.extractor = Some("dom_smoothie".to_owned());
        line.extractor_version = Some("0.18.2".to_owned());
        line.http_status = Some(200);
        line.chars = Some(8642);
        line.content_sha256 = Some("ab".repeat(32));
        let text = serde_json::to_string(&line).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "access",
                "attempt",
                "attempted_at",
                "chars",
                "content_sha256",
                "extractor",
                "extractor_version",
                "final_url",
                "http_status",
                "lang",
                "schema_version",
                "status",
                "tier",
                "url"
            ]
        );
        assert_eq!(value["status"], "ok");
        assert_eq!(value["tier"], "web");
        assert_eq!(value["access"], "public");
        assert!(!value["attempted_at"].as_str().unwrap().contains('.'));
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);

        let skipped = Line::new("https://b.test/", Status::Skipped).with_reason("search results");
        let value = serde_json::to_value(&skipped).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "attempt",
                "attempted_at",
                "reason",
                "schema_version",
                "status",
                "url"
            ]
        );
    }

    #[test]
    fn the_latest_line_wins_unknown_statuses_stand_and_other_schemas_are_unreadable() {
        let text = concat!(
            r#"{"schema_version":1,"url":"https://a.test/","attempted_at":"2026-10-07T09:00:00Z","status":"error","reason":"timeout","attempt":1}"#,
            "\n",
            r#"{"schema_version":1,"url":"https://a.test/","attempted_at":"2026-10-07T10:00:00Z","status":"ok","attempt":2,"added_later":true}"#,
            "\n",
            r#"{"schema_version":1,"url":"https://b.test/","attempted_at":"2026-10-07T10:00:00Z","status":"archived"}"#,
            "\n",
            r#"{"schema_version":2,"url":"https://c.test/","attempted_at":"2026-10-07T10:00:00Z","status":"ok"}"#,
            "\n",
            r#"{"url":"https://d.test/","attempted_at":"2026-10-07T10:00:00Z","status":"ok"}"#,
            "\n",
            r#"{"schema_version":1,"url":"https://e.test/","attempted_at":"2026-10-0"#,
        );
        let log: Log = jsonl::parse(text);
        assert_eq!(log.unreadable, 3, "schema 2, no schema, torn");
        assert_eq!(log.pages.len(), 2);
        assert_eq!(log.pages["https://a.test/"].status, Status::Ok);
        assert_eq!(log.pages["https://a.test/"].attempt, 2);
        assert_eq!(log.pages["https://b.test/"].status, Status::Unknown);
        assert_eq!(log.pages["https://b.test/"].attempt, 1);
        assert_eq!(recorded(&log, "https://a.test/"), Recorded::Final);
        assert_eq!(recorded(&log, "https://b.test/"), Recorded::Final);
        assert_eq!(recorded(&log, "https://z.test/"), Recorded::Nothing);
    }

    #[test]
    fn an_error_is_retried_and_everything_else_stands() {
        let mut log = Log::default();
        for (url, status) in [
            ("e", Status::Error),
            ("u", Status::Unavailable),
            ("t", Status::Thin),
            ("m", Status::Media),
        ] {
            log.pages.insert(url.to_owned(), Line::new(url, status));
        }
        assert_eq!(recorded(&log, "e"), Recorded::Failed);
        assert_eq!(recorded(&log, "u"), Recorded::Final);
        assert_eq!(recorded(&log, "t"), Recorded::Final);
        assert_eq!(recorded(&log, "m"), Recorded::Final);
    }

    #[test]
    fn an_image_page_is_media_in_the_log() {
        let line = Line::new("https://a.test/photo.jpg", Status::Media);
        let text = serde_json::to_string(&line).unwrap();
        assert!(text.contains(r#""status":"media""#), "{text}");
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
        assert_eq!(Status::Media.word(), "media");
    }

    #[test]
    fn front_matter_round_trips_and_carries_keys_it_does_not_know() {
        let line = ok_line("https://a.test/");
        let mut carried = BTreeMap::new();
        carried.insert("rating".to_owned(), Value::from(5));
        carried.insert("labels".to_owned(), serde_json::json!(["a", "b"]));
        carried.insert("title".to_owned(), Value::from("an older title"));
        let text = render(&line, &page("# Title\n\nBody: with --- in it\n"), &carried);
        assert!(text.starts_with("---\nschema_version: 1\nurl: \"https://a.test/\"\n"));
        let (front, body) = parse(&text).unwrap();
        assert_eq!(body, "# Title\n\nBody: with --- in it\n");
        assert_eq!(
            front["title"], "A \"quoted\" title",
            "known keys are this build's"
        );
        assert_eq!(front["rating"], 5);
        assert_eq!(front["labels"], serde_json::json!(["a", "b"]));
        assert_eq!(front["completeness"], "full");
        assert_eq!(front["chars"], 42);
        assert_eq!(front["fetched_at"], line.attempted_at.to_string());
        let again = render(&line, &page(body), &front);
        assert_eq!(again, text, "a second round trip changes nothing");
        assert!(parse("no front matter").is_none());
        assert_eq!(parse("---\n---\nbody").unwrap().1, "body");
    }

    #[test]
    fn a_rendered_page_says_headless_in_its_line_and_front_matter() {
        let mut line = ok_line("https://a.test/");
        line.tier = Some(Tier::Headless);
        let text = serde_json::to_string(&line).unwrap();
        assert!(text.contains(r#""tier":"headless""#));
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
        let (front, _) = parse(&render(&line, &page("Body\n"), &BTreeMap::new()))
            .map(|(f, b)| (f, b.to_owned()))
            .unwrap();
        assert_eq!(front["tier"], "headless");
    }

    #[test]
    fn a_video_records_its_caption_kind_and_an_older_one_is_not_carried() {
        let mut line = ok_line("https://www.youtube.com/watch?v=aBc-12_xYz9");
        line.tier = Some(Tier::Youtube);
        let captioned = Page {
            captions: Some(Captions::Automatic),
            ..page("Body\n")
        };
        let (front, _) = parse(&render(&line, &captioned, &BTreeMap::new()))
            .map(|(f, b)| (f, b.to_owned()))
            .unwrap();
        assert_eq!(front["tier"], "youtube");
        assert_eq!(front["lang"], "en");
        assert_eq!(front["captions"], "automatic");
        let refetched = render(&line, &page("Body\n"), &front);
        assert!(
            !refetched.contains("captions:"),
            "a page without captions now drops the old key"
        );
    }

    #[test]
    fn a_store_writes_the_file_then_the_line_and_leaves_an_unchanged_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::open(root).unwrap();
        let first = store
            .record(ok_line("https://a.test/"), Some(&page("Body\n")))
            .unwrap();
        let path = super::dir(root).join(file_name("https://a.test/"));
        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(
            first.content_sha256.as_deref(),
            Some(sha256_hex(b"Body\n").as_str())
        );
        assert_eq!(first.chars, Some(42));

        // An unchanged body leaves the file untouched; a newer build's key
        // added by hand survives a change of body.
        let with_key = written.replacen("---\n", "---\nrating: 5\n", 1);
        fs::write(&path, &with_key).unwrap();
        let mut later = ok_line("https://a.test/");
        later.attempted_at = Timestamp::from_second(first.attempted_at.as_second() + 60).unwrap();
        store.record(later.clone(), Some(&page("Body\n"))).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), with_key);
        store.record(later, Some(&page("New body\n"))).unwrap();
        let (front, body) = parse(&fs::read_to_string(&path).unwrap())
            .map(|(f, b)| (f, b.to_owned()))
            .unwrap();
        assert_eq!(body, "New body\n");
        assert_eq!(front["rating"], 5);

        store
            .record(Line::new("https://b.test/", Status::NotFound), None)
            .unwrap();
        assert!(!super::dir(root).join(file_name("https://b.test/")).exists());
        let log = read(root).unwrap();
        assert_eq!((log.pages.len(), log.unreadable), (2, 0));
        assert_eq!(
            fs::read_to_string(log_path(root)).unwrap().lines().count(),
            4
        );
    }

    #[cfg(unix)]
    #[test]
    fn files_and_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        archive::create_private_dir(&root).unwrap();
        let mut store = Store::open(&root).unwrap();
        store
            .record(ok_line("https://a.test/"), Some(&page("Body\n")))
            .unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&super::dir(&root)), 0o700);
        assert_eq!(mode(&log_path(&root)), 0o600);
        assert_eq!(
            mode(&super::dir(&root).join(file_name("https://a.test/"))),
            0o600
        );
    }
}
