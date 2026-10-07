//! The GitHub route: a repository, issue, pull request or discussion read
//! through `gh api` and kept as markdown.
//!
//! slice: content
//! why: A repository page is mostly navigation to plain HTTP, and an issue
//!      or a discussion is rendered for a browser, while the GitHub API
//!      answers with the text itself: a repository's description, topics
//!      and README as the author wrote it, a thread's opening post and its
//!      first comments. `gh` is GitHub's own client and holds its own sign
//!      in, so this program never sees a token: it only runs `gh api` with
//!      an argument vector, under a timeout, at most four at a time. Without
//!      a signed in `gh`, GitHub pages are read from the web as any page is.
//!      Only public repositories are kept, as every other route keeps only
//!      what anyone may read; a private one waits for a signed in reading.

use std::path::PathBuf;
use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;

use crate::content_fetch::{self, BODY_CAP, Capture, Passing};
use crate::content_store::{Line, Page, Status, Tier};
use crate::github::{Repo, Target};
use crate::github_page::{self, Graph, GraphError, GraphRepo, RepoInfo, ThreadKind};
use crate::image_pick::Found;
use crate::tools::{self, Limits, Probe};

/// The host every `gh api` call goes to.
pub const API_HOST: &str = "api.github.com";
/// `gh` calls at once: well inside GitHub's limits for one account.
pub const CONCURRENT: usize = 4;
/// Recorded as the extractor, with `gh`'s own version.
pub const EXTRACTOR: &str = "gh";
const TIMEOUT: Duration = Duration::from_secs(20);
/// Never ask, never page, never check for updates, never colour.
pub const ENV: [(&str, &str); 3] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("NO_COLOR", "1"),
];
const HOST: &str = "github.com";
const AUTH_STATUS: [&str; 4] = ["auth", "status", "--hostname", HOST];
// Match readiness even when gh is configured for an enterprise host.
const API_ARGS: &[&str] = &["api", "--include", "--hostname", HOST];

/// The `gh` this run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gh {
    pub path: PathBuf,
    /// The first line of `gh --version`.
    pub version: Option<String>,
}

impl Gh {
    /// `2.102.0` from `gh version 2.102.0 (2026-09-30)`.
    fn version_number(&self) -> Option<&str> {
        let line = self.version.as_deref()?;
        line.split_whitespace()
            .skip_while(|word| *word != "version")
            .nth(1)
    }

    fn extractor(&self) -> String {
        match self.version_number() {
            Some(version) => format!("{EXTRACTOR} {version}"),
            None => EXTRACTOR.to_owned(),
        }
    }
}

/// Whether GitHub pages can be read through the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready(Gh),
    /// `gh` is not on `PATH`.
    Missing,
    /// `gh auth status` did not exit 0; its exit code, if it exited.
    NotSignedIn(Gh, Option<i32>),
}

impl Readiness {
    /// `gh` found, and signed in to github.com by its own account. `gh`
    /// checks that with GitHub itself; only its exit code is read.
    pub fn check(probe: &impl Probe) -> Self {
        let Some(gh) = found(probe) else {
            return Self::Missing;
        };
        match probe.exit_code(&gh.path, &AUTH_STATUS, &ENV) {
            Some(0) => Self::Ready(gh),
            code => Self::NotSignedIn(gh, code),
        }
    }

    /// `gh` found, its sign in not asked: what a dry run, which sends
    /// nothing, plans with.
    pub fn assumed(probe: &impl Probe) -> Self {
        found(probe).map_or(Self::Missing, Self::Ready)
    }

    pub fn gh(&self) -> Option<&Gh> {
        match self {
            Self::Ready(gh) => Some(gh),
            _ => None,
        }
    }

    /// Why GitHub pages are read from the web instead, when they are.
    pub fn fallback(&self) -> Option<&'static str> {
        match self {
            Self::Ready(_) => None,
            Self::Missing => Some("gh not found"),
            Self::NotSignedIn(..) => Some("gh not signed in"),
        }
    }
}

fn found(probe: &impl Probe) -> Option<Gh> {
    let path = probe.find("gh")?;
    let version = probe.version(&path);
    Some(Gh { path, version })
}

