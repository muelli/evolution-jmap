// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8887 JMAP over WebSocket transport (Batch 68).
//!
//! Provides [`WebSocketConnection`] built from a session's
//! `urn:ietf:params:jmap:websocket` capability advertisement.
//!
//! Handles:
//! - Subprotocol negotiation (`Sec-WebSocket-Protocol: jmap`)
//! - Credential handling matching HTTP transport rules
//! - Multiplexed `Request` and `Response`/`RequestError` correlation by `id`
//! - `maxConcurrentRequests` bounding
//! - Clean close handshake (RFC 6455 §5.5.1)

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use jmap_proto::push::StateChange;
use jmap_proto::request::Invocation;
use jmap_proto::websocket::{WebSocketRequest, WebSocketRequestError, WebSocketResponse};

use crate::concurrency::ConcurrencyLimiter;
use crate::eventsource::SharedHeaders;
use crate::transport::CancelFlag;
use crate::{Client, Error};

/// Default duration allowed for establishing the TCP and WebSocket connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default duration allowed for awaiting a method response.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeouts configured for a [`WebSocketConnection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSocketTimeouts {
    /// Maximum duration allowed for the initial TCP connection and TLS/WS handshake.
    pub connect: Duration,
    /// Maximum duration allowed for awaiting a response to a request frame.
    pub request: Duration,
}

impl Default for WebSocketTimeouts {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            request: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

enum PendingResponse {
    Success(WebSocketResponse),
    RequestError(WebSocketRequestError),
    Error(Error),
}

/// A connection abstraction for plain TCP and TLS streams.
pub(crate) enum WsStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for WsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for WsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

impl WsStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.set_read_timeout(timeout),
            Self::Tls(s) => s.get_ref().set_read_timeout(timeout),
        }
    }
}

fn tls_config() -> &'static Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    })
}

/// An established RFC 8887 WebSocket connection to a JMAP server.
pub struct WebSocketConnection {
    url: String,
    supports_push: bool,
    timeouts: WebSocketTimeouts,
    ws: Arc<Mutex<tungstenite::WebSocket<WsStream>>>,
    pending: Arc<Mutex<HashMap<String, SyncSender<PendingResponse>>>>,
    closed: Arc<AtomicBool>,
    limiter: Arc<ConcurrencyLimiter>,
    next_request_id: AtomicU64,
    reader_handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for WebSocketConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketConnection")
            .field("url", &self.url)
            .field("supports_push", &self.supports_push)
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl WebSocketConnection {
    /// Connect to the WebSocket endpoint advertised by `client`'s session with default timeouts.
    pub fn connect(client: &Client) -> Result<Self, Error> {
        Self::connect_with_timeouts(client, WebSocketTimeouts::default())
    }

    /// Connect to the WebSocket endpoint advertised by `client`'s session with explicit timeouts.
    pub fn connect_with_timeouts(
        client: &Client,
        timeouts: WebSocketTimeouts,
    ) -> Result<Self, Error> {
        let session = client.session();
        let capability = session
            .capabilities
            .get(jmap_proto::session::CAPABILITY_WEBSOCKET)
            .ok_or_else(|| {
                Error::Protocol("session does not advertise websocket capability".into())
            })?;

        let raw_url = capability
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                Error::Protocol("websocket capability is missing a non-empty url".into())
            })?;

        let supports_push = capability
            .get("supportsPush")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Apply URL rebasing if configured on Client.
        let url = if let Some(origin) = client.rebase_origin() {
            rebase_websocket_url(raw_url, origin)
        } else {
            raw_url.to_owned()
        };

        // Determine if credentials may be attached to the upgrade request.
        let http_equivalent = to_http_equivalent_url(&url);
        let may_carry = crate::url::may_carry_credentials(client.session_url(), &http_equivalent);

        let auth_header = if may_carry {
            client.authorization_header()
        } else {
            None
        };

