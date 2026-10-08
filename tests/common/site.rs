//! A loopback HTTP server that stands in for every site a test reaches:
//! a debug build resolves every name to it under
//! `KNOWMORETABS_TEST_RESOLVE`, so nothing reaches the internet.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub at: Instant,
    pub host: String,
    pub path: String,
    pub headers: HashMap<String, String>,
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering at all.
    pub stall: Option<Duration>,
}

impl Reply {
    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type", "text/html; charset=utf-8".into())],
            body: body.into(),
            stall: None,
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self {
            status,
            headers: vec![
                ("Location", location.to_owned()),
                ("Set-Cookie", "session=abc; Path=/".to_owned()),
            ],
            body: Vec::new(),
            stall: None,
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/html".into())],
            body: b"<title>nope</title>".to_vec(),
            stall: None,
        }
    }
}

pub type Route = dyn Fn(&str, &str, u16) -> Reply + Send + Sync;

/// A server on an ephemeral loopback port that answers for every host name,
/// one request per connection, and remembers each request.
pub struct Site {
    pub address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Site {
    pub fn start(route: impl Fn(&str, &str, u16) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let route: Arc<Route> = Arc::new(route);
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (route, log) = (Arc::clone(&route), Arc::clone(&log));
                std::thread::spawn(move || serve(stream, &*route, &log, address.port()));
            }
        });
        Self { address, seen }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.seen()
            .iter()
            .map(|s| format!("{}{}", s.host, s.path))
            .collect()
    }
}

fn serve(stream: TcpStream, route: &Route, log: &Mutex<Vec<Seen>>, port: u16) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let host = headers
        .get("host")
        .map(|h| h.split(':').next().unwrap_or("").to_owned())
        .unwrap_or_default();
    log.lock().unwrap().push(Seen {
        at: Instant::now(),
        host: host.clone(),
        path: path.clone(),
        headers,
    });
    let reply = route(&host, &path, port);
    if let Some(stall) = reply.stall {
        std::thread::sleep(stall);
    }
    let mut out = stream;
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    // The client may stop reading once it has the head; that is the point.
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&reply.body);
}
