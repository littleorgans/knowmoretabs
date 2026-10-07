//! Regression checks for `ytdlp`.
//!
//! slice: content
//! why: Synthetic tool results and readiness probes keep command construction and response handling verifiable without requesting real videos.

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
    let dir = Path::new("/scratch with spaces");
    let output = dir.join("video.%(ext)s");
    let args = describe_args(&tool(), dir, "-Bc-12_xYz9");
    assert_eq!(
        args,
        [
            "--ignore-config",
            "--no-playlist",
            "--skip-download",
            "--sleep-requests",
            "1",
            "-o",
            output.to_str().unwrap(),
            "--write-info-json",
            "--js-runtimes",
            "node:/opt/bin/node",
            "--",
            "https://www.youtube.com/watch?v=-Bc-12_xYz9",
        ]
        .map(str::to_owned)
    );
    assert!(args.iter().all(|arg| !arg.contains("cookies")));
}

#[test]
fn the_second_call_downloads_only_the_chosen_track() {
    let dir = Path::new("/scratch with spaces");
    let output = dir.join("captions.%(ext)s");
    let info = dir.join("video.info.json");
    for (key, kind, flag) in [
        ("en-CA-captiontrack", Captions::Manual, "--write-subs"),
        ("de-orig", Captions::Automatic, "--write-auto-subs"),
    ] {
        let choice = Choice {
            key: key.to_owned(),
            kind,
        };
        let args = caption_args(dir, &info, &choice);
        assert_eq!(
            args,
            [
                "--ignore-config",
                "--no-playlist",
                "--skip-download",
                "--sleep-requests",
                "1",
                "-o",
                output.to_str().unwrap(),
                "--load-info-json",
                info.to_str().unwrap(),
                flag,
                "--sub-langs",
                key,
                "--sub-format",
                "vtt",
            ]
            .map(str::to_owned)
        );
        assert!(
            args.iter()
                .all(|arg| !arg.contains("cookies") && arg != "all")
        );
    }
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
        Found::Unread,
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
