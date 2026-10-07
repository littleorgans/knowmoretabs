//! The `YouTube` route: a video's description and captions read through
//! yt-dlp, and kept as markdown.
//!
//! slice: content
//! why: A video page is an app to plain HTTP, and its words are in caption
//!      tracks only `YouTube`'s own player knows how to ask for. yt-dlp is the
//!      tool the owner already trusts for that, so this route runs it, and
//!      nothing more: an argument vector, no shell, no configuration file
//!      and no cookies of any kind, one video at a time with a second
//!      between its requests, a minute at most per call, into a private
//!      scratch directory removed afterwards. It is given the plain watch
//!      address rebuilt from the video's id, never the library address.
//!      The first call describes the video; the second downloads the one
//!      caption track the owner's rule picks from that description. A video
//!      `YouTube` will not show without signing in keeps no text, a removed
//!      or private one is final, and a bot check or a rate limit is
//!      `blocked`, never hammered. Without yt-dlp and a JavaScript runtime
//!      for it, `YouTube` pages wait for them, unrecorded.

use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::content_fetch::{self, BODY_CAP, Capture};
use crate::content_store::{Captions, Line, Page, Status, Tier};
use crate::tools::{self, Limits, Probe};
use crate::triage::plural;
use crate::youtube;
use crate::youtube_page::{self, Choice, Info};

/// The tool, as found on `PATH` and recorded as the extractor.
pub const NAME: &str = "yt-dlp";
/// The host every video's requests go to: one queue, one video at a time.
pub const HOST: &str = "www.youtube.com";
/// The JavaScript runtimes yt-dlp can use for `YouTube`, the one it prefers
/// first.
pub const RUNTIMES: [&str; 2] = ["deno", "node"];
/// The least a video takes: describing it makes at least three requests a
/// second apart.
pub const SECONDS_AT_LEAST: usize = 2;
const TIMEOUT: Duration = Duration::from_secs(60);
/// yt-dlp's own progress lines: read and dropped past this.
const STDOUT_CAP: usize = 64 * 1024;
/// Info JSON includes translation tracks and their long addresses. This
/// private tool output gets roughly six times the observed 10.9 MB size,
/// while keeping memory bounded independently of page and caption bodies.
const INFO_JSON_CAP: usize = 64 * 1024 * 1024;
/// A second between yt-dlp's requests for one video.
const SLEEP: &str = "1";
/// The scratch file names, before yt-dlp's own suffixes.
const VIDEO: &str = "video";
const CAPTIONS: &str = "captions";

/// The yt-dlp this run uses, and the runtime it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtDlp {
    pub path: PathBuf,
    /// What `yt-dlp --version` prints: `2026.08.19`.
    pub version: Option<String>,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub name: &'static str,
    pub path: PathBuf,
}

/// Whether `YouTube` videos can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready(YtDlp),
    /// yt-dlp is not on `PATH`.
    Missing,
    /// yt-dlp, at this path with this version, but no runtime for it.
    NoRuntime(PathBuf, Option<String>),
}

impl Readiness {
    /// yt-dlp and a runtime on `PATH`, and yt-dlp's version. Runs nothing
    /// but `yt-dlp --version`, which sends nothing.
    pub fn check(probe: &impl Probe) -> Self {
        Self::found(probe, true)
    }

    /// yt-dlp and a runtime on `PATH`, nothing run: what a dry run plans
    /// with.
    pub fn assumed(probe: &impl Probe) -> Self {
        Self::found(probe, false)
    }

    fn found(probe: &impl Probe, versioned: bool) -> Self {
        let Some(path) = probe.find(NAME) else {
            return Self::Missing;
        };
        let version = if versioned {
            probe.version(&path)
        } else {
            None
        };
        let runtime = RUNTIMES.into_iter().find_map(|name| {
            Some(Runtime {
                name,
                path: probe.find(name)?,
            })
        });
        match runtime {
            Some(runtime) => Self::Ready(YtDlp {
                path,
                version,
                runtime,
            }),
            None => Self::NoRuntime(path, version),
        }
    }

    pub fn tool(&self) -> Option<&YtDlp> {
        match self {
            Self::Ready(tool) => Some(tool),
            _ => None,
        }
    }
}

/// The report's note for `waiting` `YouTube` pages left for another run.
pub fn waiting_note(waiting: usize) -> String {
    let pages = plural(waiting, "YouTube page");
    format!("{pages} waiting for yt-dlp (see knowmoretabs doctor)")
}

