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
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use jmap_proto::request::Invocation;
use jmap_proto::websocket::{WebSocketRequest, WebSocketRequestError, WebSocketResponse};

use crate::concurrency::ConcurrencyLimiter;
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
