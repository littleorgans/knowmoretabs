//! The `DevTools` protocol, as far as knowmoretabs speaks it: a closed set of
//! commands, their replies and the one event read, over a WebSocket on
//! loopback with a deadline on every wait.
//!
//! slice: content
//! why: A browser is driven through its `DevTools` socket, which can do
//!      anything the browser can. knowmoretabs needs eleven things of it:
//!      open and close a private context and a tab, attach to a tab it
//!      opened, load one address, hear when it has loaded, read the page
//!      back by one fixed expression, ask the version, and close the
//!      browser. Each is a variant here with a fixed method and shape, so
//!      what is ever sent can be read in one place and is tested to the
//!      byte. A tab and a session are values only the reply that made them
//!      can produce, so a command names no tab or session but one this run
//!      created, and a session's events are heard by it alone. Every wait
//!      has a deadline, and a message is capped at what the largest page
//!      knowmoretabs keeps can take on the wire, so a browser that hangs or
//!      floods cannot stall a run or exhaust its memory.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::content_fetch::BODY_CAP;

/// What a rendered page is read with, by value: where it ended, its media
/// type, the status of its navigation, and its HTML unless that is longer
/// than [`BODY_CAP`] UTF-16 units, which UTF-8 bytes never undercount. The
/// number is [`BODY_CAP`]; a test holds them equal.
pub const EXTRACT: &str = "(() => {
  const nav = performance.getEntriesByType('navigation')[0];
  const root = document.documentElement;
  const html = root ? root.outerHTML : '';
  return {
    url: location.href,
    mime: document.contentType,
    status: nav ? nav.responseStatus : 0,
    html: html.length > 10485760 ? null : html,
  };
})()";

/// The largest message read: JSON spends at most six bytes on one UTF-16
/// unit, so every reply the expression lets through fits, with room for
/// the rest of the reply.
const MAX_MESSAGE: usize = 6 * BODY_CAP + 1024 * 1024;

/// A tab this run created: made only from the reply that created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(String);

impl Target {
    /// The tab a `Target.createTarget` reply names.
    pub fn of(reply: &Value) -> Option<Self> {
        named(reply, "targetId").map(Self)
    }

    /// Where the tab's own socket is.
    pub fn route(&self) -> String {
        format!("/devtools/page/{}", self.0)
    }
}

/// A session this run attached to a tab it created: made only from the
/// reply that attached it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session(String);

impl Session {
    /// The session a `Target.attachToTarget` reply names.
    pub fn of(reply: &Value) -> Option<Self> {
        named(reply, "sessionId").map(Self)
    }
}

/// A reply's string field, when it is not empty.
fn named(reply: &Value, key: &str) -> Option<String> {
    reply[key]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Everything ever sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    CreateBrowserContext,
    /// A blank tab, in `context` when there is one; `hidden` keeps it out
    /// of the tab strip and ends it with the connection that made it.
    CreateTarget {
        context: Option<String>,
        hidden: bool,
    },
    CloseTarget {
        target: Target,
    },
    /// A session for `target` on the socket that sends it.
    AttachToTarget {
        target: Target,
    },
    DisposeBrowserContext {
        context: String,
    },
    GetVersion,
    Close,
    PageEnable,
    LifecycleEvents,
    Navigate {
        url: String,
    },
    /// [`EXTRACT`], returned by value.
    Extract,
}

