//! The loopback SOCKS5 relay a headless browser must send every connection
//! through, each one opened by `fetch`'s public only rule.
//!
//! slice: content
//! why: A page rendered in a browser runs its own scripts, and those can
//!      reach for this machine or the private network by every route a
//!      browser has: fetches, frames, workers, sockets, redirects and script
//!      navigations, by address or by a public name that resolves inward.
//!      Judging URLs inside the browser missed some of them; judging the
//!      connection does not. The browser is told to use this relay for
//!      everything, loopback included, and the relay opens each connection
//!      through `fetch::connect_public`, the same rule plain fetching keeps,
//!      checking the address it connects to. Refusals are counted, so a run
//!      can say what it kept home. Stopping it closes every tunnel still
//!      open, so nothing outlives the browser it served.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::fetch::{self, Refusal};

/// How long the browser may take to say where it wants to go.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections open at once; one past this is dropped unanswered.
const MAX_LIVE: usize = 64;
/// How long stopping waits for the tunnels it closed to end.
const STOP_WAIT: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(20);

/// SOCKS5 reply codes.
const SUCCEEDED: u8 = 0;
const NOT_ALLOWED: u8 = 2;
const REFUSED: u8 = 5;
const COMMAND_NOT_SUPPORTED: u8 = 7;
const ADDRESS_NOT_SUPPORTED: u8 = 8;

/// A running relay on `127.0.0.1`. Dropping it stops it.
#[derive(Debug)]
pub struct Relay {
    port: u16,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

#[derive(Debug, Default)]
struct Shared {
    test_address: Option<SocketAddr>,
    stopping: AtomicBool,
    /// Connections refused for this machine or the private network.
    refused: AtomicUsize,
    next: AtomicU64,
    /// The sockets of every open connection, so stopping can close them.
    live: Mutex<HashMap<u64, Vec<TcpStream>>>,
}

impl Relay {
    /// Listens on a free loopback port. Debug builds honour the fetcher's
    /// test address, so a local check can stand a server in for every host.
    pub fn start() -> io::Result<Self> {
        Self::with(fetch::test_address())
    }

    fn with(test_address: Option<SocketAddr>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            test_address,
            ..Shared::default()
        });
        let accepting = Arc::clone(&shared);
        let accept = thread::Builder::new()
            .name("socks-accept".to_owned())
            .spawn(move || accept(&listener, &accepting))?;
        Ok(Self {
            port,
            shared,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Connections refused for this machine or the private network, by
    /// name or by address.
    pub fn refused(&self) -> usize {
        self.shared.refused.load(Ordering::Relaxed)
    }

    /// Stops accepting and closes every open connection; how many were
    /// still open [`STOP_WAIT`] later. A second call only counts again.
    /// On Windows a shutdown refuses later reads but does not wake one
    /// already waiting, so a tunnel ends there only once a peer answers the
    /// close, and until then it is counted.
    pub fn stop(&mut self) -> usize {
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            // One connection wakes the accept loop to see it should end.
            let wake = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
            if TcpStream::connect_timeout(&wake, STOP_WAIT).is_ok() {
                let _ = accept.join();
            }
        }
        for socket in self.shared.live().values().flatten() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        let deadline = Instant::now() + STOP_WAIT;
        loop {
            let open = self.shared.live().len();
            if open == 0 || Instant::now() >= deadline {
                return open;
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    fn live(&self) -> MutexGuard<'_, HashMap<u64, Vec<TcpStream>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers `socket` under connection `id`, its first when `id` is
    /// new; false when the relay is stopping or, for a new connection, full.
    fn hold(&self, id: u64, socket: &TcpStream) -> bool {
        let mut live = self.live();
        if self.stopping.load(Ordering::SeqCst)
            || (!live.contains_key(&id) && live.len() >= MAX_LIVE)
        {
            return false;
        }
        match socket.try_clone() {
            Ok(clone) => {
                live.entry(id).or_default().push(clone);
                true
            }
            Err(_) => false,
        }
    }

    /// Opens the connection asked for, or says why not with a reply code.
    fn connect(&self, host: &str, port: u16) -> Result<TcpStream, u8> {
        let deadline = Instant::now() + fetch::TIMEOUT;
        match fetch::connect_public(host, port, deadline, self.test_address) {
            Ok(stream) => Ok(stream),
            Err(Refusal::Failed(_)) => Err(REFUSED),
            Err(_) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                Err(NOT_ALLOWED)
            }
        }
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    for client in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        let Ok(client) = client else { continue };
        let id = shared.next.fetch_add(1, Ordering::Relaxed);
        if !shared.hold(id, &client) {
            continue;
        }
        let serving = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name("socks".to_owned())
            .spawn(move || {
                let _ = serve(&serving, id, client);
                serving.live().remove(&id);
            });
        if spawned.is_err() {
            shared.live().remove(&id);
        }
    }
}

/// One connection: the handshake, the decision, then the tunnel.
fn serve(shared: &Shared, id: u64, mut client: TcpStream) -> io::Result<()> {
    client.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    if !greet(&mut client)? {
        return Ok(());
    }
    let (host, port) = match read_request(&mut client)? {
        Ok(destination) => destination,
        Err(code) => return client.write_all(&reply(code)),
    };
    let upstream = match shared.connect(&host, port) {
        Ok(upstream) => upstream,
        Err(code) => return client.write_all(&reply(code)),
    };
    if !shared.hold(id, &upstream) {
        return Ok(());
    }
    client.write_all(&reply(SUCCEEDED))?;
    client.set_read_timeout(None)?;
    tunnel(&client, &upstream)
}

/// Reads the greeting and accepts it when it offers no authentication.
fn greet(client: &mut (impl Read + Write)) -> io::Result<bool> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head)?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods)?;
    if head[0] != 5 {
        return Ok(false);
    }
    let accepted = methods.contains(&0);
    client.write_all(&[5, if accepted { 0 } else { 0xff }])?;
    Ok(accepted)
}