/// Reads `target` for library page `raw`, retrying passing failures as
/// every route does, and with `images` asks a repository for its social
/// preview. Never fails: a failure is a capture too.
pub fn capture(gh: &Gh, raw: &str, target: &Target, images: bool) -> Capture {
    let attempt = Attempt { gh, raw, images };
    content_fetch::retrying(|| match target {
        Target::Repo(repo) => attempt.repo(repo),
        Target::Issue(repo, n) | Target::Pull(repo, n) => {
            attempt.thread(repo, *n, ThreadKind::IssueOrPull)
        }
        Target::Discussion(repo, n) => attempt.thread(repo, *n, ThreadKind::Discussion),
    })
}

/// How one attempt ends early: a capture, or a failure worth a retry.
type Ended = Box<Result<Capture, Passing>>;

/// One HTTP answer as `gh api --include` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    /// Names lowercased.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// How long GitHub asked us to wait: its `Retry-After`, or the time to
    /// its rate limit's reset once that is spent.
    fn retry_after(&self, now: Timestamp) -> Option<Duration> {
        if let Some(value) = self.header("retry-after") {
            return content_fetch::retry_after(value, now);
        }
        if self.header("x-ratelimit-remaining") != Some("0") {
            return None;
        }
        let reset: i64 = self.header("x-ratelimit-reset")?.parse().ok()?;
        let seconds = reset.saturating_sub(now.as_second());
        Some(Duration::from_secs(u64::try_from(seconds).unwrap_or(0)))
    }

    /// GitHub says 403 both for "not for you" and for "slow down"; the
    /// rate limit headers tell them apart.
    fn is_rate_limited(&self) -> bool {
        self.status == 429
            || (self.status == 403
                && (self.header("retry-after").is_some()
                    || self.header("x-ratelimit-remaining") == Some("0")))
    }

    /// The API's own `message`, as one line.
    fn message(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct Message {
            message: String,
        }
        let message = serde_json::from_slice::<Message>(&self.body).ok()?.message;
        Some(tools::complaint(&message)).filter(|m| !m.is_empty())
    }
}

/// `HTTP/2.0 200 OK`, header lines, a blank line, then the body; `None`
/// when `gh` printed no answer.
fn parse_include(stdout: &[u8]) -> Option<Answer> {
    let mut rest = stdout;
    let mut next_line = || {
        let end = rest.iter().position(|&b| b == b'\n')?;
        let line = String::from_utf8_lossy(&rest[..end]).trim_end().to_owned();
        rest = &rest[end + 1..];
        Some(line)
    };
    let status_line = next_line()?;
    let mut words = status_line.split_whitespace();
    if !words.next()?.starts_with("HTTP/") {
        return None;
    }
    let status = words.next()?.parse().ok()?;
    let mut headers = Vec::new();
    loop {
        let line = next_line()?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    Some(Answer {
        status,
        headers,
        body: rest.to_vec(),
    })
}

/// One attempt at one page.
struct Attempt<'a> {
    gh: &'a Gh,
    raw: &'a str,
    /// Whether the run keeps images, which costs a repository one more
    /// call.
    images: bool,
}