impl Command {
    fn method(&self) -> &'static str {
        match self {
            Self::CreateBrowserContext => "Target.createBrowserContext",
            Self::CreateTarget { .. } => "Target.createTarget",
            Self::CloseTarget { .. } => "Target.closeTarget",
            Self::AttachToTarget { .. } => "Target.attachToTarget",
            Self::DisposeBrowserContext { .. } => "Target.disposeBrowserContext",
            Self::GetVersion => "Browser.getVersion",
            Self::Close => "Browser.close",
            Self::PageEnable => "Page.enable",
            Self::LifecycleEvents => "Page.setLifecycleEventsEnabled",
            Self::Navigate { .. } => "Page.navigate",
            Self::Extract => "Runtime.evaluate",
        }
    }

    fn params(&self) -> Value {
        match self {
            Self::CreateBrowserContext | Self::GetVersion | Self::Close | Self::PageEnable => {
                json!({})
            }
            Self::CreateTarget { context, hidden } => {
                let mut params = json!({"url": "about:blank"});
                if let Some(context) = context {
                    params["browserContextId"] = json!(context);
                }
                if *hidden {
                    params["hidden"] = json!(true);
                    params["background"] = json!(true);
                }
                params
            }
            Self::CloseTarget { target } => json!({"targetId": target.0}),
            Self::AttachToTarget { target } => json!({"targetId": target.0, "flatten": true}),
            Self::DisposeBrowserContext { context } => json!({"browserContextId": context}),
            Self::LifecycleEvents => json!({"enabled": true}),
            Self::Navigate { url } => json!({"url": url}),
            Self::Extract => json!({"expression": EXTRACT, "returnByValue": true}),
        }
    }

    /// The message that sends it as call number `id`, to `session`'s tab
    /// when there is one.
    pub fn encode(&self, id: u64, session: Option<&Session>) -> String {
        let mut message = json!({"id": id, "method": self.method(), "params": self.params()});
        if let Some(session) = session {
            message["sessionId"] = json!(session.0);
        }
        message.to_string()
    }
}

/// A `Page.lifecycleEvent`: a frame's document reached `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lifecycle {
    pub frame: String,
    pub loader: String,
    pub name: String,
}

/// A message from the browser, as far as it matters here.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// The answer to call `id`: its result, or the browser's error message.
    Reply {
        id: u64,
        result: Result<Value, String>,
    },
    /// A lifecycle event, and the session it came from when it came over
    /// a browser's socket.
    Lifecycle {
        session: Option<String>,
        event: Lifecycle,
    },
    /// Any other event or message, ignored.
    Other,
}

pub fn decode(text: &str) -> Incoming {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Incoming::Other;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let result = match value.get("error") {
            Some(error) => Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .chars()
                .take(160)
                .collect()),
            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
        };
        return Incoming::Reply { id, result };
    }
    if value.get("method").and_then(Value::as_str) == Some("Page.lifecycleEvent") {
        let field = |key: &str| {
            value["params"][key]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default()
        };
        return Incoming::Lifecycle {
            session: value["sessionId"].as_str().map(str::to_owned),
            event: Lifecycle {
                frame: field("frameId"),
                loader: field("loaderId"),
                name: field("name"),
            },
        };
    }
    Incoming::Other
}

/// One `DevTools` WebSocket: the browser's, or one tab's.
#[derive(Debug)]
pub struct Socket {
    ws: WebSocket<Bounded>,
    next: u64,
    /// Lifecycle events read while waiting for a reply, each with the
    /// session it came from.
    events: VecDeque<(Option<String>, Lifecycle)>,
}

/// Reaches the browser's loopback `port`: the part of connecting that
/// fails when nothing listens there.
pub fn reach(port: u16, deadline: Instant) -> Result<TcpStream, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    remaining(deadline)
        .and_then(|left| TcpStream::connect_timeout(&address, left))
        .map_err(|err| reason(&err))
}

impl Socket {
    /// Connects to `path` on the browser's loopback `port`.
    pub fn connect(port: u16, path: &str, deadline: Instant) -> Result<Self, String> {
        Self::over(reach(port, deadline)?, path, deadline)
    }

    /// Opens the WebSocket at `path` over `stream`: the part of connecting
    /// a browser may ask its owner about first.
    pub fn over(stream: TcpStream, path: &str, deadline: Instant) -> Result<Self, String> {
        let address = stream.peer_addr().map_err(|err| reason(&err))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (ws, _) = tungstenite::client::client_with_config(
            format!("ws://{address}{path}"),
            Bounded { stream, deadline },
            Some(config),
        )
        .map_err(|err| match err {
            tungstenite::HandshakeError::Failure(err) => failure(&err),
            tungstenite::HandshakeError::Interrupted(_) => "timeout".to_owned(),
        })?;
        Ok(Self {
            ws,
            next: 1,
            events: VecDeque::new(),
        })
    }

