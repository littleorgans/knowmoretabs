//! Synthetic fixtures shared by content command and planner tests.
//!
//! slice: content
//! why: Both test the same library and attempt shapes, so their fixtures
//!      have one owner independent of either module's private tests.

use std::path::Path;

use crate::archive::{Archive, SNAPSHOT_JSON};
use crate::browser;
use crate::content_plan::Work;
use crate::content_store::{self, Line, Status};
use crate::github_api::Readiness;
use crate::library::State;
use crate::model::Snapshot;
use crate::ytdlp;

pub(crate) fn snapshot(urls: &[&str]) -> Snapshot {
    let tabs: Vec<serde_json::Value> = urls
        .iter()
        .enumerate()
        .map(|(i, url)| {
            serde_json::json!({"tab_id": i, "window": 1, "position": i, "url": url,
                "title": "", "pinned": false, "active": false, "group": null,
                "last_active": null, "window_id": 1})
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "schema_version": 1, "id": "2026-10-07-090000Z", "captured_at": "2026-10-07T09:00:00Z",
        "source": {"browser": null, "profile": null, "profile_display": null, "path": "/x",
            "file": "session.snss", "sha256": "", "bytes": 0, "saved_at": null,
            "session_started_at": null},
        "stats": {"file_version": 3, "command_table": "session", "commands": 0,
            "commands_by_id": {}, "unknown_commands": 0, "unknown_command_ids": [],
            "malformed_commands": 0, "truncated_bytes": 0, "marker_count": 1, "marker_ok": true,
            "windows": 0, "tabs": 0, "groups": 0, "dropped_tabs": 0,
            "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0, "window_closed": 0},
            "navigation_fallbacks": 0, "groups_without_metadata": 0},
        "windows": [], "groups": [], "tabs": tabs,
    }))
    .unwrap()
}

/// [`snapshot`] of `urls`, saved in the archive at `root` as `save` would.
pub(crate) fn saved(root: &Path, urls: &[&str]) {
    let archive = Archive::open(root).unwrap();
    let snapshot = snapshot(urls);
    let dir = archive.snapshots_dir().join(&snapshot.id);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join(SNAPSHOT_JSON),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
}

pub(crate) fn state(forgotten: &[&str]) -> State {
    serde_json::from_value(serde_json::json!({"schema_version": 1, "forgotten": forgotten}))
        .unwrap()
}

pub(crate) fn log(lines: &[(&str, Status, u32)]) -> content_store::Log {
    let mut log = content_store::Log::default();
    for (url, status, attempt) in lines {
        let mut line = Line::new(url, *status);
        line.attempt = *attempt;
        log.pages.insert((*url).to_owned(), line);
    }
    log
}

pub(crate) fn no_gh() -> Readiness {
    Readiness::Missing
}

pub(crate) fn no_ytdlp() -> ytdlp::Readiness {
    ytdlp::Readiness::Missing
}

pub(crate) fn no_browser() -> browser::Readiness {
    browser::Readiness::Missing("no chrome binary found".to_owned())
}

pub(crate) fn unsent(work: &Work) -> Vec<(&str, Status, &str)> {
    work.unsent
        .iter()
        .map(|line| {
            (
                line.url.as_str(),
                line.status,
                line.reason.as_deref().unwrap_or(""),
            )
        })
        .collect()
}

/// A browser's user-data directory holding only what a signed in run
/// reads: `Local State` with its remote debugging switch `flag` (no
/// switch when `None`), and the port file `port` when given.
pub(crate) fn chrome(flag: Option<bool>, port: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let devtools = flag.map_or(
        serde_json::json!({}),
        |on| serde_json::json!({"remote_debugging": {"user-enabled": on}}),
    );
    let state = serde_json::json!({"profile": {}, "devtools": devtools});
    std::fs::write(dir.path().join("Local State"), state.to_string()).unwrap();
    if let Some(port) = port {
        std::fs::write(dir.path().join(crate::browser::PORT_FILE), port).unwrap();
    }
    dir
}

/// Where [`Peer`] says its `DevTools` socket is.
pub(crate) const PEER_ROUTE: &str = "/devtools/browser/peer";

/// A loopback stand-in for the owner's browser, for tests that attach. It
/// answers what a signed in run sends: each tab it creates is `T<n>`,
/// attached as `S<n>`, and loads at once with enough text to keep, after
/// an event of another session; a page whose address says `fails` fails
/// to load. It records every message it is sent, and answers `closing` by
/// closing the connection, as a browser that quits does.
pub(crate) struct Peer {
    pub(crate) port: u16,
    received: std::thread::JoinHandle<Vec<serde_json::Value>>,
}