        let parsed = parse_websocket_url(&url)?;

        let mut connected_stream = None;
        let mut last_error = None;
        for addr in parsed
            .dial
            .to_socket_addrs()
            .map_err(|e| Error::Transport(format!("failed to resolve {}: {e}", parsed.dial)))?
        {
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

        let tcp_stream = connected_stream.ok_or_else(|| {
            let msg = last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| format!("could not connect to {}", parsed.dial));
            Error::Transport(msg)
        })?;

        // Allow the full connect timeout during the initial handshake.
        let _ = tcp_stream.set_read_timeout(Some(timeouts.connect));
        let _ = tcp_stream.set_write_timeout(Some(timeouts.connect));

        let ws_stream = if parsed.is_tls {
            let server_name = ServerName::try_from(parsed.host.clone()).map_err(|e| {
                Error::Transport(format!("invalid TLS server name {}: {e}", parsed.host))
            })?;
            let conn = ClientConnection::new(Arc::clone(tls_config()), server_name)
                .map_err(|e| Error::Transport(format!("tls setup failed: {e}")))?;
            WsStream::Tls(Box::new(StreamOwned::new(conn, tcp_stream)))
        } else {
            WsStream::Plain(tcp_stream)
        };

        let req_uri = tungstenite::http::Uri::try_from(&url)
            .map_err(|e| Error::Transport(format!("invalid websocket uri: {e}")))?;

        let mut builder = tungstenite::ClientRequestBuilder::new(req_uri)
            .with_sub_protocol(jmap_proto::websocket::SUBPROTOCOL);

        let host_header =
            if (parsed.is_tls && parsed.port == 443) || (!parsed.is_tls && parsed.port == 80) {
                parsed.host.clone()
            } else {
                format!("{}:{}", parsed.host, parsed.port)
            };
        builder = builder.with_header("Host", host_header);

        if let Some(auth) = auth_header {
            builder = builder.with_header("Authorization", auth);
        }

