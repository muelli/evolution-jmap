// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Push: a long-lived `EventSource` (SSE) reader (RFC 8620 §7.1, §7.3).
//!
//! [`crate::transport::Transport`] is a unary request/response abstraction —
//! deliberately, so a libsoup-backed transport can drop in later without
//! touching protocol logic — but a push connection is the opposite shape: one
//! request whose response body never ends. So this reads the wire itself,
//! on a dedicated thread per subscription, rather than going through
//! `Transport`. It speaks both plain `http://` (the mock, and every URL this
//! codebase builds today) and `https://` (`rustls` over the same raw
//! `TcpStream`, since `ureq`'s own TLS backend is exactly that already — one
//! layer below the unary request/response API this module deliberately
//! bypasses, not a second implementation of it).
//!
//! [`EventSourceSubscription::start`] connects, reads chunked-transfer SSE
//! frames, and forwards every `event: state` block's `data:` as a parsed
//! [`StateChange`]. A `ping` (or any other) event is read and discarded — it
//! exists only to prove the connection is still alive. Both a clean end (the
//! server closed the stream, as `closeafter=state` does) and a broken one
//! (a network error, a non-200 status) are just reasons to reconnect, with
//! exponential backoff, until [`CancelFlag::cancel`] is called — [`Drop`]
//! calls it too, and also shuts down whatever socket the background thread
//! currently holds, so tearing a subscription down can never block on a dead
//! or merely idle push connection.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use jmap_proto::push::StateChange;

use crate::transport::CancelFlag;

const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A live subscription to one account's `eventSourceUrl`, kept alive on its
/// own thread; see the module doc.
pub struct EventSourceSubscription {
    receiver: Receiver<StateChange>,
    cancel: CancelFlag,
    socket: Arc<Mutex<Option<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
}

impl EventSourceSubscription {
    /// Start listening on `url` — a full `GET` target, e.g. the session's
    /// `eventSourceUrl` with `types`/`closeafter`/`ping` already appended by
    /// the caller; this module does not itself build that query string, or
    /// do the RFC 6570 template substitution `eventSourceUrl` is specified
    /// as, since nothing in this codebase emits a templated one yet.
    /// `headers` (typically `Authorization`) are sent on every connection
    /// attempt. `url` must include an explicit port — every `eventSourceUrl`
    /// this codebase produces does.
    pub fn start(url: String, headers: Vec<(String, String)>, cancel: CancelFlag) -> Self {
        let (sender, receiver) = mpsc::channel();
        let socket = Arc::new(Mutex::new(None));
        let handle = {
            let cancel = cancel.clone();
            let socket = Arc::clone(&socket);
            thread::spawn(move || run(url, headers, cancel, socket, sender))
        };
        Self {
            receiver,
            cancel,
            socket,
            handle: Some(handle),
        }
    }