impl Peer {
    pub(crate) fn start(closing: Option<&'static str>) -> Self {
        use serde_json::json;
        use tungstenite::Message;
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut received = Vec::new();
            let (mut tabs, mut navigated) = (0, String::new());
            while let Ok(Message::Text(text)) = ws.read() {
                let message: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                received.push(message.clone());
                let method = message["method"].as_str().unwrap_or_default();
                if Some(method) == closing {
                    break;
                }
                let session = message["sessionId"].as_str().unwrap_or_default().to_owned();
                let result = match method {
                    "Browser.getVersion" => json!({"product": "Chrome/155.0.8059.39"}),
                    "Target.createTarget" => {
                        tabs += 1;
                        json!({"targetId": format!("T{tabs}")})
                    }
                    "Target.attachToTarget" => {
                        let target = message["params"]["targetId"].as_str().unwrap();
                        json!({"sessionId": target.replacen('T', "S", 1)})
                    }
                    "Page.navigate" => {
                        message["params"]["url"]
                            .as_str()
                            .unwrap()
                            .clone_into(&mut navigated);
                        let error = if navigated.contains("fails") {
                            "net::ERR_ABORTED"
                        } else {
                            ""
                        };
                        json!({"frameId": "F", "loaderId": "L", "errorText": error})
                    }
                    "Runtime.evaluate" => json!({"result": {"value": {
                        "url": navigated, "mime": "text/html", "status": 200,
                        "html": format!("<main><p>{}</p></main>", "Signed in text. ".repeat(120)),
                    }}}),
                    _ => json!({}),
                };
                let mut out = vec![json!({"id": message["id"], "result": result})];
                if method == "Page.navigate" && !navigated.contains("fails") {
                    for (from, name) in [
                        ("OTHER", "init"),
                        (session.as_str(), "load"),
                        (session.as_str(), "networkIdle"),
                    ] {
                        out.push(json!({"method": "Page.lifecycleEvent", "sessionId": from,
                            "params": {"frameId": "F", "loaderId": "L", "name": name}}));
                    }
                }
                for message in out {
                    if ws.send(Message::text(message.to_string())).is_err() {
                        return received;
                    }
                }
            }
            received
        });
        Self { port, received }
    }

    /// Every message the peer was sent, once the connection has closed.
    pub(crate) fn received(self) -> Vec<serde_json::Value> {
        self.received.join().unwrap()
    }
}

/// A page's text as a capture keeps it, thin.
pub(crate) fn page(markdown: &str, chars: usize) -> content_store::Page {
    content_store::Page {
        title: None,
        extractor: "dom_smoothie 0.18.2".to_owned(),
        completeness: content_store::Completeness::Thin,
        chars,
        captions: None,
        markdown: markdown.to_owned(),
    }
}

/// A loopback stand-in for every site a test reads, one request per
/// connection: `answer` gives the whole response to the `n`th request (from
/// 1) for a path, or `None` to read the request and say nothing.
pub(crate) fn site(
    answer: impl Fn(&str, usize) -> Option<String> + Send + Sync + 'static,
) -> std::net::SocketAddr {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::{Arc, Mutex};
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let answer = Arc::new(answer);
    let asked: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let (answer, asked) = (Arc::clone(&answer), Arc::clone(&asked));
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap_or_default();
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or_default();
                let n = {
                    let mut asked = asked.lock().unwrap();
                    let n = asked.entry(path.to_owned()).or_default();
                    *n += 1;
                    *n
                };
                match answer(path, n) {
                    Some(response) => {
                        let _ = stream.write_all(response.as_bytes());
                    }
                    None => std::thread::sleep(std::time::Duration::from_secs(5)),
                }
            });
        }
    });
    address
}

/// A whole HTTP response of `status` with an HTML `body`.
pub(crate) fn html(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// An article titled `title` with enough text to keep, or, `thin`, too
/// little.
pub(crate) fn article(title: &str, thin: bool) -> String {
    let words = if thin { 2 } else { 150 };
    format!(
        "<html><head><title>{title}</title></head><body><main><h1>{title}</h1><p>{}</p></main></body></html>",
        "Words the page says about itself. ".repeat(words)
    )
}