        let ws_config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(crate::limits::MAX_API_RESPONSE_BYTES as usize))
            .max_frame_size(Some(4 * 1024 * 1024));

        let (ws, _response) =
            tungstenite::client::client_with_config(builder, ws_stream, Some(ws_config))
                .map_err(|e| Error::Transport(format!("websocket handshake failed: {e}")))?;

        // Switch to a low read timeout so the reader thread can poll cancellation and closure.
        let _ = ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(20)));

        let ws = Arc::new(Mutex::new(ws));
        let pending: Arc<Mutex<HashMap<String, SyncSender<PendingResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));

        // Concurrency limit from session capability.
        let max_concurrent = session.max_concurrent_requests();
        let limiter = Arc::new(ConcurrencyLimiter::new(max_concurrent));

        let r_ws = Arc::clone(&ws);
        let r_pending = Arc::clone(&pending);
        let r_closed = Arc::clone(&closed);

        let reader_handle = thread::spawn(move || {
            while !r_closed.load(Ordering::Relaxed) {
                let read_result = {
                    let mut guard = match r_ws.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    guard.read()
                };

                match read_result {
                    Ok(tungstenite::Message::Text(text)) => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                            match val.get("@type").and_then(|v| v.as_str()) {
                                Some("Response") => {
                                    if let Ok(mut resp) =
                                        serde_json::from_value::<WebSocketResponse>(val)
                                    {
                                        let req_id = resp.id.clone().or_else(|| {
                                            resp.extra
                                                .get("requestId")
                                                .and_then(|v| v.as_str())
                                                .map(str::to_owned)
                                        });
                                        if resp.id.is_none() && req_id.is_some() {
                                            resp.id = req_id.clone();
                                        }
                                        if let Some(id) = req_id
                                            && let Some(sender) =
                                                r_pending.lock().unwrap().remove(&id)
                                        {
                                            let _ = sender.send(PendingResponse::Success(resp));
                                        }
                                    }
                                }
                                Some("RequestError") => {
                                    if let Ok(mut err) =
                                        serde_json::from_value::<WebSocketRequestError>(val)
                                    {
                                        let req_id = err.id.clone().or_else(|| {
                                            err.extra
                                                .get("requestId")
                                                .and_then(|v| v.as_str())
                                                .map(str::to_owned)
                                        });
                                        if err.id.is_none() && req_id.is_some() {
                                            err.id = req_id.clone();
                                        }
                                        if let Some(id) = req_id
                                            && let Some(sender) =
                                                r_pending.lock().unwrap().remove(&id)
                                        {
                                            let _ = sender.send(PendingResponse::RequestError(err));
                                        }
                                    }
                                }
                                Some("StateChange") => {
                                    // Pushed state notifications handled in Step 3.
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(tungstenite::Message::Close(_)) => {
                        r_closed.store(true, Ordering::Relaxed);
                        break;
                    }
                    Ok(
                        tungstenite::Message::Ping(_)
                        | tungstenite::Message::Pong(_)
                        | tungstenite::Message::Binary(_),
                    ) => {}
                    Ok(tungstenite::Message::Frame(_)) => {}
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => {
                        r_closed.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }

            r_closed.store(true, Ordering::Relaxed);
            let mut map = r_pending.lock().unwrap();
            for (_, sender) in map.drain() {
                let _ = sender.send(PendingResponse::Error(Error::Transport(
                    "connection closed".into(),
                )));
            }
        });

        Ok(Self {
            url,
            supports_push,
            timeouts,
            ws,
            pending,
            closed,
            limiter,
            next_request_id: AtomicU64::new(1),
            reader_handle: Some(reader_handle),
        })
    }

    /// The WebSocket URL connected to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether this server connection supports push notifications.
    pub fn supports_push(&self) -> bool {
        self.supports_push
    }

    /// Whether the WebSocket connection has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Send a [`WebSocketRequest`] frame and await the matching [`WebSocketResponse`].
    pub fn send_request(&self, mut request: WebSocketRequest) -> Result<WebSocketResponse, Error> {
        if self.is_closed() {
            return Err(Error::Transport("connection is closed".into()));
        }

        let cancel = crate::transport::observed();
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }

        let _permit = self.limiter.acquire(cancel.as_ref())?;

        let id = match request.id.take() {
            Some(id) => id,
            None => format!(
                "req-{}",
                self.next_request_id.fetch_add(1, Ordering::Relaxed)
            ),
        };
        request.id = Some(id.clone());

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        {
            let mut map = self.pending.lock().unwrap();
            if self.is_closed() {
                return Err(Error::Transport("connection is closed".into()));
            }
            map.insert(id.clone(), tx);
        }

        let json = serde_json::to_string(&request).map_err(Error::Json)?;
        {
            let mut guard = self
                .ws
                .lock()
                .map_err(|_| Error::Transport("websocket lock poisoned".into()))?;
            if let Err(e) = guard.send(tungstenite::Message::Text(json.into())) {
                self.pending.lock().unwrap().remove(&id);
                return Err(Error::Transport(format!(
                    "failed to send websocket frame: {e}"
                )));
            }
        }

        let start = std::time::Instant::now();
        let timeout = self.timeouts.request;
        loop {
            if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
                self.pending.lock().unwrap().remove(&id);
                return Err(Error::Cancelled);
            }
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                self.pending.lock().unwrap().remove(&id);
                return Err(Error::Transport("websocket request timed out".into()));
            }
            let remaining = (timeout - elapsed).min(Duration::from_millis(50));
            match rx.recv_timeout(remaining) {
                Ok(PendingResponse::Success(resp)) => return Ok(resp),
                Ok(PendingResponse::RequestError(err)) => {
                    let mut prob = jmap_proto::error::RequestError::new(err.error_type);
                    if let Some(status) = err.status {
                        prob = prob.with_status(status);
                    }
                    if let Some(detail) = err.detail {
                        prob = prob.with_detail(detail);
                    }
                    prob = prob.with_extra(err.extra);
                    return Err(Error::Http {
                        status: err.status.unwrap_or(400),
                        problem: Some(prob),
                    });
                }
                Ok(PendingResponse::Error(e)) => return Err(e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Transport(
                        "connection closed while awaiting response".into(),
                    ));
                }
            }
        }
    }

    /// Invoke JMAP methods using the given capability URIs and await the results.
    pub fn call(
        &self,
        using: impl IntoIterator<Item = impl Into<String>>,
        invocations: impl IntoIterator<Item = Invocation>,
    ) -> Result<Vec<Invocation>, Error> {
        let mut request = WebSocketRequest::new(using);
        for inv in invocations {
            request = request.with_invocation(inv);
        }
        let response = self.send_request(request)?;
        Ok(response.method_responses)
    }

    /// Enable WebSocket push notifications on this connection (RFC 8887 Section 2.3.1).
    pub fn enable_push(
        &self,
        data_types: Option<&[&str]>,
        push_state: Option<&str>,
    ) -> Result<(), Error> {
        let mut enable = serde_json::json!({
            "@type": "WebSocketPushEnable",
        });
        if let Some(types) = data_types {
            enable["dataTypes"] = serde_json::json!(types);
        }
        if let Some(state) = push_state {
            enable["pushState"] = serde_json::json!(state);
        }
        let json = serde_json::to_string(&enable).map_err(Error::Json)?;
        let mut guard = self
            .ws
            .lock()
            .map_err(|_| Error::Transport("websocket lock poisoned".into()))?;
        guard
            .send(tungstenite::Message::Text(json.into()))
            .map_err(|e| Error::Transport(format!("failed to send push enable: {e}")))?;
        Ok(())
    }

    /// Disable WebSocket push notifications on this connection (RFC 8887 Section 2.3.2).
    pub fn disable_push(&self) -> Result<(), Error> {
        let disable = serde_json::json!({
            "@type": "WebSocketPushDisable",
        });
        let json = serde_json::to_string(&disable).map_err(Error::Json)?;
        let mut guard = self
            .ws
            .lock()
            .map_err(|_| Error::Transport("websocket lock poisoned".into()))?;
        guard
            .send(tungstenite::Message::Text(json.into()))
            .map_err(|e| Error::Transport(format!("failed to send push disable: {e}")))?;
        Ok(())
    }

    /// Cleanly close the WebSocket connection (RFC 6455 Section 5.5.1).
    pub fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.ws.lock() {
            let _ = guard.close(Some(tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "Normal closure".into(),
            }));
            let _ = guard.flush();
        }
        let mut map = self.pending.lock().unwrap();
        for (_, sender) in map.drain() {
            let _ = sender.send(PendingResponse::Error(Error::Transport(
                "connection closed".into(),
            )));
        }
        Ok(())
    }
}

