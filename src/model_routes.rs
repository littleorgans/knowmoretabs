//! What the shell forwards to the model service below its prefix: each
//! admitted method and path, the type its answer must have, and how long it
//! may take.
//!
//! slice: library, triage
//! why: One table admits every forward, so an unknown route, a wrong method,
//!      a nested or encoded name or a traversal is refused before any
//!      connection is made, and each route's type and deadline are decided
//!      in one place. Tag and Add link's page, files, images and API are
//!      listed here until their views move into `web/`.

use std::time::Duration;

use crate::image_store;

/// What a route answers, which fixes the type the shell gives it and the
/// types it takes from the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Html,
    Css,
    Script,
    Jpeg,
    Json,
}

impl Kind {
    /// The type the shell answers with.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Html => "text/html; charset=utf-8",
            Self::Css => "text/css; charset=utf-8",
            Self::Script => "text/javascript; charset=utf-8",
            Self::Jpeg => "image/jpeg",
            Self::Json => "application/json",
        }
    }

    /// Whether the service's type, parameters aside, is this kind's.
    pub fn takes(self, content_type: &str) -> bool {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match self {
            Self::Html => essence == "text/html",
            Self::Css => essence == "text/css",
            Self::Script => matches!(
                essence.as_str(),
                "text/javascript" | "application/javascript"
            ),
            Self::Jpeg => essence == "image/jpeg",
            Self::Json => essence == "application/json",
        }
    }
}

/// One admitted forward: what to ask the service and by when.
#[derive(Debug, PartialEq, Eq)]
pub struct Route {
    pub method: &'static str,
    /// The service's path, from its root.
    pub path: String,
    pub kind: Kind,
    /// One deadline over connect, write and read.
    pub deadline: Duration,
}

/// The Library searches by keyword rather than wait on a busy or stuck model.
const RANK: Duration = Duration::from_secs(2);
/// The page, its files, images and the screens' API.
const ORDINARY: Duration = Duration::from_secs(10);

/// The service's files besides its page (`lab/tagger/tagger/app/static`),
/// each a single name at its root.
pub const FILES: [(&str, Kind); 10] = [
    ("add.js", Kind::Script),
    ("app.css", Kind::Css),
    ("app.js", Kind::Script),
    ("core.js", Kind::Script),
    ("lit-html.js", Kind::Script),
    ("lit.js", Kind::Script),
    ("pages.js", Kind::Script),
    ("panel.js", Kind::Script),
    ("strip.js", Kind::Script),
    ("views.js", Kind::Script),
];

/// The service's API below `api/`, by method (`ROUTES`, `RANK_ROUTES` and
/// `JOB_ROUTES` in `lab/tagger/tagger/app/server.py`); `#` is one decimal
/// segment. The screens' tag, vocabulary, forget and restore writes are
/// this shell's own (`server.rs`), never the service's.
pub const API: [(&str, &str); 11] = [
    ("GET", "library"),
    ("GET", "selection"),
    ("GET", "pins"),
    ("GET", "add/#"),
    ("POST", "rank"),
    ("POST", "search"),
    ("POST", "exclusions"),
    ("POST", "selection"),
    ("POST", "pins"),
    ("POST", "like"),
    ("POST", "add"),
];

/// The route a request names by its method and its path below the prefix,
/// query cut; `None` for anything else.
pub fn classify(method: &str, below: &str) -> Option<Route> {
    let file = FILES.iter().find(|&&(name, _)| name == below);
    let (method, kind) = match (method, below, file) {
        ("GET", "" | "index.html", _) => ("GET", Kind::Html),
        ("GET", _, Some(&(_, kind))) => ("GET", kind),
        ("GET", _, None) if below.strip_prefix("img/").is_some_and(image_store::is_name) => {
            ("GET", Kind::Jpeg)
        }
        _ => {
            let api = below.strip_prefix("api/")?;
            let &(method, _) = API
                .iter()
                .find(|&&(m, pattern)| m == method && fits(pattern, api))?;
            (method, Kind::Json)
        }
    };
    let deadline = match (method, below) {
        ("POST", "api/rank") => RANK,
        _ => ORDINARY,
    };
    Some(Route {
        method,
        path: format!("/{below}"),
        kind,
        deadline,
    })
}

