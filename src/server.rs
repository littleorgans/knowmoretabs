//! `knowmoretabs serve`: the library over HTTP/1.1 on 127.0.0.1.
//!
//! slice: triage, platforms, tags
//! why: Routing over the archive: ten routes, one client, each answered
//!      from the archive on disk and every write through the same locked
//!      paths the commands use, plus one prefix forwarded to the model
//!      service (`model_service`). It binds loopback only; the wire, its
//!      bounds and the Host and Origin checks are `http`'s. The image
//!      route's name is checked where the images live
//!      (`image_store::served`); the content route answers only a page the
//!      library lists (`content_view::served`).

use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;

use crate::archive::Archive;
use crate::assets::{Policy, Surface};
use crate::capture::Log;
use crate::error::Error;
use crate::http::{
    self, BODY_LIMIT, Framing, HEAD_LIMIT, Head, IO_TIMEOUT, Response, Status, json_request,
};
use crate::library::{self, Shape};
use crate::model_service::{self, ModelService};
use crate::triage::{self, Action};
use crate::{
    assets, content_store, content_view, export, image_store, library_history, out, suggestions,
    tags,
};

pub struct Options {
    pub root: PathBuf,
    pub port: u16,
    /// Where Library search is forwarded; `None` keeps it keyword only.
    pub model_port: Option<u16>,
    /// The key that proves each forward and each reply; forwarding is off
    /// without a usable one.
    pub model_secret_file: Option<PathBuf>,
    pub open: bool,
    pub json: bool,
    pub log: Log,
}

pub fn run(options: &Options) -> Result<(), Error> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, options.port)).map_err(|source| Error::Bind {
            port: options.port,
            source,
        })?;
    let port = listener
        .local_addr()
        .map_err(|source| Error::Bind {
            port: options.port,
            source,
        })?
        .port();
    if options.model_port == Some(port) {
        return Err(Error::ModelPort { port });
    }
    // A damaged state file is refused here, at the terminal, rather than as
    // a 500 the page can only describe as "HTTP 500".
    library::State::read(&options.root)?;
    // A damaged History record only costs the signals it held, so it is a
    // warning, given once here rather than on every request.
    library_history::for_library(&options.root, options.log);
    // Settled, and said, before the banner announces the library.
    let model = options.model_port.and_then(|port| {
        ModelService::new(port, options.model_secret_file.as_deref(), options.log)
    });
    let url = format!("http://127.0.0.1:{port}/");
    // Through `out`: the banner is written after the port is bound and
    // accepting, and on the main thread, so a `println!` that panicked on a
    // closed pipe took down a server people were already connected to.
    if options.json {
        out::json(&serde_json::json!({ "serving": { "url": url, "local_only": true } }));
    } else if !options.log.quiet {
        out::line(&format!("Your library is at {url}"));
        out::line("Local only: it answers this machine and nothing else. Press Ctrl-C to stop.");
    }
    options.log.note(&format!("listening on {url}"));
    if options.open
        && let Err(err) = open_browser(&url)
    {
        options.log.warn(&format!(
            "could not open a browser ({err}); visit {url} yourself"
        ));
    }
    let server = Arc::new(Server {
        root: options.root.clone(),
        port,
        model,
        log: options.log,
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let handler = Arc::clone(&server);
                let spawned = std::thread::Builder::new()
                    .name("connection".to_owned())
                    .spawn(move || handler.serve(stream));
                if let Err(err) = spawned {
                    server.log.warn(&format!("dropped a connection: {err}"));
                }
            }
            Err(err) => server.log.note(&format!("accept failed: {err}")),
        }
    }
    Ok(())
}