/// How yt-dlp's reason for failing maps to a status, matched without
/// regard to case, first match wins: a bot check before a sign in, a
/// private video before a sign in, an age check before a sign in.
const REFUSALS: [(&str, Status, &str); 24] = [
    (
        "not a bot",
        Status::Blocked,
        "YouTube asked to confirm this is not a bot",
    ),
    ("captcha", Status::Blocked, "YouTube asked for a captcha"),
    (
        "http error 429",
        Status::Blocked,
        "rate limited by YouTube (HTTP 429)",
    ),
    (
        "too many requests",
        Status::Blocked,
        "rate limited by YouTube (HTTP 429)",
    ),
    (
        "try again later",
        Status::Blocked,
        "rate limited by YouTube",
    ),
    ("private video", Status::NotFound, "private video"),
    ("video is private", Status::NotFound, "private video"),
    ("members-only", Status::BehindLogin, "members only"),
    ("channel's members", Status::BehindLogin, "members only"),
    ("confirm your age", Status::BehindLogin, "age restricted"),
    ("age-restricted", Status::BehindLogin, "age restricted"),
    (
        "inappropriate for some users",
        Status::BehindLogin,
        "age restricted",
    ),
    ("premium", Status::BehindLogin, "YouTube Premium only"),
    (
        "available in your country",
        Status::Blocked,
        "not available in this country",
    ),
    (
        "geo restriction",
        Status::Blocked,
        "not available in this country",
    ),
    ("live event will begin", Status::Error, "not started yet"),
    ("premieres in", Status::Error, "not started yet"),
    ("sign in", Status::BehindLogin, "sign in required"),
    ("removed", Status::NotFound, "video removed"),
    ("terminated", Status::NotFound, "account terminated"),
    (
        "no longer available",
        Status::NotFound,
        "video no longer available",
    ),
    ("video unavailable", Status::NotFound, "video unavailable"),
    (
        "video is unavailable",
        Status::NotFound,
        "video unavailable",
    ),
    (
        "video is not available",
        Status::NotFound,
        "video unavailable",
    ),
];

/// What a yt-dlp call that did not exit 0 says about the video: its exit
/// `code` and its `complaint`, the one line of reason.
fn refusal(code: Option<i32>, complaint: &str) -> (Status, String) {
    let lower = complaint.to_lowercase();
    if let Some((_, status, reason)) = REFUSALS.iter().find(|(said, ..)| lower.contains(said)) {
        return (*status, (*reason).to_owned());
    }
    let reason = match (
        code,
        complaint.strip_prefix("ERROR:").unwrap_or(complaint).trim(),
    ) {
        (_, said) if !said.is_empty() => said.to_owned(),
        (Some(code), _) => format!("{NAME} exited with code {code}"),
        (None, _) => format!("{NAME} was stopped by a signal"),
    };
    (Status::Error, reason)
}

/// Reads video `id` for library page `raw`. Never fails: a failure is a
/// capture too. Not retried in the run: yt-dlp retries its own requests,
/// and a page left `error` is tried again on the next run.
pub fn capture(tool: &YtDlp, raw: &str, id: &str) -> Capture {
    let attempt = Attempt { tool, raw };
    attempt.read(id).unwrap_or_else(|ended| *ended)
}

/// One attempt at one video.
struct Attempt<'a> {
    tool: &'a YtDlp,
    raw: &'a str,
}