/// The destination of a `CONNECT` request, as a host and a port; or the
/// reply code for a request this relay does not serve.
fn read_request(client: &mut impl Read) -> io::Result<Result<(String, u16), u8>> {
    let mut head = [0u8; 4];
    client.read_exact(&mut head)?;
    let [version, command, _, kind] = head;
    if version != 5 || command != 1 {
        return Ok(Err(COMMAND_NOT_SUPPORTED));
    }
    let host = match kind {
        1 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets)?;
            Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len)?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name)?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets)?;
            Ipv6Addr::from(octets).to_string()
        }
        _ => return Ok(Err(ADDRESS_NOT_SUPPORTED)),
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port)?;
    Ok(Ok((host, u16::from_be_bytes(port))))
}

/// A reply with `code` and no bound address.
fn reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// Copies both ways until either side ends. The end of the browser's side
/// means it is done with the connection (a browser does not half close),
/// so it ends the site's side too and frees the slot; the end of the
/// site's side is passed on as the end of what the browser reads.
fn tunnel(client: &TcpStream, upstream: &TcpStream) -> io::Result<()> {
    let (from_browser, to_site) = (client.try_clone()?, upstream.try_clone()?);
    let outbound = thread::Builder::new()
        .name("socks-copy".to_owned())
        .spawn(move || {
            let _ = io::copy(&mut &from_browser, &mut &to_site);
            let _ = to_site.shutdown(Shutdown::Both);
        })?;
    let _ = io::copy(&mut &*upstream, &mut &*client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = outbound.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client that has greeted `relay` and asked for `destination`, and
    /// the reply code it got.
    fn ask(relay: &Relay, destination: &[u8]) -> (TcpStream, u8) {
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, relay.port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        client.write_all(&[5, 1, 0]).unwrap();
        let mut chosen = [0u8; 2];
        client.read_exact(&mut chosen).unwrap();
        assert_eq!(chosen, [5, 0]);
        client.write_all(&[5, 1, 0]).unwrap();
        client.write_all(destination).unwrap();
        let mut answer = [0u8; 10];
        client.read_exact(&mut answer).unwrap();
        (client, answer[1])
    }

    fn name(host: &str, port: u16) -> Vec<u8> {
        let mut bytes = vec![3, u8::try_from(host.len()).unwrap()];
        bytes.extend_from_slice(host.as_bytes());
        bytes.extend_from_slice(&port.to_be_bytes());
        bytes
    }

    fn wait_until(done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
        true
    }

    #[test]
    fn requests_name_their_destination_by_any_address_type() {
        let parse = |bytes: &[u8]| read_request(&mut &bytes[..]).unwrap();
        assert_eq!(
            parse(&[5, 1, 0, 1, 93, 184, 216, 34, 0, 80]),
            Ok(("93.184.216.34".to_owned(), 80))
        );
        let mut by_name = vec![5, 1, 0];
        by_name.extend(name("example.test", 443));
        assert_eq!(parse(&by_name), Ok(("example.test".to_owned(), 443)));
        let mut v6 = vec![5, 1, 0, 4];
        v6.extend(Ipv6Addr::LOCALHOST.octets());
        v6.extend(8080u16.to_be_bytes());
        assert_eq!(parse(&v6), Ok(("::1".to_owned(), 8080)));
        assert_eq!(parse(&[5, 2, 0, 1]), Err(COMMAND_NOT_SUPPORTED), "BIND");
        assert_eq!(parse(&[5, 1, 0, 9]), Err(ADDRESS_NOT_SUPPORTED));
        assert_eq!(reply(SUCCEEDED), [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn this_machine_and_the_private_network_are_refused_and_counted() {
        let relay = Relay::with(None).unwrap();
        let mut v6 = vec![4];
        v6.extend(Ipv6Addr::LOCALHOST.octets());
        v6.extend(80u16.to_be_bytes());
        for destination in [
            vec![1, 127, 0, 0, 1, 0, 80],
            vec![1, 192, 168, 1, 1, 0, 80],
            name("localhost", 80),
            name("127.0.0.1", 80),
            name("printer.local", 80),
            v6,
        ] {
            assert_eq!(ask(&relay, &destination).1, NOT_ALLOWED, "{destination:?}");
        }
        assert_eq!(relay.refused(), 6);
    }

    #[test]
    fn a_public_destination_is_tunnelled_and_a_failed_connect_says_so() {
        let site = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = site.local_addr().unwrap();
        let echo = thread::spawn(move || {
            let (mut stream, _) = site.accept().unwrap();
            let mut asked = [0u8; 4];
            stream.read_exact(&mut asked).unwrap();
            stream.write_all(&asked).unwrap();
        });
        let relay = Relay::with(Some(address)).unwrap();
        let (mut client, code) = ask(&relay, &name("public.test", 80));
        assert_eq!(code, SUCCEEDED);
        client.write_all(b"ping").unwrap();
        let mut echoed = Vec::new();
        client.read_to_end(&mut echoed).unwrap();
        assert_eq!(echoed, b"ping", "and the site's end reached the browser");
        echo.join().unwrap();
        drop(client);
        assert!(
            wait_until(|| relay.shared.live().is_empty()),
            "a closed connection frees its slot"
        );

        let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let nowhere = closed.local_addr().unwrap();
        drop(closed);
        let relay = Relay::with(Some(nowhere)).unwrap();
        assert_eq!(ask(&relay, &name("public.test", 80)).1, REFUSED);
        assert_eq!(relay.refused(), 0, "a failed connect is not a refusal");
    }

    #[test]
    fn stopping_closes_open_tunnels() {
        let site = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = site.local_addr().unwrap();
        let held = thread::spawn(move || site.accept().unwrap().0);
        let mut relay = Relay::with(Some(address)).unwrap();
        let (mut client, code) = ask(&relay, &name("public.test", 80));
        assert_eq!(code, SUCCEEDED);
        let mut site_side = held.join().unwrap();
        site_side
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let open = relay.stop();
        if cfg!(not(windows)) {
            assert_eq!(open, 0, "no tunnel survives");
        }
        let mut rest = Vec::new();
        assert_eq!(client.read_to_end(&mut rest).unwrap(), 0, "the browser");
        assert_eq!(site_side.read_to_end(&mut rest).unwrap(), 0, "the site");
        drop((client, site_side));
        assert!(
            wait_until(|| relay.shared.live().is_empty()),
            "and once they answer, the tunnel is gone"
        );
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, relay.port())).is_err());
    }
}