/// Hands the URL to whatever the desktop uses to open one. Three commands,
/// one per platform, and none of them may fail the run: a headless Linux box
/// has no `xdg-open`, and `serve` is still doing its job without a window.
/// The caller warns and carries on.
///
/// Windows goes through `COMSPEC` because `start` is a shell builtin rather
/// than a program. The empty argument after it is `start`'s window title,
/// which it would otherwise take the URL to be. `cmd` would treat `&` or `|` in the
/// URL as its own syntax, which is why this only ever passes a URL this
/// process built: `http://127.0.0.1:<port>/`, digits and nothing else.
fn open_browser(url: &str) -> io::Result<()> {
    // `cfg!` rather than `#[cfg]`: all three arms are compiled everywhere, so
    // a change to the Windows one is caught by a build on a Mac.
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let shell = std::env::var_os("COMSPEC")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "cmd.exe".into());
        let mut c = std::process::Command::new(shell);
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

struct Server {
    root: PathBuf,
    port: u16,
    model: Option<ModelService>,
    log: Log,
}

impl Server {
    fn serve(&self, mut stream: TcpStream) {
        let deadline = Instant::now() + IO_TIMEOUT;
        let (head, rest) = match http::read_head(&mut stream, deadline, HEAD_LIMIT) {
            Ok(head) => head,
            // Opened and closed without a request: a preconnect, not an error.
            Err(Framing::Closed) => return,
            Err(framing) => {
                self.finish(&mut stream, "<unreadable>", &unread_head(framing));
                return;
            }
        };
        let (line, response) = match Head::parse(&head) {
            Err(response) => ("<malformed>".to_owned(), response),
            Ok(head) => {
                let line = format!("{} {}", head.method, head.target);
                // Judge Host and Origin before reading any further body bytes;
                // `rest` may contain bytes prefetched with the head.
                let body = http::check_headers(&head, self.port).and_then(|()| {
                    http::read_body(&mut stream, &head.fields, rest, deadline, BODY_LIMIT)
                        .map_err(unread_body)
                });
                let response = match body {
                    Ok(body) => self.route(&head, &body),
                    Err(response) => response,
                };
                (line, response)
            }
        };
        self.finish(&mut stream, &line, &response);
    }

    fn finish(&self, stream: &mut TcpStream, line: &str, response: &Response) {
        self.log.note(&format!("{} {line}", response.status.code()));
        http::respond(stream, response);
    }

    fn route(&self, head: &Head, body: &[u8]) -> Response {
        let path = head.target.split('?').next().unwrap_or("");
        match (head.method.as_str(), path) {
            ("GET", "/" | "/index.html") => self.asset("index.html"),
            ("GET", "/app.css" | "/app.js" | "/unavailable.js") => self.asset(&path[1..]),
            ("GET", "/api/library") => self.library(),
            ("GET", "/api/content") => self.content(&head.target),
            ("POST", "/api/forget") => self.triage(head, body, Action::Forget),
            ("POST", "/api/restore") => self.triage(head, body, Action::Restore),
            ("POST", "/api/tags") => self.tags(head, body),
            ("POST", "/api/vocabulary") => self.vocabulary(head, body),
            ("GET", _) if path.starts_with(image_store::ROUTE) => {
                match image_store::served(&self.root, &path[image_store::ROUTE.len()..]) {
                    Ok(Some(jpeg)) => Response::ok("image/jpeg", jpeg),
                    Ok(None) => Response::error(Status::NotFound, "no such image"),
                    Err(err) => self.failure(&err),
                }
            }
            // Below the prefix, so the page's relative files are the service's.
            ("GET", "/model") => Response::redirect(model_service::PREFIX),
            _ if path.starts_with(model_service::PREFIX) => {
                model_service::forward(self.model.as_ref(), &self.root, head, path, body, self.log)
            }
            (
                _,
                "/" | "/index.html" | "/app.css" | "/app.js" | "/unavailable.js" | "/api/library"
                | "/api/content",
            ) => Response::method_not_allowed("GET"),
            (_, "/api/forget" | "/api/restore" | "/api/tags" | "/api/vocabulary") => {
                Response::method_not_allowed("POST")
            }
            _ => Response::error(Status::NotFound, "no such page"),
        }
    }