    /// Sends `command`, to `session`'s tab when there is one, and waits
    /// for its reply until `deadline`, keeping that session's events only.
    pub fn call(
        &mut self,
        command: &Command,
        session: Option<&Session>,
        deadline: Instant,
    ) -> Result<Value, String> {
        remaining(deadline).map_err(|err| reason(&err))?;
        let id = self.next;
        self.next += 1;
        self.ws.get_mut().deadline = deadline;
        self.ws
            .send(Message::text(command.encode(id, session)))
            .map_err(|err| failure(&err))?;
        loop {
            match self.read(deadline)? {
                Incoming::Reply {
                    id: answered,
                    result,
                } if answered == id => {
                    return result.map_err(|err| format!("{}: {err}", command.method()));
                }
                Incoming::Lifecycle {
                    session: from,
                    event,
                } if is(from.as_deref(), session) => {
                    self.events.push_back((from, event));
                }
                Incoming::Reply { .. } | Incoming::Lifecycle { .. } | Incoming::Other => {}
            }
        }
    }

    /// `session`'s next lifecycle event, or `None` once `deadline` has
    /// passed. Another session's events are ignored.
    pub fn event(
        &mut self,
        session: Option<&Session>,
        deadline: Instant,
    ) -> Result<Option<Lifecycle>, String> {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        while let Some((from, event)) = self.events.pop_front() {
            if is(from.as_deref(), session) {
                return Ok(Some(event));
            }
        }
        loop {
            match self.read(deadline) {
                Ok(Incoming::Lifecycle {
                    session: from,
                    event,
                }) if is(from.as_deref(), session) => {
                    return Ok(Some(event));
                }
                Ok(_) => {}
                Err(why) if why == "timeout" => return Ok(None),
                Err(why) => return Err(why),
            }
        }
    }

    /// Drops the lifecycle events read so far: they were another
    /// document's.
    pub fn forget_events(&mut self) {
        self.events.clear();
    }

    fn read(&mut self, deadline: Instant) -> Result<Incoming, String> {
        remaining(deadline).map_err(|err| reason(&err))?;
        self.ws.get_mut().deadline = deadline;
        match self.ws.read().map_err(|err| failure(&err))? {
            Message::Text(text) => Ok(decode(text.as_str())),
            _ => Ok(Incoming::Other),
        }
    }
}

/// Whether an event from session `from` is `session`'s: on a tab's own
/// socket, neither has one.
fn is(from: Option<&str>, session: Option<&Session>) -> bool {
    from == session.map(|session| session.0.as_str())
}

/// The loopback stream, each read and write of which waits only what is
/// left until `deadline`: a frame that trickles in a byte at a time cannot
/// stretch a wait, and the WebSocket keeps a partial frame for the next.
#[derive(Debug)]
struct Bounded {
    stream: TcpStream,
    deadline: Instant,
}

impl Read for Bounded {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream
            .set_read_timeout(Some(remaining(self.deadline)?))?;
        self.stream.read(buf)
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream
            .set_write_timeout(Some(remaining(self.deadline)?))?;
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// What is left until `deadline`, or a timeout when nothing is.
fn remaining(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        Ok(left)
    }
}

fn failure(err: &tungstenite::Error) -> String {
    match err {
        tungstenite::Error::Io(err) => reason(err),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            "the browser closed the connection".to_owned()
        }
        tungstenite::Error::Capacity(_) => "a message from the browser was too large".to_owned(),
        other => other.to_string().chars().take(160).collect(),
    }
}

fn reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "timeout".to_owned(),
        kind => format!("the browser connection failed: {kind}"),
    }
}

#[cfg(test)]
#[path = "cdp_tests.rs"]
mod tests;
