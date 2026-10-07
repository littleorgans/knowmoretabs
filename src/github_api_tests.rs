//! Regression checks for `github_api`.
//!
//! slice: content
//! why: Synthetic API answers and readiness probes exercise the route without making network requests or running external tools.

use serde_json::json;

use super::*;
use crate::github_page::repo_page;
use crate::tools::Table;

const RAW: &str = "https://github.com/some-owner/tide";

fn gh() -> Gh {
    Gh {
        path: PathBuf::from("/usr/bin/gh"),
        version: Some("gh version 2.102.0 (2026-09-30)".to_owned()),
    }
}

fn repo() -> Repo {
    Repo {
        owner: "some-owner".to_owned(),
        name: "tide".to_owned(),
    }
}

fn answer(status: u16, headers: &[(&str, &str)], body: &serde_json::Value) -> Answer {
    Answer {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn ended(ended: Ended) -> (Status, String, bool) {
    match *ended {
        Ok(capture) => (
            capture.line.status,
            capture.line.reason.unwrap_or_default(),
            false,
        ),
        Err(passing) => {
            let line = passing.line();
            (line.status, line.reason.clone().unwrap_or_default(), true)
        }
    }
}

#[test]
fn api_calls_use_the_host_readiness_checked() {
    let api_host = API_ARGS
        .windows(2)
        .find_map(|pair| (pair[0] == "--hostname").then_some(pair[1]));
    assert_eq!(
        api_host,
        Some(AUTH_STATUS[3]),
        "API calls must not inherit a different host"
    );
    assert_eq!(api_host, Some("github.com"));
}

#[test]
fn an_included_answer_splits_into_status_headers_and_body() {
    let printed = b"HTTP/2.0 404 Not Found\nContent-Type: application/json\r\n\
        X-Ratelimit-Remaining: 4947\r\n\r\n{\"message\":\"Not Found\"}";
    let answer = parse_include(printed).unwrap();
    assert_eq!(answer.status, 404);
    assert_eq!(answer.header("content-type"), Some("application/json"));
    assert_eq!(answer.header("x-ratelimit-remaining"), Some("4947"));
    assert_eq!(answer.body, b"{\"message\":\"Not Found\"}");
    assert_eq!(answer.message().as_deref(), Some("Not Found"));
    let empty = parse_include(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap();
    assert_eq!((empty.status, empty.body.len()), (204, 0));
    assert_eq!(parse_include(b""), None);
    assert_eq!(
        parse_include(b"gh: error connecting to api.github.com\n"),
        None
    );
    assert_eq!(parse_include(b"HTTP/2.0 200 OK\nNo-Blank-Line: x\n"), None);
}

#[test]
fn api_statuses_map_to_the_vocabulary() {
    let attempt = Attempt {
        gh: &gh(),
        raw: RAW,
        images: true,
    };
    let status = |answer: Answer| ended(attempt.refusal(&answer).unwrap());
    let none = json!({});
    assert!(attempt.refusal(&answer(200, &[], &none)).is_none());
    assert_eq!(
        status(answer(404, &[], &json!({"message": "Not Found"}))),
        (
            Status::NotFound,
            "not found or private (HTTP 404)".to_owned(),
            false
        )
    );
    assert_eq!(
        status(answer(401, &[], &none)),
        (
            Status::Error,
            "gh is not signed in (HTTP 401)".to_owned(),
            false
        )
    );
    assert_eq!(
        status(answer(
            403,
            &[],
            &json!({"message": "Resource protected by organization SAML enforcement."})
        )),
        (
            Status::Blocked,
            "HTTP 403: Resource protected by organization SAML enforcement.".to_owned(),
            false
        )
    );
    assert_eq!(
        status(answer(403, &[("x-ratelimit-remaining", "0")], &none)),
        (Status::Error, "rate limited (HTTP 403)".to_owned(), true)
    );
    assert_eq!(
        status(answer(429, &[("retry-after", "30")], &none)),
        (Status::Error, "rate limited (HTTP 429)".to_owned(), true)
    );
    assert_eq!(
        status(answer(502, &[], &none)),
        (Status::Error, "HTTP 502".to_owned(), true)
    );
    assert_eq!(
        status(answer(500, &[], &none)),
        (Status::Error, "HTTP 500".to_owned(), false)
    );
    assert_eq!(
        status(answer(451, &[], &none)).0,
        Status::NotFound,
        "a takedown is final"
    );
}

#[test]
fn a_spent_rate_limit_waits_for_its_reset() {
    let now: Timestamp = "2026-10-07T09:00:00Z".parse().unwrap();
    let reset = (now.as_second() + 40).to_string();
    let spent = answer(
        403,
        &[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", reset.as_str()),
        ],
        &json!({}),
    );
    assert_eq!(spent.retry_after(now), Some(Duration::from_secs(40)));
    let asked = answer(403, &[("retry-after", "7")], &json!({}));
    assert_eq!(asked.retry_after(now), Some(Duration::from_secs(7)));
    let fine = answer(403, &[("x-ratelimit-remaining", "12")], &json!({}));
    assert_eq!(fine.retry_after(now), None);
    assert!(!fine.is_rate_limited());
}

#[test]
fn graphql_errors_and_private_repositories_keep_no_text() {
    for (kind, expected) in [
        ("NOT_FOUND", (Status::NotFound, NOT_FOUND, false)),
        ("FORBIDDEN", (Status::Blocked, "forbidden", false)),
        ("RATE_LIMITED", (Status::Error, "rate limited", true)),
        ("SOMETHING_NEW", (Status::Error, "GitHub API error", false)),
    ] {
        let graph: Graph = serde_json::from_value(json!({"data": {"repository": null},
            "errors": [{"type": kind, "message": "Could not resolve"}]}))
        .unwrap();
        assert_eq!(graph_failure(&graph.errors), Some(expected), "{kind}");
    }
    let private: Graph = serde_json::from_value(json!({"data": {"repository": {"isPrivate": true,
        "issueOrPullRequest": {"__typename": "Issue", "title": "Secret", "body": "Secret"}}}}))
    .unwrap();
    assert!(private.data.unwrap().repository.unwrap().is_private);
    let info: RepoInfo = serde_json::from_value(json!({"private": true})).unwrap();
    assert!(info.private);
}

#[test]
fn a_kept_page_records_gh_and_its_version() {
    let attempt = Attempt {
        gh: &gh(),
        raw: RAW,
        images: true,
    };
    let line = attempt.line(Status::Ok, None, Some(200));
    let info = RepoInfo::default();
    let capture = attempt.kept(
        line.clone(),
        repo_page(&repo(), &info, Some("Text.")),
        Found::Unread,
    );
    assert_eq!(capture.line.status, Status::Ok);
    assert_eq!(capture.line.tier, Some(Tier::Github));
    assert_eq!(capture.line.extractor.as_deref(), Some("gh"));
    assert_eq!(capture.line.extractor_version.as_deref(), Some("2.102.0"));
    assert_eq!(capture.page.unwrap().extractor, "gh 2.102.0");
    let thin = attempt.kept(line, repo_page(&repo(), &info, None), Found::Unread);
    assert_eq!(
        (thin.line.status, thin.line.reason.as_deref()),
        (Status::Thin, Some("no README"))
    );
}

#[test]
fn gh_is_ready_only_when_found_and_signed_in() {
    let path = PathBuf::from("/usr/bin/gh");
    let mut table = Table::default();
    let asked = |table: &Table| {
        let runs = table.runs.borrow();
        assert!(runs.iter().all(|(args, env)| {
            args == &AUTH_STATUS.join(" ") && env.contains("GH_PROMPT_DISABLED=1")
        }));
        runs.len()
    };
    assert_eq!(Readiness::check(&table), Readiness::Missing);
    assert_eq!(Readiness::check(&table).fallback(), Some("gh not found"));
    table.found.insert("gh", path.clone());
    table
        .versions
        .insert(path.clone(), "gh version 2.102.0 (2026-09-30)".to_owned());
    table.codes.insert(path.clone(), 1);
    let unsigned = Readiness::check(&table);
    assert_eq!(unsigned, Readiness::NotSignedIn(gh(), Some(1)));
    assert_eq!(unsigned.fallback(), Some("gh not signed in"));
    assert_eq!(unsigned.gh(), None);
    let before = asked(&table);
    assert_eq!(Readiness::assumed(&table), Readiness::Ready(gh()));
    assert_eq!(asked(&table), before, "a dry run does not ask gh");
    table.codes.insert(path, 0);
    assert_eq!(Readiness::check(&table).gh(), Some(&gh()));
    assert_eq!(Readiness::check(&table).fallback(), None);
}
