// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Push: a long-lived `EventSource` (SSE) reader (RFC 8620 Section 7.1 and 7.3).
//!
//! Push connections stream responses indefinitely rather than using single request/response cycles.
//! This module reads directly from sockets on a dedicated thread per subscription.
//! It supports plain `http://` and TLS-encrypted `https://` connections.
//!
//! [`expand_url`] resolves `eventSourceUrl` templates (RFC 8620 Section 7.3).
//! [`EventSourceSubscription::start`] connects and processes SSE frames.
//! Parsed `event: state` payloads are forwarded as [`StateChange`] events.
//! Keep-alive `ping` events are discarded.
//! Reconnections use exponential backoff until cancelled.
//! Dropping a subscription cancels the loop and shuts down the active socket.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// The longest line the reader accepts, line terminator included.
const MAX_LINE_BYTES: usize = 1 << 20;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_READ_HEAD_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_STREAM_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Timeouts configured for an [`EventSourceSubscription`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSourceTimeouts {
    /// Maximum duration allowed for the initial TCP connection.
    pub connect: Duration,
    /// Maximum duration allowed for reading the initial HTTP response head.
    pub read_head: Duration,
    /// Maximum duration allowed between SSE events while streaming.
    pub stream_read: Duration,
}

impl Default for EventSourceTimeouts {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            read_head: DEFAULT_READ_HEAD_TIMEOUT,
            stream_read: DEFAULT_STREAM_READ_TIMEOUT,
        }
    }
}

/// Special `types` value requesting all event types (RFC 8620 Section 7.3).
pub const ALL_TYPES: &str = "*";

/// Subscription request headers shared with the reconnect loop.
///
/// Updating headers via [`set`](Self::set) applies them to future reconnect attempts.
/// This avoids tearing down and restarting the subscription.
#[derive(Clone)]
pub struct SharedHeaders(Arc<Mutex<Vec<(String, String)>>>);

impl SharedHeaders {
    /// Start out sending `headers` on every connection attempt.
    pub fn new(headers: Vec<(String, String)>) -> Self {
        Self(Arc::new(Mutex::new(headers)))
    }

    /// Replace headers for future connection attempts.
    ///
    /// Takes effect on the next reconnect attempt. Active connections remain unchanged.
    pub fn set(&self, headers: Vec<(String, String)>) {
        *self.0.lock().expect("shared headers lock poisoned") = headers;
    }

    fn snapshot(&self) -> Vec<(String, String)> {
        self.0.lock().expect("shared headers lock poisoned").clone()
    }
}

impl std::fmt::Debug for SharedHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.0.lock().expect("shared headers lock poisoned").len();
        f.debug_struct("SharedHeaders")
            .field("header_count", &count)
            .finish()
    }
}

/// An item received over an [`EventSourceSubscription`].
#[derive(Clone, Debug, PartialEq)]
pub enum EventSourceItem {
    /// A parsed state change event pushed by the server.
    State(StateChange),
    /// The connection dropped and was re-established.
    Reconnected {
        /// The last event ID seen before reconnection, if any.
        last_event_id: Option<String>,
        /// The total number of successful reconnections so far.
        reconnect_count: u64,
    },
}

/// A live subscription to one account's `eventSourceUrl`, kept alive on its
/// own thread; see the module doc.
pub struct EventSourceSubscription {
    receiver: Receiver<EventSourceItem>,
    cancel: CancelFlag,
    socket: Arc<Mutex<Option<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
    headers: SharedHeaders,
    last_event_id: Arc<Mutex<Option<String>>>,
    reconnect_count: Arc<AtomicU64>,
}

impl std::fmt::Debug for EventSourceSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventSourceSubscription")
            .field("reconnect_count", &self.reconnect_count())
            .field("last_event_id", &self.last_event_id())
            .finish_non_exhaustive()
    }
}

impl EventSourceSubscription {
    /// Start listening on `url` with explicit timeout configuration.
    pub fn start_with_timeouts(
        url: String,
        headers: SharedHeaders,
        cancel: CancelFlag,
        timeouts: EventSourceTimeouts,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let socket = Arc::new(Mutex::new(None));
        let last_event_id = Arc::new(Mutex::new(None));
        let reconnect_count = Arc::new(AtomicU64::new(0));
        let handle = {
            let cancel = cancel.clone();
            let headers = headers.clone();
            let ctx = ConnectionContext {
                socket: Arc::clone(&socket),
                last_id_slot: Arc::clone(&last_event_id),
                reconnect_count: Arc::clone(&reconnect_count),
            };
            thread::spawn(move || run(url, headers, cancel, sender, ctx, timeouts))
        };
        Self {
            receiver,
            cancel,
            socket,
            handle: Some(handle),
            headers,
            last_event_id,
            reconnect_count,
        }
    }

