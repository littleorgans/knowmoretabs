//! Regression checks for `cdp`.
//!
//! slice: content
//! why: A loopback WebSocket peer and the commands' exact bytes settle what
//!      is ever sent, which session hears which event, and that every wait
//!      ends at its deadline, without a browser.

use std::net::TcpListener;

use tungstenite::protocol::Role;
use tungstenite::protocol::frame::Frame;
use tungstenite::protocol::frame::coding::{Data, OpCode};

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
                context: Some("C".to_owned()),
                hidden: false,
            },
            r#"{"id":1,"method":"Target.createTarget","params":{"browserContextId":"C","url":"about:blank"}}"#,
        ),
        (
            Command::CreateTarget {
                context: None,
                hidden: true,
            },
            r#"{"id":1,"method":"Target.createTarget","params":{"background":true,"hidden":true,"url":"about:blank"}}"#,
        ),
        (
            Command::CloseTarget {
                target: Target("T".to_owned()),
            },
            r#"{"id":1,"method":"Target.closeTarget","params":{"targetId":"T"}}"#,
        ),
        (
            Command::AttachToTarget {
                target: Target("T".to_owned()),
            },
            r#"{"id":1,"method":"Target.attachToTarget","params":{"flatten":true,"targetId":"T"}}"#,
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
        assert_eq!(command.encode(1, None), message);
    }
    assert_eq!(
        Command::PageEnable.encode(2, Some(&Session("S".to_owned()))),
        r#"{"id":2,"method":"Page.enable","params":{},"sessionId":"S"}"#
    );
    let extract: Value = serde_json::from_str(&Command::Extract.encode(7, None)).unwrap();
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
        Incoming::Lifecycle {
            session: None,
            event: Lifecycle {
                frame: "F".to_owned(),
                loader: "L".to_owned(),
                name: "load".to_owned()
            }
        }
    );
    assert_eq!(
        decode(
            r#"{"method":"Page.lifecycleEvent","params":{"frameId":"F","loaderId":"L","name":"init"},"sessionId":"S"}"#
        ),
        Incoming::Lifecycle {
            session: Some("S".to_owned()),
            event: Lifecycle {
                frame: "F".to_owned(),
                loader: "L".to_owned(),
                name: "init".to_owned()
            }
        }
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

#[test]
fn a_reply_that_trickles_in_ends_at_the_deadline() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        ws.read().unwrap();
        ws.send(Message::text(r#"{"id":1,"result":{"product":"P"}}"#))
            .unwrap();
        ws.read().unwrap();
        // The head of a 200 byte text frame, then a byte every 20 ms.
        let stream = ws.get_mut();
        stream.write_all(&[0x81, 126, 0, 200]).unwrap();
        for _ in 0..200 {
            if stream.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut socket = Socket::connect(port, "/devtools/browser/B", deadline).unwrap();
    assert_eq!(
        socket.call(&Command::GetVersion, None, deadline),
        Ok(json!({"product": "P"}))
    );
    let start = Instant::now();
    let reply = socket.call(
        &Command::GetVersion,
        None,
        start + Duration::from_millis(200),
    );
    assert_eq!(reply, Err("timeout".to_owned()));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_buffered_event_is_not_read_after_the_deadline() {
    let event =
        r#"{"method":"Page.lifecycleEvent","params":{"frameId":"F","loaderId":"L","name":"load"}}"#;
    let mut frame = Vec::new();
    Frame::message(event.as_bytes().to_vec(), OpCode::Data(Data::Text), true)
        .format(&mut frame)
        .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let _peer = listener.accept().unwrap();
    let deadline = Instant::now();
    // The whole frame is already in the WebSocket's buffer.
    let ws =
        WebSocket::from_partially_read(Bounded { stream, deadline }, frame, Role::Client, None);
    let mut socket = Socket {
        ws,
        next: 1,
        events: VecDeque::new(),
    };
    assert_eq!(socket.read(deadline), Err("timeout".to_owned()));
}

#[test]
fn a_call_past_its_deadline_is_never_sent() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let Message::Text(text) = ws.read().unwrap() else {
            panic!("expected a command");
        };
        let command: Value = serde_json::from_str(text.as_str()).unwrap();
        ws.send(Message::text(
            json!({"id": command["id"], "result": {}}).to_string(),
        ))
        .unwrap();
        command["method"].as_str().unwrap().to_owned()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut socket = Socket::connect(port, "/devtools/browser/B", deadline).unwrap();
    let navigate = Command::Navigate {
        url: "https://a.test/".to_owned(),
    };
    assert_eq!(
        socket.call(&navigate, None, Instant::now()),
        Err("timeout".to_owned())
    );
    assert_eq!(
        socket.call(&Command::GetVersion, None, deadline),
        Ok(json!({}))
    );
    assert_eq!(server.join().unwrap(), "Browser.getVersion");
}

#[test]
fn tabs_and_sessions_are_made_only_from_the_replies_that_name_them() {
    let target = Target::of(&json!({"targetId": "T"})).unwrap();
    assert_eq!(target.route(), "/devtools/page/T");
    assert_eq!(
        Session::of(&json!({"sessionId": "S"})),
        Some(Session("S".to_owned()))
    );
    for reply in [json!({}), json!({"targetId": ""}), json!({"targetId": 7})] {
        assert_eq!(Target::of(&reply), None, "{reply}");
    }
    assert_eq!(Session::of(&json!({"targetId": "T"})), None);
}

#[test]
fn a_session_hears_its_own_events_and_never_another_sessions() {
    let event = |session: &str, name: &str| {
        json!({"method": "Page.lifecycleEvent", "sessionId": session,
            "params": {"frameId": "F", "loaderId": "L", "name": name}})
        .to_string()
    };
    let (other, ours) = (event("OTHER", "load"), event("S", "load"));
    let (other_idle, our_idle) = (event("OTHER", "networkIdle"), event("S", "networkIdle"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let Message::Text(text) = ws.read().unwrap() else {
            panic!("expected a command");
        };
        let command: Value = serde_json::from_str(text.as_str()).unwrap();
        // Events of both sessions while the call waits, then its reply,
        // then more of both.
        for message in [
            other,
            ours,
            json!({"id": command["id"], "result": {}}).to_string(),
        ] {
            ws.send(Message::text(message)).unwrap();
        }
        for message in [other_idle, our_idle] {
            ws.send(Message::text(message)).unwrap();
        }
        // Open until the client is done.
        let _ = ws.read();
        command
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut socket = Socket::connect(port, "/devtools/browser/B", deadline).unwrap();
    let session = Session("S".to_owned());
    socket
        .call(&Command::PageEnable, Some(&session), deadline)
        .unwrap();
    let heard: Vec<String> = (0..2)
        .map(|_| {
            socket
                .event(Some(&session), deadline)
                .unwrap()
                .unwrap()
                .name
        })
        .collect();
    assert_eq!(heard, ["load", "networkIdle"]);
    let quick = Instant::now() + Duration::from_millis(100);
    assert_eq!(socket.event(Some(&session), quick), Ok(None));
    drop(socket);
    assert_eq!(server.join().unwrap()["sessionId"], "S");
}