    /// The same files `export` writes, and the unavailable page's script;
    /// the data element is served empty so the page knows to fetch.
    fn asset(&self, name: &str) -> Response {
        if name == "index.html" {
            return match export::page(Surface::Serve, "") {
                Ok(html) => Response::html(Status::Ok, html.into_bytes(), Policy::Library),
                Err(err) => self.failure(&err),
            };
        }
        let Some(content) = assets::load(name) else {
            return Response::error(Status::NotFound, "no such page");
        };
        match name {
            "app.css" => Response::ok("text/css; charset=utf-8", content.as_bytes().to_vec()),
            _ => Response::ok(
                "text/javascript; charset=utf-8",
                content.as_bytes().to_vec(),
            ),
        }
    }

    fn library(&self) -> Response {
        match self.build_library() {
            Ok(json) => Response::ok("application/json", json),
            Err(err) => self.failure(&err),
        }
    }

    /// One page's capture, the page named by `?url=`, percent encoded.
    fn content(&self, target: &str) -> Response {
        let Some(url) = query_url(target) else {
            return Response::error(
                Status::BadRequest,
                "name a page: /api/content?url=<address>",
            );
        };
        match content_view::served(&self.root, &url) {
            Ok(Some(served)) => match serde_json::to_vec(&served) {
                Ok(json) => Response::ok("application/json", json),
                Err(source) => self.failure(&Error::Json {
                    path: content_store::log_path(&self.root),
                    source,
                }),
            },
            Ok(None) => Response::error(Status::NotFound, "not in your library"),
            Err(err) => self.failure(&err),
        }
    }

    /// Rebuilt from disk on every request, under the lock, so a `save` that
    /// ran meanwhile and a forget in flight are both seen whole.
    fn build_library(&self) -> Result<Vec<u8>, Error> {
        let archive = Archive::open(&self.root)?;
        let _lock = archive.lock(|| {})?;
        let loaded = library::load(&archive)?;
        let state = library::State::read(&self.root)?;
        let suggested = suggestions::by_page(&suggestions::read(&self.root)?, &state);
        // Quiet: a damaged record was reported once, when `serve` started.
        let recorded = library_history::for_library(
            &self.root,
            Log {
                quiet: true,
                verbose: false,
            },
        );
        let library = library::build(
            &loaded.snapshots,
            &recorded,
            &state,
            &suggested,
            Shape::Serve,
        )
        .with_images(image_store::kept(&self.root, self.log));
        serde_json::to_vec(&library).map_err(|source| Error::Json {
            path: self.root.join(library::STATE_FILE),
            source,
        })
    }

    fn triage(&self, head: &Head, body: &[u8], action: Action) -> Response {
        let request: TriageRequest = match json_request(head, body, "{\"urls\":[...]}") {
            Ok(request) => request,
            Err(response) => return response,
        };
        match triage::apply(&self.root, &request.urls, action, false, self.log) {
            Ok(outcome) => {
                let body = serde_json::json!({
                    "urls": outcome.changed,
                    "counts": {
                        "changed": outcome.changed.len(),
                        "unchanged": outcome.unchanged.len(),
                        "unknown": outcome.unknown.len(),
                        "forgotten": outcome.forgotten,
                    },
                    // The key the stand-in server answered with, kept so the
                    // documented contract stays true.
                    action.key(): outcome.changed,
                });
                Response::ok("application/json", body.to_string().into_bytes())
            }
            Err(err) => self.failure(&err),
        }
    }