impl Drop for WebSocketConnection {
    fn drop(&mut self) {
        let _ = self.close();
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

struct ParsedWsUrl {
    is_tls: bool,
    host: String,
    port: u16,
    dial: String,
}

fn parse_websocket_url(url: &str) -> Result<ParsedWsUrl, Error> {
    let (is_tls, rest) = if let Some(stripped) = url.strip_prefix("wss://") {
        (true, stripped)
    } else if let Some(stripped) = url.strip_prefix("ws://") {
        (false, stripped)
    } else {
        return Err(Error::Transport(format!(
            "invalid websocket scheme in URL: {url}"
        )));
    };

    let authority = match rest.find(['/', '?', '#']) {
        Some(pos) => &rest[..pos],
        None => rest,
    };

    let (host, port) = if authority.starts_with('[') {
        let closing = authority
            .find(']')
            .ok_or_else(|| Error::Transport("unclosed IPv6 bracket in URL".into()))?;
        let host = &authority[1..closing];
        let port = match authority[closing + 1..].strip_prefix(':') {
            Some(p) => p
                .parse::<u16>()
                .map_err(|e| Error::Transport(format!("invalid port: {e}")))?,
            None => {
                if is_tls {
                    443
                } else {
                    80
                }
            }
        };
        (host.to_owned(), port)
    } else if let Some((h, p)) = authority.split_once(':') {
        let port = p
            .parse::<u16>()
            .map_err(|e| Error::Transport(format!("invalid port: {e}")))?;
        (h.to_owned(), port)
    } else {
        let port = if is_tls { 443 } else { 80 };
        (authority.to_owned(), port)
    };

    let dial = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    Ok(ParsedWsUrl {
        is_tls,
        host,
        port,
        dial,
    })
}

fn to_http_equivalent_url(ws_url: &str) -> String {
    if let Some(rest) = ws_url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = ws_url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        ws_url.to_owned()
    }
}

fn rebase_websocket_url(url: &str, origin: &str) -> String {
    let http_equiv = to_http_equivalent_url(url);
    let rebased_http = crate::url::rebase_origin(&http_equiv, origin);
    if let Some(rest) = rebased_http.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = rebased_http.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        rebased_http
    }
}

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Timeouts and timing configuration for a [`WebSocketPushSubscription`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSocketPushTimeouts {
    /// Maximum duration allowed for the initial TCP connection and TLS/WS handshake.
    pub connect: Duration,
    /// Interval between keep-alive Ping control frames sent to the server.
    pub ping_interval: Duration,
    /// Maximum duration to await server activity after a Ping before considering the connection dead.
    pub ping_timeout: Duration,
    /// Initial reconnect backoff delay.
    pub initial_backoff: Duration,
    /// Maximum reconnect backoff delay.
    pub max_backoff: Duration,
}

impl Default for WebSocketPushTimeouts {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            ping_interval: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(10),
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// An item received over a [`WebSocketPushSubscription`].
#[derive(Clone, Debug, PartialEq)]
pub enum WebSocketPushItem {
    /// A parsed state change event pushed by the server.
    State(StateChange),
    /// The connection dropped and was re-established.
    Reconnected {
        /// The last pushState seen before reconnection, if any.
        push_state: Option<String>,
        /// The total number of successful reconnections so far.
        reconnect_count: u64,
    },
}

/// Internal handles bundled together across reconnect attempts.
struct PushConnectionContext {
    socket: Arc<Mutex<Option<TcpStream>>>,
    push_state_slot: Arc<Mutex<Option<String>>>,
    reconnect_count: Arc<AtomicU64>,
}

/// A long-lived RFC 8887 WebSocket push subscription on a dedicated reader thread.
pub struct WebSocketPushSubscription {
    receiver: Receiver<WebSocketPushItem>,
    cancel: CancelFlag,
    socket: Arc<Mutex<Option<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
    headers: SharedHeaders,
    push_state: Arc<Mutex<Option<String>>>,
    reconnect_count: Arc<AtomicU64>,
}

impl std::fmt::Debug for WebSocketPushSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketPushSubscription")
            .field("reconnect_count", &self.reconnect_count())
            .field("push_state", &self.push_state())
            .finish_non_exhaustive()
    }
}