/// Segment by segment, `#` taking one to eighteen ASCII digits.
fn fits(pattern: &str, path: &str) -> bool {
    let mut given = path.split('/');
    pattern.split('/').all(|want| {
        given.next().is_some_and(|segment| {
            if want == "#" {
                (1..=18).contains(&segment.len()) && segment.bytes().all(|b| b.is_ascii_digit())
            } else {
                segment == want
            }
        })
    }) && given.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn route(method: &str, below: &str) -> Option<(String, Kind, u64)> {
        classify(method, below).map(|r| (r.path, r.kind, r.deadline.as_secs()))
    }

    #[test]
    fn every_family_is_admitted_with_its_type_and_deadline() {
        let image = format!("img/{HASH}");
        for (method, below, path, kind, secs) in [
            ("GET", "", "/", Kind::Html, 10),
            ("GET", "index.html", "/index.html", Kind::Html, 10),
            ("GET", "app.css", "/app.css", Kind::Css, 10),
            ("GET", "lit-html.js", "/lit-html.js", Kind::Script, 10),
            ("GET", &image, &format!("/{image}"), Kind::Jpeg, 10),
            ("GET", "api/library", "/api/library", Kind::Json, 10),
            ("GET", "api/add/42", "/api/add/42", Kind::Json, 10),
            ("POST", "api/rank", "/api/rank", Kind::Json, 2),
            ("POST", "api/add", "/api/add", Kind::Json, 10),
        ] {
            assert_eq!(
                route(method, below),
                Some((path.to_owned(), kind, secs)),
                "{method} {below}"
            );
        }
        for (name, kind) in FILES {
            assert_eq!(classify("GET", name).map(|r| r.kind), Some(kind), "{name}");
        }
        for (method, pattern) in API {
            assert!(classify(method, &format!("api/{}", pattern.replace('#', "1"))).is_some());
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for (method, below) in [
            ("POST", ""),
            ("POST", "index.html"),
            ("GET", "api/rank"),
            ("POST", "api/library"),
            ("GET", "api/add/1/pages/2"),
            ("GET", "index.htm"),
            ("GET", "server.py"),
            ("GET", "static/app.js"),
            ("GET", "app.js/"),
            ("GET", "./app.js"),
            ("GET", "../app.js"),
            ("GET", "..%2Fapp.js"),
            ("GET", "api/../api/library"),
            ("GET", "api//library"),
            ("GET", "/api/library"),
            ("GET", "api/library/"),
            ("GET", "api/add/"),
            ("GET", "api/add/1a"),
            ("GET", "api/add/-1"),
            ("GET", "api/add/%31"),
            ("GET", "api/add/1234567890123456789"),
            ("POST", "api/rank/"),
            ("PUT", "api/like"),
            // The screens' old decision, session and Send routes and scripts.
            ("POST", "api/tags"),
            ("POST", "api/apply"),
            ("POST", "api/undo"),
            ("POST", "api/forget"),
            ("GET", "api/sessions"),
            ("GET", "api/sessions/7"),
            ("POST", "api/sessions"),
            ("POST", "api/sessions/7/pages/3"),
            ("POST", "api/send/preview"),
            ("POST", "api/send"),
            ("GET", "review.js"),
            ("GET", "swipe.js"),
            ("GET", "pick.js"),
            ("GET", "search.js"),
            ("GET", "img/"),
            ("GET", &format!("img/{}", &HASH[1..])),
            ("GET", &format!("img/{}", HASH.to_uppercase())),
            ("GET", &format!("img/{HASH}.jpg")),
            ("GET", &format!("img/x/{HASH}")),
        ] {
            assert_eq!(classify(method, below), None, "{method} {below}");
        }
    }

    #[test]
    fn types_are_taken_by_their_essence_only() {
        assert!(Kind::Html.takes("text/html; charset=utf-8"));
        assert!(Kind::Script.takes("application/javascript"));
        assert!(Kind::Script.takes("text/javascript; charset=utf-8"));
        assert!(Kind::Json.takes("Application/JSON; charset=utf-8"));
        assert!(!Kind::Json.takes("text/html"));
        assert!(!Kind::Css.takes("text/plain"));
        assert!(!Kind::Jpeg.takes("image/png"));
        assert!(!Kind::Html.takes(""));
    }
}
