//! The `DevTools` protocol, as far as knowmoretabs speaks it: a closed set of
//! commands, their replies and the one event read, over a WebSocket on
//! loopback with a deadline on every wait.
//!
//! slice: content
//! why: A headless browser is driven through its `DevTools` socket, which
//!      can do anything the browser can. knowmoretabs needs ten things of
//!      it: open and close a private context and a tab, load one address,
//!      hear when it has loaded, read the page back by one fixed expression,
//!      ask the version, and close the browser. Each is a variant here with
//!      a fixed method and shape, so what is ever sent can be read in one
//!      place and is tested to the byte. Every wait has a deadline, and a
//!      message is capped at what the largest page knowmoretabs keeps can
//!      take on the wire, so a browser that hangs or floods cannot stall a
//!      run or exhaust its memory.
#![allow(dead_code, reason = "the headless tier drives the browser in step 6")]

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Instant;

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

/// Everything ever sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    CreateBrowserContext,
    /// A blank tab in `context`.
    CreateTarget {
        context: String,
    },
    CloseTarget {
        target: String,
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
            Self::CreateTarget { context } => {
                json!({"url": "about:blank", "browserContextId": context})
            }
            Self::CloseTarget { target } => json!({"targetId": target}),
            Self::DisposeBrowserContext { context } => json!({"browserContextId": context}),
            Self::LifecycleEvents => json!({"enabled": true}),
            Self::Navigate { url } => json!({"url": url}),
            Self::Extract => json!({"expression": EXTRACT, "returnByValue": true}),
        }
    }

    /// The message that sends it as call number `id`.
    pub fn encode(&self, id: u64) -> String {
        json!({"id": id, "method": self.method(), "params": self.params()}).to_string()
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
    Lifecycle(Lifecycle),
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
        return Incoming::Lifecycle(Lifecycle {
            frame: field("frameId"),
            loader: field("loaderId"),
            name: field("name"),
        });
    }
    Incoming::Other
}

/// One `DevTools` WebSocket: the browser's, or one tab's.
#[derive(Debug)]
pub struct Socket {
    ws: WebSocket<TcpStream>,
    next: u64,
    /// Lifecycle events read while waiting for a reply.
    events: VecDeque<Lifecycle>,
}

impl Socket {
    /// Connects to `path` on the browser's loopback `port`.
    pub fn connect(port: u16, path: &str, deadline: Instant) -> Result<Self, String> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&address, remaining(deadline)?)
            .map_err(|err| reason(&err))?;
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|err| reason(&err))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (ws, _) = tungstenite::client::client_with_config(
            format!("ws://{address}{path}"),
            stream,
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

    /// Sends `command` and waits for its reply until `deadline`.
    pub fn call(&mut self, command: &Command, deadline: Instant) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        self.ws
            .send(Message::text(command.encode(id)))
            .map_err(|err| failure(&err))?;
        loop {
            match self.read(deadline)? {
                Incoming::Reply {
                    id: answered,
                    result,
                } if answered == id => {
                    return result.map_err(|err| format!("{}: {err}", command.method()));
                }
                Incoming::Lifecycle(event) => self.events.push_back(event),
                Incoming::Reply { .. } | Incoming::Other => {}
            }
        }
    }

    /// The next lifecycle event, or `None` once `deadline` has passed.
    pub fn event(&mut self, deadline: Instant) -> Result<Option<Lifecycle>, String> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        loop {
            match self.read(deadline) {
                Ok(Incoming::Lifecycle(event)) => return Ok(Some(event)),
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
        let wait = remaining(deadline)?;
        self.ws
            .get_ref()
            .set_read_timeout(Some(wait))
            .map_err(|err| reason(&err))?;
        match self.ws.read().map_err(|err| failure(&err))? {
            Message::Text(text) => Ok(decode(text.as_str())),
            _ => Ok(Incoming::Other),
        }
    }
}

/// What is left until `deadline`, or a timeout when nothing is.
fn remaining(deadline: Instant) -> Result<std::time::Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("timeout".to_owned())
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
mod tests {
    use super::*;

    #[test]
    fn every_command_encodes_to_its_exact_message() {
        let cases = [
            (
                Command::CreateBrowserContext,
                r#"{"id":1,"method":"Target.createBrowserContext","params":{}}"#,
            ),
            (
                Command::CreateTarget {
                    context: "C".to_owned(),
                },
                r#"{"id":1,"method":"Target.createTarget","params":{"browserContextId":"C","url":"about:blank"}}"#,
            ),
            (
                Command::CloseTarget {
                    target: "T".to_owned(),
                },
                r#"{"id":1,"method":"Target.closeTarget","params":{"targetId":"T"}}"#,
            ),
            (
                Command::DisposeBrowserContext {
                    context: "C".to_owned(),
                },
                r#"{"id":1,"method":"Target.disposeBrowserContext","params":{"browserContextId":"C"}}"#,
            ),
            (
                Command::GetVersion,
                r#"{"id":1,"method":"Browser.getVersion","params":{}}"#,
            ),
            (
                Command::Close,
                r#"{"id":1,"method":"Browser.close","params":{}}"#,
            ),
            (
                Command::PageEnable,
                r#"{"id":1,"method":"Page.enable","params":{}}"#,
            ),
            (
                Command::LifecycleEvents,
                r#"{"id":1,"method":"Page.setLifecycleEventsEnabled","params":{"enabled":true}}"#,
            ),
            (
                Command::Navigate {
                    url: "https://a.test/".to_owned(),
                },
                r#"{"id":1,"method":"Page.navigate","params":{"url":"https://a.test/"}}"#,
            ),
        ];
        for (command, message) in cases {
            assert_eq!(command.encode(1), message);
        }
        let extract: Value = serde_json::from_str(&Command::Extract.encode(7)).unwrap();
        assert_eq!(
            extract,
            json!({"id": 7, "method": "Runtime.evaluate",
                "params": {"expression": EXTRACT, "returnByValue": true}})
        );
    }

    #[test]
    fn the_expression_caps_html_at_the_body_cap() {
        assert!(EXTRACT.contains(&format!("html.length > {BODY_CAP} ? null")));
        assert!(!EXTRACT.contains("JSON.stringify"));
    }

    #[test]
    fn replies_errors_and_lifecycle_events_decode_and_the_rest_is_ignored() {
        assert_eq!(
            decode(r#"{"id":3,"result":{"targetId":"T"}}"#),
            Incoming::Reply {
                id: 3,
                result: Ok(json!({"targetId": "T"}))
            }
        );
        assert_eq!(
            decode(r#"{"id":4,"error":{"code":-32000,"message":"No target"}}"#),
            Incoming::Reply {
                id: 4,
                result: Err("No target".to_owned())
            }
        );
        assert_eq!(
            decode(
                r#"{"method":"Page.lifecycleEvent","params":{"frameId":"F","loaderId":"L","name":"load","timestamp":1.5}}"#
            ),
            Incoming::Lifecycle(Lifecycle {
                frame: "F".to_owned(),
                loader: "L".to_owned(),
                name: "load".to_owned()
            })
        );
        for other in [
            r#"{"method":"Page.frameNavigated","params":{}}"#,
            r#"{"method":"Target.targetCreated"}"#,
            "not json",
            "[]",
        ] {
            assert_eq!(decode(other), Incoming::Other, "{other}");
        }
    }
}