    /// Block until a [`StateChange`] arrives or `timeout` elapses.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<StateChange> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Stop listening: cancel, shut down the current socket (if any) so a
    /// blocking read on it returns immediately, and join the background
    /// thread. Idempotent; also runs on [`Drop`] — calling it explicitly is
    /// only useful to observe completion at a chosen point.
    pub fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(stream) = self.socket.lock().expect("socket lock poisoned").take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for EventSourceSubscription {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reconnect loop: keep opening `url` until `cancel` fires.
fn run(
    url: String,
    headers: Vec<(String, String)>,
    cancel: CancelFlag,
    socket: Arc<Mutex<Option<TcpStream>>>,
    sender: Sender<StateChange>,
) {
    let mut backoff = INITIAL_BACKOFF;
    while !cancel.is_cancelled() {
        match connect_and_stream(&url, &headers, &cancel, &socket, &sender) {
            Ok(()) => backoff = INITIAL_BACKOFF,
            Err(error) => tracing::debug!(%error, url, "eventsource connection ended"),
        }
        *socket.lock().expect("socket lock poisoned") = None;
        if cancel.is_cancelled() {
            break;
        }
        sleep_cancellable(backoff, &cancel);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Sleep up to `duration`, checking `cancel` every [`CANCEL_POLL_INTERVAL`]
/// so a cancellation during backoff is noticed promptly rather than only
/// after the full backoff elapses.
fn sleep_cancellable(duration: Duration, cancel: &CancelFlag) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.is_cancelled() {
            return;
        }
        thread::sleep(CANCEL_POLL_INTERVAL.min(duration));
    }
}

/// One connection attempt: connect, send the request, read the response
/// head, then stream events off a chunked body until it ends or fails.
fn connect_and_stream(
    url: &str,
    headers: &[(String, String)],
    cancel: &CancelFlag,
    socket_slot: &Arc<Mutex<Option<TcpStream>>>,
    sender: &Sender<StateChange>,
) -> io::Result<()> {
    let parts = parse_url(url)
        .ok_or_else(|| io::Error::other(format!("eventsource url is not http(s)://: {url}")))?;
    let stream = TcpStream::connect(&parts.authority)?;
    // Stored before the TLS handshake (if any): `shutdown()` on this clone
    // aborts the handshake or any in-flight read/write on the *other* clone
    // `Conn` goes on to own, since both are handles onto the same socket.
    *socket_slot.lock().expect("socket lock poisoned") = Some(stream.try_clone()?);
    if cancel.is_cancelled() {
        return Ok(());
    }

    let mut conn = if parts.tls {
        let server_name = ServerName::try_from(parts.host.clone()).map_err(|error| {
            io::Error::other(format!(
                "not a valid TLS server name: {:?}: {error}",
                parts.host
            ))
        })?;
        let client = ClientConnection::new(Arc::clone(tls_config()), server_name)
            .map_err(|error| io::Error::other(format!("tls setup failed: {error}")))?;
        Conn::Tls(Box::new(StreamOwned::new(client, stream)))
    } else {
        Conn::Plain(stream)
    };

    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n",
        parts.path_and_query, parts.authority
    );
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    conn.write_all(request.as_bytes())?;

    let mut reader = BufReader::new(conn);
    read_response_head(&mut reader)?;
    let mut body = BufReader::new(ChunkedBody::new(reader));
    stream_events(&mut body, cancel, sender)
}

/// The two shapes a connection can take once a scheme is known — `Read`/
/// `Write` just dispatch to whichever one this attempt picked, so everything
/// downstream (`ChunkedBody`, the SSE line reader) stays scheme-agnostic.
enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(stream) => stream.read(buf),
            Conn::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(stream) => stream.write(buf),
            Conn::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(stream) => stream.flush(),
            Conn::Tls(stream) => stream.flush(),
        }
    }
}

/// The process-wide TLS client config: the real webpki CA roots, plus (test
/// builds only) the one self-signed root this crate's own `tests/fixtures`
/// TLS test server uses — never compiled into a shipped binary, since it is
/// behind `#[cfg(test)]`. Installing a default crypto provider a second time
/// (another `rustls` user in the same process already did) returns `Err`
/// rather than panicking, so it is ignored rather than unwrapped — the same
/// "tolerant of being called again" idiom `jmap_backend_core::logging::init`
/// and `i18n::bind` already use for their own once-per-process setup.
fn tls_config() -> &'static Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        #[cfg(test)]
        {
            let fixture = include_bytes!("../tests/fixtures/tls-test-cert.der");
            roots
                .add(rustls::pki_types::CertificateDer::from(fixture.as_slice()).into_owned())
                .expect("test fixture cert parses as a valid trust anchor");
        }
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
}

/// A URL this module can act on: which scheme (`tls`), the authority to
/// dial and put in the `Host:` header (`host:port`), the bare host for TLS
/// SNI/certificate-name checking, and the request target.
struct UrlParts {
    tls: bool,
    authority: String,
    host: String,
    path_and_query: String,
}