impl WebSocketPushSubscription {
    /// Start listening for push notifications on the WebSocket endpoint advertised by `client`.
    pub fn start(
        client: &Client,
        types: &[&str],
        timeouts: WebSocketPushTimeouts,
    ) -> Result<Self, Error> {
        let session = client.session();
        let capability = session
            .capabilities
            .get(jmap_proto::session::CAPABILITY_WEBSOCKET)
            .ok_or_else(|| {
                Error::Protocol("session does not advertise websocket capability".into())
            })?;

        let raw_url = capability
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                Error::Protocol("websocket capability is missing a non-empty url".into())
            })?;

        let supports_push = capability
            .get("supportsPush")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !supports_push {
            return Err(Error::Protocol(
                "websocket capability does not advertise push support".into(),
            ));
        }

        let url = if let Some(origin) = client.rebase_origin() {
            rebase_websocket_url(raw_url, origin)
        } else {
            raw_url.to_owned()
        };

        let http_equivalent = to_http_equivalent_url(&url);
        let may_carry = crate::url::may_carry_credentials(client.session_url(), &http_equivalent);

        let mut headers = Vec::new();
        if may_carry && let Some(auth) = client.authorization_header() {
            headers.push(("Authorization".to_string(), auth));
        }

        let data_types = if types.is_empty() {
            None
        } else {
            Some(types.iter().map(|s| (*s).to_string()).collect())
        };

        let (sender, receiver) = std::sync::mpsc::channel();
        let socket = Arc::new(Mutex::new(None));
        let push_state = Arc::new(Mutex::new(None));
        let reconnect_count = Arc::new(AtomicU64::new(0));
        let cancel = CancelFlag::default();
        let shared_headers = SharedHeaders::new(headers);

        let ctx = PushConnectionContext {
            socket: Arc::clone(&socket),
            push_state_slot: Arc::clone(&push_state),
            reconnect_count: Arc::clone(&reconnect_count),
        };

        let thread_cancel = cancel.clone();
        let thread_headers = shared_headers.clone();
        let handle = thread::spawn(move || {
            run_push_loop(
                url,
                data_types,
                thread_headers,
                thread_cancel,
                sender,
                ctx,
                timeouts,
            );
        });

        Ok(Self {
            receiver,
            cancel,
            socket,
            handle: Some(handle),
            headers: shared_headers,
            push_state,
            reconnect_count,
        })
    }

    /// Block until a [`StateChange`] arrives or `timeout` elapses.
    ///
    /// If a reconnection occurs while waiting, this method continues waiting
    /// for a [`StateChange`] until `timeout` expires.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<StateChange> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline - now;
            match self.receiver.recv_timeout(remaining).ok()? {
                WebSocketPushItem::State(change) => return Some(change),
                WebSocketPushItem::Reconnected { .. } => continue,
            }
        }
    }

    /// Block until a [`WebSocketPushItem`] arrives or `timeout` elapses.
    pub fn recv_item_timeout(&self, timeout: Duration) -> Option<WebSocketPushItem> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Return the last pushState received from the server, if any.
    pub fn push_state(&self) -> Option<String> {
        self.push_state
            .lock()
            .expect("push state lock poisoned")
            .clone()
    }

    /// Return the total number of successful reconnections so far.
    pub fn reconnect_count(&self) -> u64 {
        self.reconnect_count.load(Ordering::SeqCst)
    }

    /// Drop the active connection from the client side by shutting down
    /// the socket, without cancelling the subscription.
    pub fn drop_connection(&self) {
        if let Some(stream) = self.socket.lock().expect("socket lock poisoned").as_ref() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Replace headers sent on future reconnect attempts.
    pub fn set_headers(&self, headers: Vec<(String, String)>) {
        self.headers.set(headers);
    }

    /// Stop listening and join the background thread.
    pub fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(stream) = self.socket.lock().expect("socket lock poisoned").take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WebSocketPushSubscription {
    fn drop(&mut self) {
        self.stop();
    }
}

struct SimpleRng(u64);

impl SimpleRng {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x853c49e6748fea9b);
        Self(nanos ^ 0x9e3779b97f4a7c15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.0
    }

    fn next_ratio(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

fn apply_jitter(base: Duration) -> Duration {
    let mut rng = SimpleRng::new();
    let factor = 0.5 + 0.5 * rng.next_ratio();
    base.mul_f64(factor)
}

fn sleep_cancellable(duration: Duration, cancel: &CancelFlag) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.is_cancelled() {
            return;
        }
        thread::sleep(CANCEL_POLL_INTERVAL.min(duration));
    }
}