    /// Start listening on `url`.
    ///
    /// `url` is an expanded `eventSourceUrl` (see [`expand_url`]).
    /// Ports default from the URL scheme if not specified.
    /// Headers are re-read on every reconnect attempt (see [`set_headers`](Self::set_headers)).
    pub fn start(url: String, headers: SharedHeaders, cancel: CancelFlag) -> Self {
        Self::start_with_timeouts(url, headers, cancel, EventSourceTimeouts::default())
    }

    /// Block until a [`StateChange`] arrives or `timeout` elapses.
    ///
    /// If a reconnection occurs while waiting, this method continues waiting
    /// for a [`StateChange`] until `timeout` expires. To observe reconnection
    /// notifications directly, use [`recv_item_timeout`](Self::recv_item_timeout)
    /// or inspect [`reconnect_count`](Self::reconnect_count).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<StateChange> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline - now;
            match self.receiver.recv_timeout(remaining).ok()? {
                EventSourceItem::State(change) => return Some(change),
                EventSourceItem::Reconnected { .. } => continue,
            }
        }
    }

    /// Block until an [`EventSourceItem`] arrives or `timeout` elapses.
    pub fn recv_item_timeout(&self, timeout: Duration) -> Option<EventSourceItem> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Return the last event ID received from the server, if any.
    pub fn last_event_id(&self) -> Option<String> {
        self.last_event_id
            .lock()
            .expect("last event id lock poisoned")
            .clone()
    }

    /// Return the total number of successful reconnections so far.
    pub fn reconnect_count(&self) -> u64 {
        self.reconnect_count.load(Ordering::SeqCst)
    }

    /// Drop the active TCP connection from the client side by shutting down
    /// the socket, without cancelling the subscription.
    pub fn drop_connection(&self) {
        if let Some(stream) = self.socket.lock().expect("socket lock poisoned").as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// Replace headers sent on future reconnect attempts.
    ///
    /// Callers can update authentication tokens after credential refreshes.
    /// The next reconnect attempt uses the new headers.
    pub fn set_headers(&self, headers: Vec<(String, String)>) {
        self.headers.set(headers);
    }

    /// Stop listening and join the background thread.
    ///
    /// Cancels subscription, shuts down the active socket, and waits for thread exit.
    /// Idempotent. Also called automatically on [`Drop`].
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

/// Internal handles bundled together across reconnect attempts.
struct ConnectionContext {
    socket: Arc<Mutex<Option<TcpStream>>>,
    last_id_slot: Arc<Mutex<Option<String>>>,
    reconnect_count: Arc<AtomicU64>,
}

/// Reconnect loop: keep opening `url` until `cancel` fires. Reads `headers`
/// fresh on every attempt, so a [`SharedHeaders::set`] reaches the very next
/// one.
fn run(
    url: String,
    headers: SharedHeaders,
    cancel: CancelFlag,
    sender: Sender<EventSourceItem>,
    ctx: ConnectionContext,
    timeouts: EventSourceTimeouts,
) {
    let mut backoff = INITIAL_BACKOFF;
    let mut connection_attempts: u64 = 0;
    while !cancel.is_cancelled() {
        let is_reconnect = connection_attempts > 0;
        match connect_and_stream(
            &url,
            &headers.snapshot(),
            &cancel,
            &sender,
            &ctx,
            is_reconnect,
            timeouts,
        ) {
            Ok(()) => backoff = INITIAL_BACKOFF,
            Err(error) => tracing::debug!(%error, url, "eventsource connection ended"),
        }
        connection_attempts += 1;
        *ctx.socket.lock().expect("socket lock poisoned") = None;
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
    sender: &Sender<EventSourceItem>,
    ctx: &ConnectionContext,
    is_reconnect: bool,
    timeouts: EventSourceTimeouts,
) -> io::Result<()> {
    use std::net::ToSocketAddrs;

    let parts = parse_url(url).ok_or_else(|| {
        io::Error::other(format!(
            "eventsource url is not a usable http(s) URL: {url:?}"
        ))
    })?;
    let mut connected_stream = None;
    let mut last_error = None;
    for addr in parts.dial.to_socket_addrs()? {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match TcpStream::connect_timeout(&addr, timeouts.connect) {
            Ok(s) => {
                connected_stream = Some(s);
                break;
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }
    let stream = connected_stream.ok_or_else(|| {
        last_error.unwrap_or_else(|| io::Error::other(format!("could not resolve {}", parts.dial)))
    })?;

    stream.set_read_timeout(Some(timeouts.read_head))?;
    stream.set_write_timeout(Some(timeouts.connect))?;

    // Store a socket handle before starting the TLS handshake.
    // Calling `shutdown()` on this clone aborts pending I/O operations.
    *ctx.socket.lock().expect("socket lock poisoned") = Some(stream.try_clone()?);
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
    let mut sent_last_event_id = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("last-event-id") {
            sent_last_event_id = true;
        }
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    let last_event_id = ctx
        .last_id_slot
        .lock()
        .expect("last id lock poisoned")
        .clone();
    if !sent_last_event_id && let Some(ref id) = last_event_id {
        request.push_str("Last-Event-ID: ");
        request.push_str(id);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    conn.write_all(request.as_bytes())?;

    let mut reader = BufReader::new(conn);
    read_response_head(&mut reader)?;

    if is_reconnect {
        let count = ctx.reconnect_count.fetch_add(1, Ordering::SeqCst) + 1;
        let last_id = ctx
            .last_id_slot
            .lock()
            .expect("last id lock poisoned")
            .clone();
        let _ = sender.send(EventSourceItem::Reconnected {
            last_event_id: last_id,
            reconnect_count: count,
        });
    }

    if let Some(sock) = ctx.socket.lock().expect("socket lock poisoned").as_ref() {
        let _ = sock.set_read_timeout(Some(timeouts.stream_read));
    }

    let mut body = BufReader::new(ChunkedBody::new(reader));
    stream_events(&mut body, cancel, sender, &ctx.last_id_slot)
}

/// Connection abstraction for plain TCP and TLS streams.
///
/// Dispatches `Read` and `Write` calls to the underlying stream type.
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

/// Shared process-wide TLS client configuration.
///
/// Loads system CA trust anchors via `webpki-roots`.
/// Test builds include an additional self-signed certificate fixture.
/// Provider installation errors are ignored if already initialized.
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

/// Parsed URL components for opening an event source connection.
///
/// Includes scheme, dial target, authority, host name, and request path.
struct UrlParts {
    tls: bool,
    authority: String,
    dial: String,
    host: String,
    path_and_query: String,
}

/// Split an `http(s)://host[:port]/path?query` URL into its parts.
///
/// Returns `None` for another scheme, for a control character or a space anywhere in the URL, and for an authority that is not a plain host and port.
/// A refused character in the request line could add request lines that carry the credentials.
/// A refused character in the authority could make the dialed host differ from the host that `url::is_loopback` reads.
fn parse_url(url: &str) -> Option<UrlParts> {
    if url.bytes().any(|byte| byte <= b' ' || byte == 0x7f) {
        return None;
    }
    let (tls, rest) = crate::url::split_http_scheme(url)?;
    let slash = rest.find('/').unwrap_or(rest.len());
    let authority = rest[..slash].to_owned();
    let path_and_query = if slash < rest.len() {
        rest[slash..].to_owned()
    } else {
        "/".to_owned()
    };
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (literal, after) = bracketed.split_once(']')?;
            literal.parse::<std::net::Ipv6Addr>().ok()?;
            let port = match after {
                "" => None,
                _ => Some(after.strip_prefix(':')?),
            };
            (literal, port)
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority.as_str(), None),
        },
    };
    let host_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_');
    if host.is_empty() || !(authority.starts_with('[') || host.chars().all(host_char)) {
        return None;
    }
    let port = match port {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            digits.parse::<u16>().ok()?
        }
        Some(_) => return None,
        // Use the default port of the scheme (RFC 3986). The Host header keeps the authority of the URL.
        None => {
            if tls {
                443
            } else {
                80
            }
        }
    };
    let dial = if authority.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Some(UrlParts {
        tls,
        authority: authority.clone(),
        dial,
        host: host.to_owned(),
        path_and_query,
    })
}

/// Expand a session object's `eventSourceUrl` (RFC 8620 Section 7.3).
///
/// RFC 8620 Section 7.3 specifies a URI template containing `types`, `closeafter`, and `ping` (RFC 6570).
/// An empty `types` slice requests all event types (`*`).
/// `close_after_state` toggles between `state` and `no`.
/// `ping_seconds` sets the ping interval in seconds (zero disables pings).
///
/// If the template contains none of the three variables, parameters are appended as query arguments.
/// Variable values are percent-encoded except for `,` and `*` (RFC 3986 Section 3.4).
pub fn expand_url(
    template: &str,
    types: &[&str],
    close_after_state: bool,
    ping_seconds: u32,
) -> String {
    // Non-alphanumeric characters allowed unencoded in query values.
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~')
        .remove(b',')
        .remove(b'*');

    let types = if types.is_empty() {
        ALL_TYPES.to_owned()
    } else {
        types.join(",")
    };
    let types = percent_encoding::utf8_percent_encode(&types, KEEP).to_string();
    let closeafter = if close_after_state { "state" } else { "no" };
    let ping = ping_seconds.to_string();

    let mut expanded = template.to_owned();
    let mut substituted = false;
    for (variable, value) in [
        ("{types}", types.as_str()),
        ("{closeafter}", closeafter),
        ("{ping}", ping.as_str()),
    ] {
        if expanded.contains(variable) {
            expanded = expanded.replace(variable, value);
            substituted = true;
        }
    }
    if !substituted {
        let separator = if expanded.contains('?') { '&' } else { '?' };
        expanded.push(separator);
        expanded.push_str(&format!(
            "types={types}&closeafter={closeafter}&ping={ping}"
        ));
    }
    expanded
}

/// Append one line of at most [`MAX_LINE_BYTES`] bytes to `line`.
///
/// A longer line fails with `InvalidData`, so a server cannot exhaust memory with one endless line.
fn read_bounded_line(reader: &mut impl BufRead, line: &mut String) -> io::Result<usize> {
    let mut bytes = Vec::new();
    let n = reader
        .by_ref()
        .take(MAX_LINE_BYTES as u64)
        .read_until(b'\n', &mut bytes)?;
    if n == MAX_LINE_BYTES && bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("eventsource line longer than {MAX_LINE_BYTES} bytes"),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    line.push_str(text);
    Ok(n)
}

/// Read the status line and headers.
///
/// Verifies HTTP 200 and chunked transfer encoding.
fn read_response_head(reader: &mut impl BufRead) -> io::Result<()> {
    let mut status_line = String::new();
    read_bounded_line(reader, &mut status_line)?;
    if !status_line.contains(" 200 ") {
        return Err(io::Error::other(format!(
            "eventsource request failed: {}",
            status_line.trim_end()
        )));
    }
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let n = read_bounded_line(reader, &mut line)?;
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

/// Stream SSE frames from the response body (RFC 8620 Section 7.3).
///
/// Forwards parsed `StateChange` records to `sender`.
/// Returns `Ok(())` when reading stops due to end-of-stream or cancellation.
fn stream_events(
    reader: &mut impl BufRead,
    cancel: &CancelFlag,
    sender: &Sender<EventSourceItem>,
    last_id_slot: &Arc<Mutex<Option<String>>>,
) -> io::Result<()> {
    let mut event = String::new();
    let mut data = String::new();
    let mut frame_id: Option<Option<String>> = None;
    let mut line = String::new();
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        line.clear();
        if read_bounded_line(reader, &mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if let Some(maybe_id) = frame_id.take() {
                *last_id_slot.lock().expect("last id lock poisoned") = maybe_id;
            }
            if event == "state"
                && let Ok(change) = serde_json::from_str::<StateChange>(&data)
                && sender.send(EventSourceItem::State(change)).is_err()
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
        } else if let Some(value) = trimmed.strip_prefix("id:") {
            let val = value.trim();
            if val.is_empty() {
                frame_id = Some(None);
            } else {
                frame_id = Some(Some(val.to_owned()));
            }
        } else if trimmed == "id" {
            frame_id = Some(None);
        }
    }
}

/// Decode an HTTP/1.1 chunked-transfer body on the fly (RFC 9112 Section 7.1).
///
/// Does not assume that individual SSE frames align with chunk boundaries.
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
            if read_bounded_line(&mut self.inner, &mut size_line)? == 0 {
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
                    let n = read_bounded_line(&mut self.inner, &mut trailer)?;
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
            // Connection closed mid-chunk.
            // Surface as EOF so the reconnect loop can retry.
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
    fn parse_url_recognizes_scheme_case_insensitively() {
        let tls = parse_url("HTTPS://example.com:443/eventsource").expect("parses HTTPS");
        assert!(tls.tls);
        assert_eq!(tls.host, "example.com");
        assert_eq!(tls.path_and_query, "/eventsource");

        let mixed_tls = parse_url("Https://example.com/eventsource").expect("parses Https");
        assert!(mixed_tls.tls);
        assert_eq!(mixed_tls.dial, "example.com:443");

        let plain = parse_url("HTTP://example.com:80/eventsource").expect("parses HTTP");
        assert!(!plain.tls);
        assert_eq!(plain.host, "example.com");

        let mixed_plain = parse_url("Http://example.com/eventsource").expect("parses Http");
        assert!(!mixed_plain.tls);
        assert_eq!(mixed_plain.dial, "example.com:80");
    }

    #[test]
    fn parse_url_rejects_an_unknown_scheme() {
        assert!(parse_url("ftp://example.com/eventsource").is_none());
    }

    #[test]
    fn parse_url_dials_the_scheme_default_port_when_the_url_names_none() {
        let plain = parse_url("http://example.com/eventsource").expect("parses");
        assert_eq!(plain.dial, "example.com:80");
        // The `Host:` header must stay what the URL said, without the port
        // this only added in order to have something to connect to.
        assert_eq!(plain.authority, "example.com");
        assert_eq!(plain.host, "example.com");

        let tls = parse_url("https://api.example.com/jmap/eventsource").expect("parses");
        assert_eq!(tls.dial, "api.example.com:443");
        assert_eq!(tls.authority, "api.example.com");
    }

    #[test]
    fn parse_url_keeps_an_explicit_port_to_dial() {
        let parts = parse_url("https://example.com:8443/eventsource").expect("parses");
        assert_eq!(parts.dial, "example.com:8443");
        assert_eq!(parts.authority, "example.com:8443");
    }

    #[test]
    fn parse_url_refuses_a_control_character_or_a_space() {
        for url in [
            "http://127.0.0.1:6379/x\r\nSET key value",
            "http://127.0.0.1/x\nHost: evil.example",
            "http://127.0.0.1/x HTTP/1.0",
            "http://127.0.0.1/x\ty",
            "http://127.0.0.1/x\0y",
            "http://127.0.0.1/x\u{7f}y",
            "http://127.0.0.1\r\n/x",
        ] {
            assert!(parse_url(url).is_none(), "{url:?} must be refused");
        }
    }

    #[test]
    fn parse_url_refuses_a_host_that_the_loopback_check_reads_differently() {
        for url in [
            "http://localhost?.evil.example/es",
            "http://localhost#.evil.example/es",
            "http://localhost\\.evil.example/es",
            "http://localhost@evil.example/es",
            "http://user:secret@evil.example/es",
            "http://[::1/es",
            "http://[::1]x/es",
            "http://local host/es",
            "http:///es",
            "http://:80/es",
            "http://example.com:/es",
            "http://example.com:http/es",
            "http://example.com:65536/es",
        ] {
            assert!(parse_url(url).is_none(), "{url:?} must be refused");
        }
    }

    #[test]
    fn parse_url_dials_the_host_that_the_loopback_check_reads() {
        for (url, loopback) in [
            ("http://localhost/es", true),
            ("http://127.0.0.1:4242/es", true),
            ("http://[::1]:4242/es", true),
            ("http://localhost.evil.example/es", false),
            ("https://jmap.example.com/es", false),
        ] {
            let parts = parse_url(url).expect("parses");
            assert_eq!(crate::url::is_loopback(url), loopback, "{url}");
            let dialed = parts.dial.rsplit_once(':').expect("port").0;
            assert_eq!(
                crate::url::is_loopback(&format!("http://{dialed}/")),
                loopback
            );
        }
    }

    #[test]
    fn parse_url_dials_a_bracketed_ipv6_literal() {
        let plain = parse_url("http://[::1]:4242/eventsource").expect("parses");
        assert_eq!(plain.dial, "[::1]:4242");
        assert_eq!(plain.authority, "[::1]:4242");
        assert_eq!(plain.host, "::1");

        let tls = parse_url("https://[::1]/eventsource").expect("parses");
        assert_eq!(tls.dial, "[::1]:443");
        assert_eq!(tls.authority, "[::1]");
        assert_eq!(tls.host, "::1");
    }

    #[test]
    fn a_url_with_an_injected_request_line_is_never_dialed() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let url = format!(
            "http://{}/x HTTP/1.1\r\nHost: a\r\n\r\nSET key value\r\n",
            listener.local_addr().expect("addr")
        );
        let _subscription = EventSourceSubscription::start(
            url,
            SharedHeaders::new(vec![("Authorization".into(), "Bearer t".into())]),
            CancelFlag::default(),
        );
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok(_) => panic!("the subscription dialed a URL with a CR LF"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    #[test]
    fn expand_url_substitutes_the_three_required_template_variables() {
        let expanded = expand_url(
            "https://jmap.example.com/eventsource/\
             ?types={types}&closeafter={closeafter}&ping={ping}",
            &["Mailbox", "Email"],
            false,
            30,
        );
        assert_eq!(
            expanded,
            "https://jmap.example.com/eventsource/\
             ?types=Mailbox,Email&closeafter=no&ping=30"
        );
    }

    #[test]
    fn expand_url_asks_for_every_type_when_given_none() {
        let expanded = expand_url("http://h/es?types={types}", &[], false, 0);
        assert_eq!(expanded, "http://h/es?types=*");
    }

    #[test]
    fn expand_url_says_state_when_the_caller_wants_the_stream_closed() {
        let expanded = expand_url("http://h/es?closeafter={closeafter}", &[], true, 0);
        assert_eq!(expanded, "http://h/es?closeafter=state");
    }

    #[test]
    fn expand_url_percent_encodes_a_value_that_needs_it() {
        // Not a type name any server would send, but the substitution must
        // not be able to smuggle a second query parameter in either way.
        let expanded = expand_url("http://h/es?types={types}", &["a b&ping=9"], false, 0);
        assert_eq!(expanded, "http://h/es?types=a%20b%26ping%3D9");
    }

    #[test]
    fn expand_url_appends_the_parameters_a_server_left_out_of_its_template() {
        // `jmap-mock` advertises exactly this: a bare path, no variables.
        let expanded = expand_url("http://127.0.0.1:4242/eventsource", &["Email"], true, 0);
        assert_eq!(
            expanded,
            "http://127.0.0.1:4242/eventsource?types=Email&closeafter=state&ping=0"
        );
    }

    #[test]
    fn expand_url_appends_with_an_ampersand_to_an_existing_query() {
        let expanded = expand_url("http://h/es?tenant=7", &[], false, 0);
        assert_eq!(
            expanded,
            "http://h/es?tenant=7&types=*&closeafter=no&ping=0"
        );
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
        // Verify line reassembly when chunk boundaries split a single line.
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
        let last_id_slot = Arc::new(Mutex::new(None));
        stream_events(&mut reader, &CancelFlag::new(), &sender, &last_id_slot)
            .expect("no I/O error");
        let received = receiver.try_recv().expect("one state event forwarded");
        assert_eq!(
            received,
            EventSourceItem::State(changed("a1", "Mailbox", "1"))
        );
        assert!(
            receiver.try_recv().is_err(),
            "the ping must not be forwarded"
        );
    }

    #[test]
    fn stream_events_tracks_and_resets_event_id() {
        let raw = b"id: 42\nevent: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}\n\n\
                    id:\nevent: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"2\"}}}\n\n";
        let mut reader = BufReader::new(Cursor::new(&raw[..]));
        let (sender, receiver) = mpsc::channel();
        let last_id_slot = Arc::new(Mutex::new(None));
        stream_events(&mut reader, &CancelFlag::new(), &sender, &last_id_slot)
            .expect("no I/O error");

        let first = receiver.try_recv().expect("first event");
        assert_eq!(first, EventSourceItem::State(changed("a1", "Mailbox", "1")));

        let second = receiver.try_recv().expect("second event");
        assert_eq!(
            second,
            EventSourceItem::State(changed("a1", "Mailbox", "2"))
        );
        assert_eq!(*last_id_slot.lock().unwrap(), None);
    }

    #[test]
    fn subscribing_receives_a_pushed_state_change() {
        let server = jmap_mock::MockServer::builder().start();
        let url = format!(
            "{}/eventsource?types=*&closeafter=no&ping=0",
            server.origin()
        );
        let subscription =
            EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

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
        let subscription =
            EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        let first = changed("a1", "Mailbox", "1");
        server.push_state_change(&first);
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(5))
                .expect("first push arrives"),
            first
        );

        // Dead subscribers are pruned lazily on the next broadcast.
        // Wait for a second subscriber to ensure the reconnect succeeded.
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
        let subscription =
            EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());
        server.wait_for_event_source_subscriber(Duration::from_secs(5));

        let started = Instant::now();
        drop(subscription);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "dropping a live subscription must not wait out a reconnect backoff"
        );
    }

    #[test]
    fn set_headers_lets_a_stalled_reconnect_succeed_with_a_refreshed_token() {
        let server = jmap_mock::MockServer::builder()
            .bearer_token("fresh-token")
            .start();
        let url = format!(
            "{}/eventsource?types=*&closeafter=no&ping=0",
            server.origin()
        );
        let headers = SharedHeaders::new(vec![(
            "Authorization".to_owned(),
            "Bearer stale-token".to_owned(),
        )]);
        let subscription = EventSourceSubscription::start(url, headers, CancelFlag::new());

        // The stale token is rejected.
        // This confirms the reconnect loop retries with initial headers.
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.unauthorized_responses() == 0 {
            assert!(
                Instant::now() < deadline,
                "the stale token was never refused"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            server
                .state()
                .lock()
                .unwrap()
                .event_source
                .subscriber_count(),
            0,
            "a refused connection attempt must not count as a subscriber"
        );

        subscription.set_headers(vec![(
            "Authorization".to_owned(),
            "Bearer fresh-token".to_owned(),
        )]);

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
    }

    /// Minimal one-shot TLS server for testing.
    ///
    /// Accepts a connection, sends a chunked `event: state` frame, and closes.
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

            // Discard request bytes up to the terminating blank line.
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

        let subscription =
            EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());
        let received = subscription
            .recv_timeout(Duration::from_secs(5))
            .expect("the pushed StateChange arrives over TLS");
        assert_eq!(received, changed("a1", "Mailbox", "1"));
    }

    #[test]
    fn sleep_cancellable_waits_for_duration_and_wakes_on_cancel() {
        let cancel = CancelFlag::new();
        let start = Instant::now();
        sleep_cancellable(Duration::from_millis(60), &cancel);
        assert!(start.elapsed() >= Duration::from_millis(50));

        let cancel2 = CancelFlag::new();
        cancel2.cancel();
        let start2 = Instant::now();
        sleep_cancellable(Duration::from_secs(5), &cancel2);
        assert!(start2.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn read_response_head_validates_status_and_chunked_encoding() {
        let mut ok_head = Cursor::new(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert!(read_response_head(&mut ok_head).is_ok());

        let mut err_status = Cursor::new(
            b"HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\n\r\n",
        );
        let err = read_response_head(&mut err_status).unwrap_err();
        assert!(
            err.to_string()
                .contains("eventsource request failed: HTTP/1.1 500")
        );

        let mut not_chunked = Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n");
        let err = read_response_head(&mut not_chunked).unwrap_err();
        assert!(
            err.to_string()
                .contains("eventsource response is not chunked")
        );

        let mut gzip_chunked = Cursor::new(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n");
        let err = read_response_head(&mut gzip_chunked).unwrap_err();
        assert!(
            err.to_string()
                .contains("eventsource response is not chunked")
        );
    }

    #[test]
    fn stream_events_handles_id_clearing_and_non_state_events() {
        let raw = b"id\nevent: other\ndata: {}\n\nid: 99\nevent: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}\n\n";
        let mut reader = BufReader::new(Cursor::new(&raw[..]));
        let (sender, receiver) = mpsc::channel();
        let last_id_slot = Arc::new(Mutex::new(Some("old-id".to_owned())));
        stream_events(&mut reader, &CancelFlag::new(), &sender, &last_id_slot).expect("no error");
        let event = receiver.try_recv().expect("state event");
        assert_eq!(event, EventSourceItem::State(changed("a1", "Mailbox", "1")));
        assert_eq!(*last_id_slot.lock().unwrap(), Some("99".to_owned()));

        let raw_reset = b"id\nevent: ping\ndata: {}\n\n";
        let mut reader_reset = BufReader::new(Cursor::new(&raw_reset[..]));
        stream_events(
            &mut reader_reset,
            &CancelFlag::new(),
            &sender,
            &last_id_slot,
        )
        .expect("no error");
        assert_eq!(*last_id_slot.lock().unwrap(), None);
    }

    #[test]
    fn chunked_body_handles_extensions_and_trailers() {
        let raw = b"5;ext=foo\r\nhello\r\n0\r\nTrailer-Header: val\r\n\r\nextra";
        let mut inner = BufReader::new(Cursor::new(&raw[..]));
        let mut body = ChunkedBody::new(&mut inner);
        let mut out = String::new();
        body.read_to_string(&mut out).expect("decode");
        assert_eq!(out, "hello");
        assert!(body.finished);

        // Calling read on finished ChunkedBody with non-empty buffer must return 0
        // without consuming from inner reader (kills || replaced with &&).
        let mut extra_buf = [0u8; 10];
        assert_eq!(body.read(&mut extra_buf).unwrap(), 0);

        // All trailer lines including blank line were consumed (kills == with != on trailer loop).
        let mut remaining = String::new();
        inner
            .read_to_string(&mut remaining)
            .expect("read remaining");
        assert_eq!(remaining, "extra");

        let empty = b"";
        let mut empty_body = ChunkedBody::new(BufReader::new(Cursor::new(empty)));
        let mut buf = [0u8; 10];
        assert_eq!(empty_body.read(&mut buf).unwrap(), 0);

        let malformed = b"xyz\r\nhello\r\n";
        let mut malformed_body = ChunkedBody::new(BufReader::new(Cursor::new(malformed)));
        assert!(malformed_body.read(&mut buf).is_err());
    }

    /// `len` bytes of `a`, longer than any line the reader accepts.
    fn overlong(len: usize) -> Vec<u8> {
        vec![b'a'; len]
    }

    fn assert_line_too_long(result: io::Result<()>) {
        let err = result.expect_err("an overlong line must fail the read");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        assert!(err.to_string().contains("line longer than"), "{err}");
    }

    #[test]
    fn stream_events_refuses_a_data_line_longer_than_the_cap() {
        let mut raw = b"event: state\ndata: ".to_vec();
        raw.extend(overlong(2 * MAX_LINE_BYTES));
        raw.extend(b"\n\n");
        let mut reader = BufReader::new(Cursor::new(raw));
        let (sender, receiver) = mpsc::channel();
        let last_id_slot = Arc::new(Mutex::new(None));
        assert_line_too_long(stream_events(
            &mut reader,
            &CancelFlag::new(),
            &sender,
            &last_id_slot,
        ));
        assert!(receiver.try_recv().is_err(), "nothing may be forwarded");
    }

    #[test]
    fn stream_events_accepts_a_line_of_exactly_the_cap() {
        let head = b"data: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}";
        let mut raw = b"event: state\n".to_vec();
        raw.extend(head);
        raw.extend(vec![b' '; MAX_LINE_BYTES - head.len() - 1]);
        raw.extend(b"\n\n");
        let mut reader = BufReader::new(Cursor::new(raw));
        let (sender, receiver) = mpsc::channel();
        let last_id_slot = Arc::new(Mutex::new(None));
        stream_events(&mut reader, &CancelFlag::new(), &sender, &last_id_slot)
            .expect("a line of exactly the cap is accepted");
        assert_eq!(
            receiver.try_recv().expect("the state event is forwarded"),
            EventSourceItem::State(changed("a1", "Mailbox", "1"))
        );
    }

    #[test]
    fn read_response_head_refuses_an_overlong_status_line() {
        let mut raw = b"HTTP/1.1 200 OK ".to_vec();
        raw.extend(overlong(2 * MAX_LINE_BYTES));
        raw.extend(b"\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_line_too_long(read_response_head(&mut Cursor::new(raw)));
    }

    #[test]
    fn read_response_head_refuses_an_overlong_header_line() {
        let mut raw = b"HTTP/1.1 200 OK\r\nX-Pad: ".to_vec();
        raw.extend(overlong(2 * MAX_LINE_BYTES));
        raw.extend(b"\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_line_too_long(read_response_head(&mut Cursor::new(raw)));
    }

    #[test]
    fn chunked_body_refuses_an_overlong_chunk_size_line() {
        let mut raw = b"5;".to_vec();
        raw.extend(overlong(2 * MAX_LINE_BYTES));
        raw.extend(b"\r\nhello\r\n0\r\n\r\n");
        let mut body = ChunkedBody::new(BufReader::new(Cursor::new(raw)));
        let mut out = Vec::new();
        assert_line_too_long(body.read_to_end(&mut out).map(drop));
    }

    #[test]
    fn chunked_body_refuses_an_overlong_trailer_line() {
        let mut raw = b"5\r\nhello\r\n0\r\nX-Trailer: ".to_vec();
        raw.extend(overlong(2 * MAX_LINE_BYTES));
        raw.extend(b"\r\n\r\n");
        let mut body = ChunkedBody::new(BufReader::new(Cursor::new(raw)));
        let mut out = Vec::new();
        assert_line_too_long(body.read_to_end(&mut out).map(drop));
    }

    #[test]
    fn an_endless_line_ends_the_connection_and_the_next_one_delivers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            for (attempt, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            return;
                        }
                        request.push(byte[0]);
                    }
                    let _ =
                        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                    if attempt == 0 {
                        // One chunk that never ends a line, then hold the connection open.
                        let _ =
                            stream.write_all(format!("{:x}\r\n", 8 * MAX_LINE_BYTES).as_bytes());
                        let _ = stream.write_all(&overlong(2 * MAX_LINE_BYTES));
                        let _ = stream.read(&mut byte);
                    } else {
                        let frame = "event: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Mailbox\":\"1\"}}}\n\n";
                        let _ = stream.write_all(
                            format!("{:x}\r\n{frame}\r\n0\r\n\r\n", frame.len()).as_bytes(),
                        );
                    }
                });
            }
        });

        let subscription = EventSourceSubscription::start(
            format!("http://{addr}/eventsource"),
            SharedHeaders::new(Vec::new()),
            CancelFlag::new(),
        );
        let received = subscription
            .recv_timeout(Duration::from_secs(10))
            .expect("the reader drops the endless line and reconnects");
        assert_eq!(received, changed("a1", "Mailbox", "1"));
    }
}