/// Split an `http(s)://host:port/path?query` URL into its parts. `None` for
/// any other scheme, or one this module cannot make sense of.
fn parse_url(url: &str) -> Option<UrlParts> {
    let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let slash = rest.find('/').unwrap_or(rest.len());
    let authority = rest[..slash].to_owned();
    let path_and_query = if slash < rest.len() {
        rest[slash..].to_owned()
    } else {
        "/".to_owned()
    };
    // Every `eventSourceUrl` this codebase produces (see the module doc)
    // includes an explicit port, so a plain `rsplit_once` is enough — no
    // IPv6 literal (`[::1]:port`) support is needed or attempted here.
    let host = authority
        .rsplit_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(&authority)
        .to_owned();
    Some(UrlParts {
        tls,
        authority,
        host,
        path_and_query,
    })
}

/// Read the status line and headers, checking the response is chunked —
/// the only framing this reader knows how to consume, and the only one an
/// indefinite stream (no `Content-Length` is possible) can arrive as.
fn read_response_head(reader: &mut impl BufRead) -> io::Result<()> {
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    if !status_line.contains(" 200 ") {
        return Err(io::Error::other(format!(
            "eventsource request failed: {}",
            status_line.trim_end()
        )));
    }
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    if chunked {
        Ok(())
    } else {
        Err(io::Error::other(
            "eventsource response is not chunked transfer-encoding",
        ))
    }
}

/// Read SSE frames (RFC 8620 §7.3 example: `event: state` / `data: {…}`,
/// blank-line terminated) off `reader`, forwarding a `state` event's `data`
/// — parsed as a [`StateChange`] — to `sender`. Returns `Ok(())` on a clean
/// end (terminal chunk / EOF) as readily as on cancellation: both just mean
/// "stop reading", and [`run`] treats every `Ok`/`Err` return alike, as a
/// reason to reconnect.
fn stream_events(
    reader: &mut impl BufRead,
    cancel: &CancelFlag,
    sender: &Sender<StateChange>,
) -> io::Result<()> {
    let mut event = String::new();
    let mut data = String::new();
    let mut line = String::new();
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if event == "state"
                && let Ok(change) = serde_json::from_str::<StateChange>(&data)
                && sender.send(change).is_err()
            {
                return Ok(());
            }
            event.clear();
            data.clear();
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("event:") {
            event = value.trim().to_owned();
        } else if let Some(value) = trimmed.strip_prefix("data:") {
            data = value.trim().to_owned();
        }
    }
}

/// Decodes an HTTP/1.1 chunked-transfer body (RFC 9112 §7.1) on the fly, so
/// the SSE parser above can just read lines without knowing chunk
/// boundaries exist. Deliberately does not assume one SSE frame is one
/// chunk — true of `jmap-mock`'s own writer, not guaranteed by the spec.
struct ChunkedBody<R> {
    inner: R,
    remaining_in_chunk: usize,
    finished: bool,
}

impl<R: BufRead> ChunkedBody<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            remaining_in_chunk: 0,
            finished: false,
        }
    }
}