    /// Normal edits and their exact undo use the same guarded write path.
    fn tags(&self, head: &Head, body: &[u8]) -> Response {
        let expected = "{\"urls\":[...],\"add\":[...],\"remove\":[...]}";
        let request: TagsRequest = match json_request(head, body, expected) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if request.undo.is_none() && request.add.is_empty() && request.remove.is_empty() {
            return Response::error(Status::BadRequest, "name at least one tag to add or remove");
        }
        let applied = if let Some(undo) = &request.undo {
            if !request.urls.is_empty() || !request.add.is_empty() || !request.remove.is_empty() {
                return Response::error(
                    Status::BadRequest,
                    "undo cannot be combined with tag edits",
                );
            }
            tags::undo(&self.root, undo, self.log)
        } else {
            tags::apply(
                &self.root,
                &request.urls,
                &request.add,
                &request.remove,
                &[],
                false,
                self.log,
            )
        };
        match applied {
            Ok(outcome) => {
                let body = serde_json::json!({
                    "undo": outcome.undo,
                    "urls": outcome.changed,
                    "tags": outcome.tags,
                    "vocabulary": outcome.vocabulary,
                    "counts": {
                        "changed": outcome.changed.len(),
                        "unchanged": outcome.unchanged.len(),
                        "unknown": outcome.unknown.len(),
                    },
                });
                Response::ok("application/json", body.to_string().into_bytes())
            }
            Err(err) => self.refusal(&err),
        }
    }

    fn vocabulary(&self, head: &Head, body: &[u8]) -> Response {
        let expected = "{\"create\":[...],\"retire\":[...]}";
        let request: VocabularyRequest = match json_request(head, body, expected) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if request.create.is_empty() && request.retire.is_empty() {
            return Response::error(
                Status::BadRequest,
                "name at least one tag to create or retire",
            );
        }
        let edit = tags::VocabularyEdit {
            create: request.create,
            retire: request.retire,
            ..tags::VocabularyEdit::default()
        };
        match tags::edit_vocabulary(&self.root, &edit, false, self.log) {
            Ok(outcome) => {
                let body = serde_json::json!({
                    "vocabulary": outcome.vocabulary,
                    "resolved": outcome.resolved,
                });
                Response::ok("application/json", body.to_string().into_bytes())
            }
            Err(err) => self.refusal(&err),
        }
    }

    /// A tag name the rules refuse is the request's fault, not the server's.
    fn refusal(&self, err: &Error) -> Response {
        match err {
            Error::TagName { .. } => Response::error(Status::BadRequest, &err.to_string()),
            _ => self.failure(err),
        }
    }

    fn failure(&self, err: &Error) -> Response {
        self.log.warn(&err.to_string());
        Response::error(Status::InternalServerError, &err.to_string())
    }
}

/// The `url` a request target names in its query, percent decoded, as
/// `encodeURIComponent` encodes it; `None` when absent, empty or not UTF-8.
fn query_url(target: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("url="))?;
    let url = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()?;
    (!url.is_empty()).then(|| url.into_owned())
}

/// A request head the wire could not frame.
fn unread_head(framing: Framing) -> Response {
    match framing {
        Framing::TooLarge => Response::error(
            Status::RequestHeaderFieldsTooLarge,
            "request head too large",
        ),
        _ => Response::error(Status::BadRequest, "incomplete request"),
    }
}

/// A request body the wire could not frame.
fn unread_body(framing: Framing) -> Response {
    match framing {
        Framing::Chunked => Response::error(
            Status::LengthRequired,
            "send a Content-Length, not a transfer encoding",
        ),
        Framing::BadLength => Response::error(Status::BadRequest, "bad Content-Length"),
        Framing::TooLarge => Response::error(Status::ContentTooLarge, "request body too large"),
        Framing::Overrun => Response::error(Status::BadRequest, "body longer than Content-Length"),
        Framing::Closed | Framing::Incomplete => {
            Response::error(Status::BadRequest, "incomplete body")
        }
    }
}

#[derive(Deserialize)]
struct TriageRequest {
    urls: Vec<String>,
}

#[derive(Deserialize)]
struct TagsRequest {
    #[serde(default)]
    undo: Option<tags::Undo>,
    urls: Vec<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Deserialize)]
struct VocabularyRequest {
    #[serde(default)]
    create: Vec<String>,
    #[serde(default)]
    retire: Vec<String>,
}