impl Attempt<'_> {
    fn line(&self, status: Status) -> Line {
        content_fetch::public_line(self.raw, Tier::Youtube, status)
    }

    /// How an attempt ends early: a capture with no text.
    fn ended(&self, status: Status, reason: impl Into<String>) -> Box<Capture> {
        Box::new(Capture {
            line: self.line(status).with_reason(reason),
            page: None,
        })
    }

    fn read(&self, id: &str) -> Result<Capture, Box<Capture>> {
        let scratch = tempfile::Builder::new()
            .prefix("knowmoretabs-yt-dlp-")
            .tempdir()
            .map_err(|err| {
                self.ended(
                    Status::Error,
                    format!("no scratch directory: {}", err.kind()),
                )
            })?;
        let dir = scratch.path();
        self.call(&describe_args(self.tool, dir, id))?;
        let info_path = dir.join(format!("{VIDEO}.info.json"));
        let info: Info =
            serde_json::from_slice(&self.file(&info_path, "video information", INFO_JSON_CAP)?)
                .map_err(|_| self.ended(Status::Error, "yt-dlp's video information is not JSON"))?;
        let mut line = self.line(Status::Ok);
        line.final_url.clone_from(&info.webpage_url);
        let Some(choice) = youtube_page::choose(&info) else {
            line.status = Status::Thin;
            line.reason = Some("no captions".to_owned());
            line.lang.clone_from(&info.language);
            return Ok(self.kept(line, youtube_page::page(&info, &[], None)));
        };
        self.call(&caption_args(dir, &info_path, &choice))?;
        let vtt = self.file(
            &dir.join(format!(
                "{CAPTIONS}.{}.{}",
                choice.key,
                youtube_page::FORMAT
            )),
            "captions",
            BODY_CAP,
        )?;
        let said = youtube_page::transcript(&String::from_utf8_lossy(&vtt), choice.kind);
        if said.is_empty() {
            line.status = Status::Thin;
            line.reason = Some("captions empty".to_owned());
        }
        line.lang = Some(choice.lang().to_owned());
        Ok(self.kept(line, youtube_page::page(&info, &said, Some(choice.kind))))
    }

    /// One bounded yt-dlp run; a run that did not exit 0 ends the attempt.
    fn call(&self, args: &[String]) -> Result<(), Box<Capture>> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let limits = Limits {
            timeout: TIMEOUT,
            stdout_cap: STDOUT_CAP,
        };
        let output = tools::run(&self.tool.path, &args, &[], limits)
            .map_err(|failure| self.ended(Status::Error, failure.reason(NAME)))?;
        if output.code == Some(0) {
            return Ok(());
        }
        let (status, reason) = refusal(output.code, &output.complaint);
        Err(self.ended(status, reason))
    }

    /// Up to `cap` bytes of a file yt-dlp wrote, or the attempt ends.
    fn file(&self, path: &Path, what: &str, cap: usize) -> Result<Vec<u8>, Box<Capture>> {
        let mut bytes = Vec::new();
        match fs::File::open(path)
            .and_then(|file| file.take(cap as u64 + 1).read_to_end(&mut bytes))
        {
            Ok(_) if bytes.len() > cap => Err(self.ended(
                Status::Error,
                format!("yt-dlp's {what} over {} MiB", cap >> 20),
            )),
            Ok(_) => Ok(bytes),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                Err(self.ended(Status::Error, format!("yt-dlp wrote no {what}")))
            }
            Err(err) => Err(self.ended(
                Status::Error,
                format!("reading yt-dlp's {what}: {}", err.kind()),
            )),
        }
    }

    /// `line` with what every kept page records, and the page.
    fn kept(&self, mut line: Line, page: Page) -> Capture {
        line.extractor = Some(NAME.to_owned());
        line.extractor_version.clone_from(&self.tool.version);
        let extractor = match &self.tool.version {
            Some(version) => format!("{NAME} {version}"),
            None => NAME.to_owned(),
        };
        Capture {
            line,
            page: Some(Page { extractor, ..page }),
        }
    }
}