impl<R: BufRead> Read for ChunkedBody<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining_in_chunk == 0 {
            let mut size_line = String::new();
            if self.inner.read_line(&mut size_line)? == 0 {
                self.finished = true;
                return Ok(0);
            }
            let size_str = size_line.trim_end_matches(['\r', '\n']);
            // A chunk-extension (`;name=value`) can follow the size; RFC
            // 9112 §7.1.1 says a recipient MAY ignore it, so this does.
            let size_str = size_str.split(';').next().unwrap_or(size_str).trim();
            let size = usize::from_str_radix(size_str, 16)
                .map_err(|_| io::Error::other(format!("malformed chunk size: {size_str:?}")))?;
            if size == 0 {
                // The terminal chunk: consume the trailer section up to its
                // blank line, then this body is over.
                loop {
                    let mut trailer = String::new();
                    let n = self.inner.read_line(&mut trailer)?;
                    if n == 0 || trailer.trim_end_matches(['\r', '\n']).is_empty() {
                        break;
                    }
                }
                self.finished = true;
                return Ok(0);
            }
            self.remaining_in_chunk = size;
        }
        let want = buf.len().min(self.remaining_in_chunk);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            // The connection closed mid-chunk: an incomplete body, not a
            // graceful end, but the caller treats every non-`Err` return
            // from `stream_events` alike, so surfacing it as EOF here is
            // enough for the reconnect loop to act on.
            self.finished = true;
            return Ok(0);
        }
        self.remaining_in_chunk -= n;
        if self.remaining_in_chunk == 0 {
            let mut crlf = [0u8; 2];
            self.inner.read_exact(&mut crlf)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::time::Duration;

    use jmap_proto::{Id, State};

    use super::*;

    fn changed(account: &str, kind: &str, state: &str) -> StateChange {
        let mut types = BTreeMap::new();
        types.insert(kind.to_owned(), State::new(state));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new(account), types);
        StateChange::new(changed)
    }

    #[test]
    fn parse_url_separates_authority_host_and_path_and_query() {
        let parts = parse_url("http://127.0.0.1:4242/eventsource?types=*").expect("parses");
        assert!(!parts.tls);
        assert_eq!(parts.authority, "127.0.0.1:4242");
        assert_eq!(parts.host, "127.0.0.1");
        assert_eq!(parts.path_and_query, "/eventsource?types=*");
    }

    #[test]
    fn parse_url_defaults_a_missing_path_to_root() {
        let parts = parse_url("http://127.0.0.1:4242").expect("parses");
        assert_eq!(parts.authority, "127.0.0.1:4242");
        assert_eq!(parts.path_and_query, "/");
    }

    #[test]
    fn parse_url_recognizes_the_https_scheme() {
        let parts = parse_url("https://example.com:443/eventsource").expect("parses");
        assert!(parts.tls);
        assert_eq!(parts.host, "example.com");
        assert_eq!(parts.path_and_query, "/eventsource");
    }

    #[test]
    fn parse_url_rejects_an_unknown_scheme() {
        assert!(parse_url("ftp://example.com/eventsource").is_none());
    }

    #[test]
    fn chunked_body_decodes_one_chunk() {
        let raw = b"5\r\nhello\r\n0\r\n\r\n";
        let mut body = ChunkedBody::new(BufReader::new(Cursor::new(raw)));
        let mut out = String::new();
        body.read_to_string(&mut out).expect("decode");
        assert_eq!(out, "hello");
    }

    #[test]
    fn chunked_body_reassembles_a_line_split_across_two_chunks() {
        // The SSE parser reads lines; this proves it still gets a whole
        // one even when the chunk boundary falls inside it: "event:" (6
        // bytes) in the first chunk, " state\n" (7 bytes) in the second.
        let raw = b"6\r\nevent:\r\n7\r\n state\n\r\n0\r\n\r\n";
        let mut reader = BufReader::new(ChunkedBody::new(BufReader::new(Cursor::new(raw))));
        let mut line = String::new();
        reader.read_line(&mut line).expect("read a line");
        assert_eq!(line, "event: state\n");
    }

    #[test]
    fn stream_events_forwards_a_state_event_and_ignores_a_ping() {
        let raw = b"event: ping\ndata: {\"interval\":60}\n\nevent: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}\n\n";
        let mut reader = BufReader::new(Cursor::new(&raw[..]));
        let (sender, receiver) = mpsc::channel();
        stream_events(&mut reader, &CancelFlag::new(), &sender).expect("no I/O error");
        let received = receiver.try_recv().expect("one state event forwarded");
        assert_eq!(received, changed("a1", "Mailbox", "1"));
        assert!(
            receiver.try_recv().is_err(),
            "the ping must not be forwarded"
        );
    }

    #[test]
    fn subscribing_receives_a_pushed_state_change() {
        let server = jmap_mock::MockServer::builder().start();
        let url = format!(
            "{}/eventsource?types=*&closeafter=no&ping=0",
            server.origin()
        );
        let subscription = EventSourceSubscription::start(url, Vec::new(), CancelFlag::new());

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        let expected = changed("a3123", "Email", "d35ecb040aab");
        server.push_state_change(&expected);

        let received = subscription
            .recv_timeout(Duration::from_secs(5))
            .expect("the pushed StateChange arrives");
        assert_eq!(received, expected);
    }

    #[test]
    fn it_reconnects_after_the_server_ends_the_connection_cleanly() {
        let server = jmap_mock::MockServer::builder().start();
        // `closeafter=state` ends the chunked body right after one push
        // (RFC 8620 §7.3), so the first push forces exactly the clean
        // reconnect this test is after.
        let url = format!(
            "{}/eventsource?types=*&closeafter=state&ping=0",
            server.origin()
        );
        let subscription = EventSourceSubscription::start(url, Vec::new(), CancelFlag::new());

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        let first = changed("a1", "Mailbox", "1");
        server.push_state_change(&first);
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(5))
                .expect("first push arrives"),
            first
        );

        // The connection above just closed, but its dead subscriber is
        // only pruned lazily, on the next failed broadcast (see
        // `EventSourceHub::broadcast`) — so `wait_for_event_source_
        // subscriber` (count > 0) would see the stale entry and return
        // immediately, racing ahead of the actual reconnect. Wait for a
        // *second* registered subscriber instead, as
        // `jmap-mock/tests/eventsource.rs`'s own multi-subscriber test does.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let count = server
                .state()
                .lock()
                .unwrap()
                .event_source
                .subscriber_count();
            if count >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "no reconnect within 5s");
            thread::sleep(Duration::from_millis(5));
        }
        let second = changed("a1", "Mailbox", "2");
        server.push_state_change(&second);
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(5))
                .expect("second push arrives after reconnecting"),
            second
        );
    }

    #[test]
    fn dropping_the_subscription_does_not_block_on_a_live_connection() {
        let server = jmap_mock::MockServer::builder().start();
        let url = format!(
            "{}/eventsource?types=*&closeafter=no&ping=0",
            server.origin()
        );
        let subscription = EventSourceSubscription::start(url, Vec::new(), CancelFlag::new());
        server.wait_for_event_source_subscriber(Duration::from_secs(5));

        let started = Instant::now();
        drop(subscription);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "dropping a live subscription must not wait out a reconnect backoff"
        );
    }

    /// A minimal one-shot TLS server: accepts one connection, discards
    /// whatever the client sends (only this module's own `GET` request is
    /// ever expected), and writes back one chunked `event: state` frame
    /// using the same key/cert pair `tls_config`'s `#[cfg(test)]` half
    /// trusts. Proves the real handshake and chunked-SSE decode work
    /// together over TLS, not just that each is independently correct.
    fn start_tls_test_server(state_change_frame: &'static str) -> std::net::SocketAddr {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use rustls::{ServerConfig, ServerConnection};

        let cert =
            CertificateDer::from(include_bytes!("../tests/fixtures/tls-test-cert.der").as_slice());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("../tests/fixtures/tls-test-key.der").as_slice(),
        ));
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .expect("test cert/key pair is valid"),
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            let (stream, _peer) = listener.accept().expect("accept");
            let server_conn = ServerConnection::new(config).expect("server tls session");
            let mut tls = StreamOwned::new(server_conn, stream);

            // Read (and discard) the request up to its terminating blank
            // line — enough to know the client is done sending, without a
            // full HTTP parser this one-shot server doesn't need.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if tls.read(&mut byte).expect("read request") == 0 {
                    break;
                }
                request.push(byte[0]);
            }

            let body = format!(
                "{:x}\r\n{state_change_frame}\r\n0\r\n\r\n",
                state_change_frame.len()
            );
            let response = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{body}");
            tls.write_all(response.as_bytes()).expect("write response");
            tls.conn.send_close_notify();
            let _ = tls.flush();
        });
        addr
    }

    #[test]
    fn subscribing_over_https_completes_a_real_tls_handshake() {
        let frame = "event: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}\n\n";
        let addr = start_tls_test_server(frame);
        let url = format!("https://{addr}/eventsource?types=*");

        let subscription = EventSourceSubscription::start(url, Vec::new(), CancelFlag::new());
        let received = subscription
            .recv_timeout(Duration::from_secs(5))
            .expect("the pushed StateChange arrives over TLS");
        assert_eq!(received, changed("a1", "Mailbox", "1"));
    }
}
