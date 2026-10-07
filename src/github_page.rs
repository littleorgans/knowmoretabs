//! What the GitHub API answers about a repository or a thread, and the
//! markdown kept of it.
//!
//! slice: content
//! why: The route that asks `gh` and the page it keeps change for
//!      different reasons: one follows GitHub's statuses and limits, the
//!      other what a reader of the page needs. Here the answers are read
//!      into types that name only what is kept, and turned into one
//!      markdown layout: a repository's description, topics and README as
//!      its author wrote it; a thread's title, who opened it and when, its
//!      state, its opening post and its first comments with the total. A
//!      page with no text beyond its title is thin, and says why.

use std::fmt::Write as _;

use serde::Deserialize;
use url::Url;

use crate::content_store::{Completeness, Page};
use crate::extract;
use crate::github::Repo;
use crate::image_page;
use crate::image_pick::{Candidate, Found, Source};

/// The comments kept of a thread, from its start: the question and its
/// first answers, which is where a thread says what it is about, in one
/// bounded request. The file says how many there were in all.
pub const COMMENTS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadKind {
    IssueOrPull,
    Discussion,
}

/// The GraphQL query for one thread and its first [`COMMENTS`] comments.
/// Issues and pull requests share their numbers, so one query asks for
/// whichever the number is.
pub fn thread_query(kind: ThreadKind) -> String {
    let fields = format!(
        "title body url createdAt author{{login}} \
         comments(first:{COMMENTS}){{totalCount nodes{{id author{{login}} createdAt body}}}}"
    );
    let thread = match kind {
        ThreadKind::IssueOrPull => format!(
            "issueOrPullRequest(number:$number){{__typename \
             ...on Issue{{state {fields}}} ...on PullRequest{{state {fields}}}}}"
        ),
        ThreadKind::Discussion => format!(
            "discussion(number:$number){{__typename category{{name}} \
             answer{{id author{{login}} createdAt body}} {fields}}}"
        ),
    };
    format!(
        "query($owner:String!,$name:String!,$number:Int!)\
         {{repository(owner:$owner,name:$name){{isPrivate {PREVIEW} {thread}}}}}"
    )
}

/// A repository's social preview: the image GitHub shows when it is
/// shared, and whether its owner chose it or GitHub drew it.
const PREVIEW: &str = "openGraphImageUrl usesCustomOpenGraphImage";

/// The GraphQL query for a repository's social preview alone.
pub fn preview_query() -> String {
    format!(
        "query($owner:String!,$name:String!){{repository(owner:$owner,name:$name){{{PREVIEW}}}}}"
    )
}

/// A repository's images, best first: the social preview its owner chose,
/// the first picture its README shows, then the card GitHub generated.
/// `preview` is what the GraphQL API said, when it was asked.
pub fn images(repo: &Repo, preview: Option<&GraphRepo>, readme: Option<&str>) -> Found {
    let (custom, generated) = match preview {
        Some(graph) => match graph.preview.clone() {
            Some(url) if graph.custom_preview => (Some(url), None),
            url => (None, url),
        },
        None => (None, None),
    };
    let files = Url::parse(&format!(
        "https://raw.githubusercontent.com/{}/{}/HEAD/",
        repo.owner, repo.name
    ))
    .ok();
    let readme = readme
        .zip(files.as_ref())
        .and_then(|(markdown, files)| image_page::readme_image(markdown, files));
    Found::Candidates(
        [
            (custom, Source::GithubSocial),
            (readme, Source::GithubReadme),
            (generated, Source::GithubCard),
        ]
        .into_iter()
        .filter_map(|(url, source)| Some(Candidate { url: url?, source }))
        .collect(),
    )
}