/// What every call says: no configuration file, so none can add cookies,
/// and a second between requests.
fn common(dir: &Path, name: &str) -> Vec<String> {
    let output = dir.join(format!("{name}.%(ext)s"));
    [
        "--ignore-config",
        "--no-playlist",
        "--skip-download",
        "--sleep-requests",
        SLEEP,
        "-o",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([output.to_string_lossy().into_owned()])
    .collect()
}

/// The first call: describe video `id` into `dir`, with the runtime the
/// readiness check found.
fn describe_args(tool: &YtDlp, dir: &Path, id: &str) -> Vec<String> {
    let mut args = common(dir, VIDEO);
    args.extend([
        "--write-info-json".to_owned(),
        "--js-runtimes".to_owned(),
        format!("{}:{}", tool.runtime.name, tool.runtime.path.display()),
        "--".to_owned(),
        youtube::watch_url(id),
    ]);
    args
}

/// The second call: the chosen track only, from the description the first
/// call wrote, so the video is not described again.
fn caption_args(dir: &Path, info: &Path, choice: &Choice) -> Vec<String> {
    let mut args = common(dir, CAPTIONS);
    let which = match choice.kind {
        Captions::Manual => "--write-subs",
        Captions::Automatic => "--write-auto-subs",
    };
    args.extend([
        "--load-info-json".to_owned(),
        info.to_string_lossy().into_owned(),
        which.to_owned(),
        "--sub-langs".to_owned(),
        choice.key.clone(),
        "--sub-format".to_owned(),
        youtube_page::FORMAT.to_owned(),
    ]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Table;

    fn tool() -> YtDlp {
        YtDlp {
            path: PathBuf::from("/opt/bin/yt-dlp"),
            version: Some("2026.08.19".to_owned()),
            runtime: Runtime {
                name: "node",
                path: PathBuf::from("/opt/bin/node"),
            },
        }
    }

    #[test]
    fn reasons_map_to_the_vocabulary() {
        for (complaint, status, reason) in [
            (
                "ERROR: [youtube] aBc-12_xYz9: Sign in to confirm you\u{2019}re not a bot. Use --cookies",
                Status::Blocked,
                "YouTube asked to confirm this is not a bot",
            ),
            (
                "ERROR: Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests",
                Status::Blocked,
                "rate limited by YouTube (HTTP 429)",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This content isn't available, try again later.",
                Status::Blocked,
                "rate limited by YouTube",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Private video. Sign in if you've been granted access",
                Status::NotFound,
                "private video",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Video unavailable. This video is private",
                Status::NotFound,
                "private video",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Video unavailable. This video has been removed by the uploader",
                Status::NotFound,
                "video removed",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Video unavailable",
                Status::NotFound,
                "video unavailable",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This video is no longer available because the YouTube account associated with this video has been terminated.",
                Status::NotFound,
                "account terminated",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Sign in to confirm your age. This video may be inappropriate for some users.",
                Status::BehindLogin,
                "age restricted",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Join this channel to get access to members-only content like this video",
                Status::BehindLogin,
                "members only",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This video is only available to Music Premium members",
                Status::BehindLogin,
                "YouTube Premium only",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Sign in to view this video",
                Status::BehindLogin,
                "sign in required",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: The uploader has not made this video available in your country",
                Status::Blocked,
                "not available in this country",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This live event will begin in 3 hours.",
                Status::Error,
                "not started yet",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Unable to download API page: HTTP Error 503: Service Unavailable",
                Status::Error,
                "[youtube] aBc-12_xYz9: Unable to download API page: HTTP Error 503: Service Unavailable",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: Unable to download webpage: timed out",
                Status::Error,
                "[youtube] aBc-12_xYz9: Unable to download webpage: timed out",
            ),
        ] {
            assert_eq!(
                refusal(Some(1), complaint),
                (status, reason.to_owned()),
                "{complaint}"
            );
        }
        assert_eq!(
            refusal(Some(2), ""),
            (Status::Error, "yt-dlp exited with code 2".to_owned())
        );
        assert_eq!(
            refusal(None, ""),
            (Status::Error, "yt-dlp was stopped by a signal".to_owned())
        );
    }

    #[test]
    fn unavailable_wording_is_final_without_overriding_access_refusals() {
        for (complaint, status, reason) in [
            (
                "ERROR: [youtube] aBc-12_xYz9: This video is not available.",
                Status::NotFound,
                "video unavailable",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This video is not available in your country.",
                Status::Blocked,
                "not available in this country",
            ),
            (
                "ERROR: [youtube] aBc-12_xYz9: This video is not available. Sign in to view this video.",
                Status::BehindLogin,
                "sign in required",
            ),
        ] {
            assert_eq!(refusal(Some(1), complaint), (status, reason.to_owned()));
        }
    }

    #[test]
    fn the_first_call_describes_the_rebuilt_address_without_cookies() {
        let args = describe_args(&tool(), Path::new("/scratch"), "-Bc-12_xYz9");
        assert_eq!(
            args.join(" "),
            "--ignore-config --no-playlist --skip-download --sleep-requests 1 \
             -o /scratch/video.%(ext)s --write-info-json --js-runtimes node:/opt/bin/node \
             -- https://www.youtube.com/watch?v=-Bc-12_xYz9"
        );
        assert!(args.iter().all(|arg| !arg.contains("cookies")));
    }

    #[test]
    fn the_second_call_downloads_only_the_chosen_track() {
        let manual = Choice {
            key: "en".to_owned(),
            kind: Captions::Manual,
        };
        let args = caption_args(
            Path::new("/scratch"),
            Path::new("/scratch/video.info.json"),
            &manual,
        );
        assert_eq!(
            args.join(" "),
            "--ignore-config --no-playlist --skip-download --sleep-requests 1 \
             -o /scratch/captions.%(ext)s --load-info-json /scratch/video.info.json \
             --write-subs --sub-langs en --sub-format vtt"
        );
        let automatic = Choice {
            key: "de-orig".to_owned(),
            kind: Captions::Automatic,
        };
        let args = caption_args(
            Path::new("/scratch"),
            Path::new("/scratch/video.info.json"),
            &automatic,
        );
        assert!(
            args.join(" ")
                .contains("--write-auto-subs --sub-langs de-orig ")
        );
        assert!(!args.contains(&"--write-subs".to_owned()));
        assert!(
            args.iter()
                .all(|arg| !arg.contains("cookies") && arg != "all")
        );
    }

    #[test]
    fn ready_needs_yt_dlp_and_a_runtime_and_a_dry_run_runs_nothing() {
        let path = |name: &str| PathBuf::from(format!("/opt/bin/{name}"));
        let mut table = Table::default();
        assert_eq!(Readiness::check(&table), Readiness::Missing);
        table.found.insert("yt-dlp", path("yt-dlp"));
        table
            .versions
            .insert(path("yt-dlp"), "2026.08.19".to_owned());
        assert_eq!(
            Readiness::check(&table),
            Readiness::NoRuntime(path("yt-dlp"), Some("2026.08.19".to_owned()))
        );
        assert_eq!(Readiness::check(&table).tool(), None);
        table.found.insert("node", path("node"));
        assert_eq!(Readiness::check(&table).tool(), Some(&tool()));
        table.found.insert("deno", path("deno"));
        let ready = Readiness::check(&table);
        assert_eq!(ready.tool().unwrap().runtime.name, "deno", "deno first");
        let assumed = Readiness::assumed(&table);
        assert_eq!(assumed.tool().unwrap().version, None, "nothing run");
        assert!(table.runs.borrow().is_empty());
    }

    #[test]
    fn waiting_pages_are_counted_in_a_note() {
        assert_eq!(
            waiting_note(1),
            "1 YouTube page waiting for yt-dlp (see knowmoretabs doctor)"
        );
        assert_eq!(
            waiting_note(113),
            "113 YouTube pages waiting for yt-dlp (see knowmoretabs doctor)"
        );
    }

    #[test]
    fn a_kept_page_records_yt_dlp_and_its_version() {
        let tool = tool();
        let attempt = Attempt {
            tool: &tool,
            raw: "https://www.youtube.com/watch?v=aBc-12_xYz9&t=5",
        };
        let info = Info::default();
        let capture = attempt.kept(
            attempt.line(Status::Ok),
            youtube_page::page(&info, &[], Some(Captions::Automatic)),
        );
        assert_eq!(capture.line.tier, Some(Tier::Youtube));
        assert_eq!(capture.line.extractor.as_deref(), Some("yt-dlp"));
        assert_eq!(
            capture.line.extractor_version.as_deref(),
            Some("2026.08.19")
        );
        let page = capture.page.unwrap();
        assert_eq!(page.extractor, "yt-dlp 2026.08.19");
        assert_eq!(page.captions, Some(Captions::Automatic));
    }

    #[test]
    fn info_json_above_the_body_cap_is_read_and_parsed() {
        let tool = tool();
        let attempt = Attempt {
            tool: &tool,
            raw: "synthetic",
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("video.info.json");
        let description = "x".repeat(BODY_CAP + 1);
        let json = serde_json::to_vec(&serde_json::json!({"description": description})).unwrap();
        fs::write(&path, &json).unwrap();
        let bytes = attempt
            .file(&path, "video information", INFO_JSON_CAP)
            .unwrap();
        assert_eq!(bytes.len(), json.len());
        let info: Info = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(info.description.unwrap().len(), BODY_CAP + 1);

        let ended = attempt.file(&path, "captions", BODY_CAP).unwrap_err();
        assert_eq!(ended.line.status, Status::Error);
        assert_eq!(
            ended.line.reason.as_deref(),
            Some("yt-dlp's captions over 10 MiB")
        );

        fs::File::create(&path)
            .unwrap()
            .set_len(INFO_JSON_CAP as u64 + 1)
            .unwrap();
        let ended = attempt
            .file(&path, "video information", INFO_JSON_CAP)
            .unwrap_err();
        assert_eq!(ended.line.status, Status::Error);
        assert_eq!(
            ended.line.reason.as_deref(),
            Some("yt-dlp's video information over 64 MiB")
        );
    }

    #[test]
    fn a_file_yt_dlp_did_not_write_ends_the_attempt_as_an_error() {
        let tool = tool();
        let attempt = Attempt {
            tool: &tool,
            raw: "https://youtu.be/aBc-12_xYz9",
        };
        let dir = tempfile::tempdir().unwrap();
        let ended = attempt
            .file(&dir.path().join("absent"), "captions", BODY_CAP)
            .unwrap_err();
        assert_eq!(
            (ended.line.status, ended.line.reason.as_deref()),
            (Status::Error, Some("yt-dlp wrote no captions"))
        );
    }
}