impl Attempt<'_> {
    fn line(&self, status: Status, reason: Option<String>, http: Option<u16>) -> Line {
        let mut line = content_fetch::public_line(self.raw, Tier::Github, status);
        line.reason = reason;
        line.http_status = http;
        line
    }

    fn ended(&self, status: Status, reason: String, http: Option<u16>) -> Ended {
        Box::new(Ok(Capture::ended(self.line(status, Some(reason), http))))
    }

    /// One `gh api` call and the answer it printed, whatever its status.
    /// When `gh` printed none (it could not start, timed out, or failed
    /// before asking), the attempt ends as an `error` for the next run.
    fn ask(&self, args: &[&str]) -> Result<Answer, Ended> {
        let mut command = API_ARGS.to_vec();
        command.extend_from_slice(args);
        let limits = Limits {
            timeout: TIMEOUT,
            stdout_cap: BODY_CAP,
        };
        let output = tools::run(&self.gh.path, &command, &ENV, limits)
            .map_err(|failure| self.ended(Status::Error, failure.reason("gh"), None))?;
        if output.overflowed {
            let reason = format!("gh answer over {} MiB", BODY_CAP >> 20);
            return Err(self.ended(Status::Error, reason, None));
        }
        parse_include(&output.stdout).ok_or_else(|| {
            let reason = match (output.code, output.complaint.is_empty()) {
                (_, false) => output.complaint,
                (Some(code), true) => format!("gh exited with code {code} and no answer"),
                (None, true) => "gh was stopped by a signal".to_owned(),
            };
            self.ended(Status::Error, reason, None)
        })
    }

    /// An answer that is not a success, as how the attempt ends; `None`
    /// for a success.
    fn refusal(&self, answer: &Answer) -> Option<Ended> {
        let status = answer.status;
        let (state, reason) = match status {
            200..=299 => return None,
            _ if answer.is_rate_limited() => {
                let line = self.line(
                    Status::Error,
                    Some(format!("rate limited (HTTP {status})")),
                    Some(status),
                );
                let wait = answer.retry_after(Timestamp::now());
                return Some(Box::new(Err(Passing::new(line, wait))));
            }
            // The API's 401 is about `gh`'s token, not the page.
            401 => (Status::Error, "gh is not signed in (HTTP 401)".to_owned()),
            // GitHub answers 404 alike for a page that does not exist and
            // one this account may not see, and a page it says does not
            // exist is final.
            404 => (
                Status::NotFound,
                "not found or private (HTTP 404)".to_owned(),
            ),
            451 => (
                Status::NotFound,
                "unavailable for legal reasons (HTTP 451)".to_owned(),
            ),
            _ => {
                let (state, passing) =
                    content_fetch::status_outcome(status).unwrap_or((Status::Error, false));
                let reason = match answer.message() {
                    Some(message) => format!("HTTP {status}: {message}"),
                    None => format!("HTTP {status}"),
                };
                if passing {
                    let line = self.line(state, Some(reason), Some(status));
                    let wait = answer.retry_after(Timestamp::now());
                    return Some(Box::new(Err(Passing::new(line, wait))));
                }
                (state, reason)
            }
        };
        Some(self.ended(state, reason, Some(status)))
    }

    /// A successful answer's JSON, or the attempt ends as an `error`.
    fn json<T: for<'de> Deserialize<'de>>(&self, answer: &Answer, what: &str) -> Result<T, Ended> {
        serde_json::from_slice(&answer.body).map_err(|_| {
            self.ended(
                Status::Error,
                format!("not a {what} answer"),
                Some(answer.status),
            )
        })
    }

    fn repo(&self, repo: &Repo) -> Result<Capture, Passing> {
        self.repo_ended(repo).or_else(|ended| *ended)
    }

    fn repo_ended(&self, repo: &Repo) -> Result<Capture, Ended> {
        let path = format!("repos/{}/{}", repo.owner, repo.name);
        let answer = self.ask(&[&path])?;
        if let Some(ended) = self.refusal(&answer) {
            return Err(ended);
        }
        let info: RepoInfo = self.json(&answer, "repository")?;
        if info.private {
            return Err(self.ended(
                Status::BehindLogin,
                "private repository".to_owned(),
                Some(answer.status),
            ));
        }
        let readme_path = format!("{path}/readme");
        let readme = self.ask(&["-H", "Accept: application/vnd.github.raw", &readme_path])?;
        let readme = match readme.status {
            404 => None,
            _ => match self.refusal(&readme) {
                Some(ended) => return Err(ended),
                None => Some(String::from_utf8_lossy(&readme.body).into_owned()),
            },
        };
        let preview = self.images.then(|| self.preview(repo)).flatten();
        let images = github_page::images(repo, preview.as_ref(), readme.as_deref());
        let mut line = self.line(Status::Ok, None, Some(answer.status));
        line.final_url.clone_from(&info.html_url);
        Ok(self.kept(
            line,
            github_page::repo_page(repo, &info, readme.as_deref()),
            images,
        ))
    }

    /// A repository's social preview, through one GraphQL call; `None`
    /// when the call fails, which costs the page only that image.
    fn preview(&self, repo: &Repo) -> Option<GraphRepo> {
        let query = format!("query={}", github_page::preview_query());
        let owner = format!("owner={}", repo.owner);
        let name = format!("name={}", repo.name);
        let answer = self
            .ask(&["graphql", "-f", &query, "-f", &owner, "-f", &name])
            .ok()?;
        if !(200..300).contains(&answer.status) {
            return None;
        }
        serde_json::from_slice::<Graph>(&answer.body)
            .ok()?
            .data?
            .repository
    }

    fn thread(&self, repo: &Repo, number: u32, kind: ThreadKind) -> Result<Capture, Passing> {
        self.thread_ended(repo, number, kind)
            .or_else(|ended| *ended)
    }

    fn thread_ended(&self, repo: &Repo, number: u32, kind: ThreadKind) -> Result<Capture, Ended> {
        let query = format!("query={}", github_page::thread_query(kind));
        let owner = format!("owner={}", repo.owner);
        let name = format!("name={}", repo.name);
        let number_field = format!("number={number}");
        // `-f` keeps a name a string even when it is all digits; `-F`
        // makes the number an Int.
        let answer = self.ask(&[
            "graphql",
            "-f",
            &query,
            "-f",
            &owner,
            "-f",
            &name,
            "-F",
            &number_field,
        ])?;
        if let Some(ended) = self.refusal(&answer) {
            return Err(ended);
        }
        let graph: Graph = self.json(&answer, "GraphQL")?;
        let http = Some(answer.status);
        if let Some((state, reason, passing)) = graph_failure(&graph.errors) {
            if passing {
                let line = self.line(state, Some(reason.to_owned()), http);
                return Err(Box::new(Err(Passing::new(line, None))));
            }
            return Err(self.ended(state, reason.to_owned(), http));
        }
        let Some(repository) = graph.data.and_then(|data| data.repository) else {
            return Err(self.ended(
                Status::Error,
                "no repository in the answer".to_owned(),
                http,
            ));
        };
        if repository.is_private {
            return Err(self.ended(Status::BehindLogin, "private repository".to_owned(), http));
        }
        let images = github_page::images(repo, Some(&repository), None);
        let Some(thread) = repository.issue.or(repository.discussion) else {
            return Err(self.ended(Status::NotFound, NOT_FOUND.to_owned(), http));
        };
        let mut line = self.line(Status::Ok, None, http);
        line.final_url = Some(thread.url.clone()).filter(|url| !url.is_empty());
        Ok(self.kept(
            line,
            github_page::thread_page(repo, number, &thread),
            images,
        ))
    }

    /// `line` with what every kept page records, the page and its images;
    /// `thin` with its reason when the page has no text beyond its title.
    fn kept(&self, mut line: Line, (page, thin): (Page, Option<&str>), images: Found) -> Capture {
        if let Some(reason) = thin {
            line.status = Status::Thin;
            line.reason = Some(reason.to_owned());
        }
        line.extractor = Some(EXTRACTOR.to_owned());
        line.extractor_version = self.gh.version_number().map(str::to_owned);
        Capture {
            line,
            page: Some(Page {
                extractor: self.gh.extractor(),
                ..page
            }),
            images,
        }
    }
}

const NOT_FOUND: &str = "not found or private";

/// What the GraphQL `errors` say, by the first one's type, and whether it
/// may pass: GitHub answers `NOT_FOUND` alike for what does not exist and
/// what this account may not see.
fn graph_failure(errors: &[GraphError]) -> Option<(Status, &'static str, bool)> {
    let first = errors.first()?;
    Some(match first.kind.as_str() {
        "NOT_FOUND" => (Status::NotFound, NOT_FOUND, false),
        "FORBIDDEN" => (Status::Blocked, "forbidden", false),
        "RATE_LIMITED" => (Status::Error, "rate limited", true),
        _ => (Status::Error, "GitHub API error", false),
    })
}

#[cfg(test)]
mod tests {
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
        let private: Graph =
            serde_json::from_value(json!({"data": {"repository": {"isPrivate": true,
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
}