struct PushStreamConfig<'a> {
    url: &'a str,
    data_types: Option<&'a [String]>,
    push_state: Option<&'a str>,
    headers: &'a [(String, String)],
    timeouts: WebSocketPushTimeouts,
}

fn run_push_loop(
    url: String,
    data_types: Option<Vec<String>>,
    headers: SharedHeaders,
    cancel: CancelFlag,
    sender: Sender<WebSocketPushItem>,
    ctx: PushConnectionContext,
    timeouts: WebSocketPushTimeouts,
) {
    let mut backoff = timeouts.initial_backoff;
    let mut connection_attempts: u64 = 0;
    while !cancel.is_cancelled() {
        let is_reconnect = connection_attempts > 0;
        let push_state = ctx.push_state_slot.lock().unwrap().clone();
        let headers_snapshot = headers.snapshot();
        let config = PushStreamConfig {
            url: &url,
            data_types: data_types.as_deref(),
            push_state: push_state.as_deref(),
            headers: &headers_snapshot,
            timeouts,
        };
        match connect_and_stream_push(config, &cancel, &sender, &ctx, is_reconnect) {
            Ok(()) => backoff = timeouts.initial_backoff,
            Err(error) => {
                tracing::debug!(%error, url, "websocket push connection ended");
            }
        }
        connection_attempts += 1;
        *ctx.socket.lock().unwrap() = None;
        if cancel.is_cancelled() {
            break;
        }
        let jittered = apply_jitter(backoff);
        sleep_cancellable(jittered, &cancel);
        backoff = (backoff * 2).min(timeouts.max_backoff);
    }
}