/// A repository as the REST API gives it; only what is kept is read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RepoInfo {
    pub full_name: String,
    pub html_url: Option<String>,
    pub description: Option<String>,
    pub topics: Vec<String>,
    pub private: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Graph {
    pub data: Option<GraphData>,
    pub errors: Vec<GraphError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GraphError {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GraphData {
    pub repository: Option<GraphRepo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GraphRepo {
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
    #[serde(rename = "openGraphImageUrl")]
    pub preview: Option<String>,
    /// The owner chose the preview; otherwise GitHub generated it.
    #[serde(rename = "usesCustomOpenGraphImage")]
    pub custom_preview: bool,
    #[serde(rename = "issueOrPullRequest")]
    pub issue: Option<Thread>,
    pub discussion: Option<Thread>,
}

/// An issue, pull request or discussion.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Thread {
    #[serde(rename = "__typename")]
    pub kind: String,
    pub title: String,
    pub body: String,
    pub url: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub author: Option<Login>,
    /// `OPEN`, `CLOSED` or `MERGED`; discussions have none.
    pub state: Option<String>,
    pub category: Option<Category>,
    pub answer: Option<Comment>,
    pub comments: Comments,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Comments {
    #[serde(rename = "totalCount")]
    pub total: usize,
    pub nodes: Vec<Comment>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Comment {
    pub id: String,
    pub author: Option<Login>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub body: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Login {
    pub login: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Category {
    pub name: String,
}

/// `@login`; a deleted account is GitHub's `@ghost`.
fn who(author: Option<&Login>) -> String {
    match author {
        Some(author) if !author.login.is_empty() => format!("@{}", author.login),
        _ => "@ghost".to_owned(),
    }
}

/// A page and, when it is thin, why. The extractor is filled in by the
/// caller, which knows the `gh` version.
fn page(
    title: String,
    markdown: String,
    has_text: bool,
    thin: &'static str,
) -> (Page, Option<&'static str>) {
    let page = Page {
        title: Some(title),
        extractor: String::new(),
        completeness: if has_text {
            Completeness::Full
        } else {
            Completeness::Thin
        },
        chars: extract::plain_chars(&markdown),
        captions: None,
        markdown,
    };
    (page, (!has_text).then_some(thin))
}

/// `# owner/name`, the description, the topics, then the README as its
/// author wrote it. Ends in one newline.
pub fn repo_page(
    repo: &Repo,
    info: &RepoInfo,
    readme: Option<&str>,
) -> (Page, Option<&'static str>) {
    let title = if info.full_name.is_empty() {
        repo.full_name()
    } else {
        info.full_name.clone()
    };
    let mut parts = vec![format!("# {title}")];
    if let Some(description) = info.description.as_deref().map(str::trim)
        && !description.is_empty()
    {
        parts.push(description.to_owned());
    }
    if !info.topics.is_empty() {
        parts.push(format!("Topics: {}", info.topics.join(", ")));
    }
    let readme = readme.map(str::trim).filter(|text| !text.is_empty());
    parts.extend(readme.map(str::to_owned));
    let markdown = format!("{}\n", parts.join("\n\n"));
    page(title, markdown, readme.is_some(), "no README")
}

/// `# Title`, what it is and who opened it when, its state, the opening
/// post, then the first comments, each under its author and date. A
/// discussion's answer follows when it is not among them.
pub fn thread_page(repo: &Repo, number: u32, thread: &Thread) -> (Page, Option<&'static str>) {
    let (noun, verb) = match thread.kind.as_str() {
        "PullRequest" => ("Pull request", "opened"),
        "Discussion" => ("Discussion", "started"),
        _ => ("Issue", "opened"),
    };
    let mut about = format!("{noun} #{number} in {}", repo.full_name());
    if let Some(category) = thread.category.as_ref().filter(|c| !c.name.is_empty()) {
        let _ = write!(about, " ({})", category.name);
    }
    let _ = write!(about, ", {verb} by {}", who(thread.author.as_ref()));
    if !thread.created_at.is_empty() {
        let _ = write!(about, ", {}", thread.created_at);
    }
    about.push('.');
    if let Some(state) = thread.state.as_deref() {
        let _ = write!(about, " {}.", capitalised(&state.to_ascii_lowercase()));
    }
    if thread.answer.is_some() {
        about.push_str(" Answered.");
    }
    let title = thread.title.trim().to_owned();
    let mut parts = vec![format!("# {title}"), about];
    let body = thread.body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    let comments = &thread.comments;
    if !comments.nodes.is_empty() {
        parts.push("## Comments".to_owned());
        parts.push(if comments.total > comments.nodes.len() {
            format!(
                "The first {} of {} comments.",
                comments.nodes.len(),
                comments.total
            )
        } else {
            match comments.nodes.len() {
                1 => "1 comment.".to_owned(),
                n => format!("{n} comments."),
            }
        });
        parts.extend(comments.nodes.iter().map(|c| comment("###", "", c)));
    }
    if let Some(answer) = &thread.answer
        && !comments.nodes.iter().any(|c| c.id == answer.id)
    {
        parts.push(comment("##", "Answer: ", answer));
    }
    let has_text = !body.is_empty()
        || comments
            .nodes
            .iter()
            .chain(thread.answer.as_ref())
            .any(|c| !c.body.trim().is_empty());
    let markdown = format!("{}\n", parts.join("\n\n"));
    page(title, markdown, has_text, "no text beyond the title")
}

fn comment(level: &str, label: &str, comment: &Comment) -> String {
    let mut heading = format!("{level} {label}{}", who(comment.author.as_ref()));
    if !comment.created_at.is_empty() {
        let _ = write!(heading, ", {}", comment.created_at);
    }
    let body = comment.body.trim();
    if body.is_empty() {
        heading
    } else {
        format!("{heading}\n\n{body}")
    }
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn repo() -> Repo {
        Repo {
            owner: "some-owner".to_owned(),
            name: "tide".to_owned(),
        }
    }

    #[test]
    fn a_repository_keeps_its_description_topics_and_readme() {
        let info: RepoInfo = serde_json::from_value(json!({
            "full_name": "Some-Owner/tide", "html_url": "https://github.com/Some-Owner/tide",
            "description": "  Tide tables, offline.  ", "topics": ["tides", "cli"],
            "private": false, "stargazers_count": 3,
        }))
        .unwrap();
        let readme = "# Tide\n\nReads tide tables.\n\n```sh\ntide --port 1\n```\n";
        let (page, thin) = repo_page(&repo(), &info, Some(readme));
        assert_eq!(thin, None);
        assert_eq!(page.title.as_deref(), Some("Some-Owner/tide"));
        assert_eq!(page.completeness, Completeness::Full);
        assert_eq!(
            page.markdown,
            "# Some-Owner/tide\n\nTide tables, offline.\n\nTopics: tides, cli\n\n\
             # Tide\n\nReads tide tables.\n\n```sh\ntide --port 1\n```\n"
        );
        assert_eq!(page.chars, extract::plain_chars(&page.markdown));
    }

    #[test]
    fn a_repository_without_a_readme_is_thin() {
        let info: RepoInfo = serde_json::from_value(json!({"description": null})).unwrap();
        let (page, thin) = repo_page(&repo(), &info, None);
        assert_eq!(thin, Some("no README"));
        assert_eq!(page.completeness, Completeness::Thin);
        assert_eq!(page.markdown, "# some-owner/tide\n");
        let (_, thin) = repo_page(&repo(), &info, Some("  \n"));
        assert_eq!(thin, Some("no README"));
    }

    fn graph(value: &serde_json::Value) -> Graph {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn thread(value: &serde_json::Value) -> Thread {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn comment_node(id: &str, login: Option<&str>, body: &str) -> serde_json::Value {
        json!({"id": id, "author": login.map(|l| json!({"login": l})),
            "createdAt": "2026-10-07T10:00:00Z", "body": body})
    }

    #[test]
    fn an_issue_keeps_its_body_and_first_comments_with_the_total() {
        let issue = thread(&json!({
            "__typename": "Issue", "state": "CLOSED", "title": " Tides are off by an hour ",
            "body": "Since the clocks changed.\n", "url": "https://github.com/some-owner/tide/issues/12",
            "createdAt": "2026-10-07T09:00:00Z", "author": {"login": "reporter"},
            "comments": {"totalCount": 25, "nodes": [
                comment_node("c1", Some("maintainer"), "Fixed in 1.2."),
                comment_node("c2", None, "Thanks!"),
            ]},
        }));
        let (page, thin) = thread_page(&repo(), 12, &issue);
        assert_eq!(thin, None);
        assert_eq!(page.title.as_deref(), Some("Tides are off by an hour"));
        assert_eq!(
            page.markdown,
            "# Tides are off by an hour\n\n\
             Issue #12 in some-owner/tide, opened by @reporter, 2026-10-07T09:00:00Z. Closed.\n\n\
             Since the clocks changed.\n\n\
             ## Comments\n\n\
             The first 2 of 25 comments.\n\n\
             ### @maintainer, 2026-10-07T10:00:00Z\n\nFixed in 1.2.\n\n\
             ### @ghost, 2026-10-07T10:00:00Z\n\nThanks!\n"
        );
    }

    #[test]
    fn a_pull_request_says_it_was_merged() {
        let pull = thread(&json!({
            "__typename": "PullRequest", "state": "MERGED", "title": "Add ports",
            "body": "Adds two ports.", "url": "https://github.com/some-owner/tide/pull/7",
            "createdAt": "2026-10-07T09:00:00Z", "author": {"login": "contributor"},
            "comments": {"totalCount": 1, "nodes": [comment_node("c1", Some("maintainer"), "")]},
        }));
        let (page, thin) = thread_page(&repo(), 7, &pull);
        assert_eq!(thin, None);
        assert_eq!(
            page.markdown,
            "# Add ports\n\n\
             Pull request #7 in some-owner/tide, opened by @contributor, \
             2026-10-07T09:00:00Z. Merged.\n\n\
             Adds two ports.\n\n## Comments\n\n1 comment.\n\n\
             ### @maintainer, 2026-10-07T10:00:00Z\n"
        );
    }

    #[test]
    fn a_discussion_keeps_its_category_and_an_answer_past_the_first_comments() {
        let value = json!({"data": {"repository": {"isPrivate": false, "discussion": {
            "__typename": "Discussion", "title": "Which datum?", "body": "",
            "url": "https://github.com/some-owner/tide/discussions/3",
            "createdAt": "2026-10-07T09:00:00Z", "author": {"login": "asker"},
            "category": {"name": "Q&A"},
            "answer": comment_node("late", Some("expert"), "Chart datum."),
            "comments": {"totalCount": 11, "nodes": [comment_node("c1", Some("other"), "Good question.")]},
        }}}});
        let graph = graph(&value);
        assert!(graph.errors.is_empty());
        let repository = graph.data.unwrap().repository.unwrap();
        let discussion = repository.discussion.unwrap();
        let (page, thin) = thread_page(&repo(), 3, &discussion);
        assert_eq!(thin, None, "comments are text even when the body is empty");
        assert_eq!(
            page.markdown,
            "# Which datum?\n\n\
             Discussion #3 in some-owner/tide (Q&A), started by @asker, \
             2026-10-07T09:00:00Z. Answered.\n\n\
             ## Comments\n\nThe first 1 of 11 comments.\n\n\
             ### @other, 2026-10-07T10:00:00Z\n\nGood question.\n\n\
             ## Answer: @expert, 2026-10-07T10:00:00Z\n\nChart datum.\n"
        );
        let mut answered = discussion;
        answered.answer = Some(Comment {
            id: "c1".to_owned(),
            ..Comment::default()
        });
        let (page, _) = thread_page(&repo(), 3, &answered);
        assert!(!page.markdown.contains("## Answer"), "{}", page.markdown);
    }

    #[test]
    fn a_thread_with_only_a_title_is_thin() {
        let bare = thread(
            &json!({"__typename": "Issue", "state": "OPEN", "title": "Idea",
            "url": "https://github.com/some-owner/tide/issues/2"}),
        );
        let (page, thin) = thread_page(&repo(), 2, &bare);
        assert_eq!(thin, Some("no text beyond the title"));
        assert_eq!(page.completeness, Completeness::Thin);
        assert_eq!(
            page.markdown,
            "# Idea\n\nIssue #2 in some-owner/tide, opened by @ghost. Open.\n"
        );
    }

    #[test]
    fn the_thread_query_asks_for_the_first_comments_and_privacy() {
        let issue = thread_query(ThreadKind::IssueOrPull);
        assert!(issue.contains(&format!(
            "isPrivate {PREVIEW} issueOrPullRequest(number:$number)"
        )));
        assert!(issue.contains(&format!("comments(first:{COMMENTS})")));
        assert!(issue.contains("...on PullRequest{state "));
        let discussion = thread_query(ThreadKind::Discussion);
        assert!(discussion.contains("discussion(number:$number)"));
        assert!(discussion.contains("answer{id author{login} createdAt body}"));
        assert_eq!(issue.matches('{').count(), issue.matches('}').count());
        assert_eq!(
            discussion.matches('{').count(),
            discussion.matches('}').count()
        );
    }

    #[test]
    fn a_repository_image_is_its_own_preview_then_its_readme_then_the_card() {
        let repo = Repo {
            owner: "owner".to_owned(),
            name: "repo".to_owned(),
        };
        let graph = |url: &str, custom: bool| GraphRepo {
            preview: Some(url.to_owned()),
            custom_preview: custom,
            ..GraphRepo::default()
        };
        let named = |found: Found| match found {
            Found::Candidates(found) => found
                .into_iter()
                .map(|c| (c.url, c.source))
                .collect::<Vec<_>>(),
            other => panic!("expected candidates, got {other:?}"),
        };
        let readme = "[![ci](https://img.shields.io/x)](y)\n![Screenshot](docs/shot.png)\n";
        let shot = "https://raw.githubusercontent.com/owner/repo/HEAD/docs/shot.png".to_owned();
        let custom = "https://repository-images.test/social.png".to_owned();
        let card = "https://opengraph.githubassets.com/1/owner/repo".to_owned();
        assert_eq!(
            named(images(&repo, Some(&graph(&custom, true)), Some(readme))),
            [
                (custom.clone(), Source::GithubSocial),
                (shot.clone(), Source::GithubReadme)
            ]
        );
        assert_eq!(
            named(images(&repo, Some(&graph(&card, false)), Some(readme))),
            [
                (shot.clone(), Source::GithubReadme),
                (card.clone(), Source::GithubCard)
            ]
        );
        assert_eq!(
            named(images(&repo, Some(&graph(&card, false)), None)),
            [(card, Source::GithubCard)]
        );
        assert_eq!(
            named(images(&repo, None, Some(readme))),
            [(shot, Source::GithubReadme)],
            "without an answer about the preview"
        );
        let answer: Graph = serde_json::from_str(
            r#"{"data":{"repository":{"openGraphImageUrl":"https://x.test/p.png","usesCustomOpenGraphImage":true}}}"#,
        )
        .unwrap();
        let read = answer.data.unwrap().repository.unwrap();
        assert_eq!(read.preview.as_deref(), Some("https://x.test/p.png"));
        assert!(read.custom_preview);
        assert!(preview_query().contains(PREVIEW));
        assert_eq!(
            preview_query().matches('{').count(),
            preview_query().matches('}').count()
        );
    }
}