fn connect_and_stream_push(
    config: PushStreamConfig<'_>,
    cancel: &CancelFlag,
    sender: &Sender<WebSocketPushItem>,
    ctx: &PushConnectionContext,
    is_reconnect: bool,
) -> Result<(), Error> {
    let parsed = parse_websocket_url(config.url)?;

    let mut connected_stream = None;
    let mut last_error = None;
    for addr in parsed
        .dial
        .to_socket_addrs()
        .map_err(|e| Error::Transport(format!("failed to resolve {}: {e}", parsed.dial)))?
    {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match TcpStream::connect_timeout(&addr, config.timeouts.connect) {
            Ok(s) => {
                connected_stream = Some(s);
                break;
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }

    let tcp_stream = connected_stream.ok_or_else(|| {
        let msg = last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("could not connect to {}", parsed.dial));
        Error::Transport(msg)
    })?;

    let _ = tcp_stream.set_read_timeout(Some(config.timeouts.connect));
    let _ = tcp_stream.set_write_timeout(Some(config.timeouts.connect));

    *ctx.socket.lock().unwrap() = Some(
        tcp_stream
            .try_clone()
            .map_err(|e| Error::Transport(e.to_string()))?,
    );
    if cancel.is_cancelled() {
        return Ok(());
    }

    let ws_stream = if parsed.is_tls {
        let server_name = ServerName::try_from(parsed.host.clone()).map_err(|e| {
            Error::Transport(format!("invalid TLS server name {}: {e}", parsed.host))
        })?;
        let conn = ClientConnection::new(Arc::clone(tls_config()), server_name)
            .map_err(|e| Error::Transport(format!("tls setup failed: {e}")))?;
        WsStream::Tls(Box::new(StreamOwned::new(conn, tcp_stream)))
    } else {
        WsStream::Plain(tcp_stream)
    };

    let req_uri = tungstenite::http::Uri::try_from(config.url)
        .map_err(|e| Error::Transport(format!("invalid websocket uri: {e}")))?;

    let mut builder = tungstenite::ClientRequestBuilder::new(req_uri)
        .with_sub_protocol(jmap_proto::websocket::SUBPROTOCOL);

    let host_header =
        if (parsed.is_tls && parsed.port == 443) || (!parsed.is_tls && parsed.port == 80) {
            parsed.host.clone()
        } else {
            format!("{}:{}", parsed.host, parsed.port)
        };
    builder = builder.with_header("Host", host_header);

    for (name, value) in config.headers {
        builder = builder.with_header(name.as_str(), value.as_str());
    }

    let ws_config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(crate::limits::MAX_API_RESPONSE_BYTES as usize))
        .max_frame_size(Some(4 * 1024 * 1024));

    let (mut ws, _response) =
        tungstenite::client::client_with_config(builder, ws_stream, Some(ws_config))
            .map_err(|e| Error::Transport(format!("websocket handshake failed: {e}")))?;

    let _ = ws
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)));

    if is_reconnect {
        let count = ctx.reconnect_count.fetch_add(1, Ordering::SeqCst) + 1;
        let last_state = ctx.push_state_slot.lock().unwrap().clone();
        let _ = sender.send(WebSocketPushItem::Reconnected {
            push_state: last_state,
            reconnect_count: count,
        });
    }

    // Send WebSocketPushEnable frame
    let mut enable = serde_json::json!({
        "@type": "WebSocketPushEnable",
    });
    if let Some(types) = config.data_types {
        enable["dataTypes"] = serde_json::json!(types);
    }
    if let Some(state) = config.push_state {
        enable["pushState"] = serde_json::json!(state);
    }
    let json_str = serde_json::to_string(&enable).map_err(Error::Json)?;
    ws.send(tungstenite::Message::Text(json_str.into()))
        .map_err(|e| Error::Transport(format!("failed to send push enable: {e}")))?;

    let mut last_ping = Instant::now();
    let mut ping_in_flight: Option<Instant> = None;

    while !cancel.is_cancelled() {
        let read_result = ws.read();
        match read_result {
            Ok(tungstenite::Message::Text(text)) => {
                ping_in_flight = None;
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text)
                    && val.get("@type").and_then(|v| v.as_str()) == Some("StateChange")
                {
                    if let Some(new_state) = val.get("pushState").and_then(|v| v.as_str())
                        && !new_state.is_empty()
                    {
                        *ctx.push_state_slot.lock().unwrap() = Some(new_state.to_owned());
                    }
                    if let Ok(change) = serde_json::from_value::<jmap_proto::push::StateChange>(val)
                        && sender.send(WebSocketPushItem::State(change)).is_err()
                    {
                        return Ok(());
                    }
                }
            }
            Ok(tungstenite::Message::Pong(_)) => {
                ping_in_flight = None;
            }
            Ok(tungstenite::Message::Ping(payload)) => {
                let _ = ws.send(tungstenite::Message::Pong(payload));
                let _ = ws.flush();
            }
            Ok(tungstenite::Message::Close(_)) => {
                return Ok(());
            }
            Ok(tungstenite::Message::Binary(_) | tungstenite::Message::Frame(_)) => {}
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                let now = Instant::now();
                if let Some(sent) = ping_in_flight {
                    if now.duration_since(sent) > config.timeouts.ping_timeout {
                        return Err(Error::Transport("ping timeout: dead connection".into()));
                    }
                } else if now.duration_since(last_ping) >= config.timeouts.ping_interval {
                    if let Err(e) = ws.send(tungstenite::Message::Ping(vec![1, 2, 3, 4].into())) {
                        return Err(Error::Transport(format!("failed to send ping: {e}")));
                    }
                    let _ = ws.flush();
                    last_ping = now;
                    ping_in_flight = Some(now);
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                return Err(Error::Transport(format!("websocket read error: {e}")));
            }
        }
    }

    let _ = ws.close(Some(tungstenite::protocol::CloseFrame {
        code: tungstenite::protocol::frame::coding::CloseCode::Normal,
        reason: "Normal closure".into(),
    }));
    let _ = ws.flush();
    Ok(())
}
