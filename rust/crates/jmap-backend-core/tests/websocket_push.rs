// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for RFC 8887 WebSocket push selection, fallback, reconnect,
//! cancellation, and live delivery in `jmap_backend_core::push`.

use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gobject_sys::{G_TYPE_OBJECT, GObject, g_object_new_with_properties, g_object_unref};
use jmap_backend_core::push::{self, PushRefresh};
use jmap_client::websocket::WebSocketPushTimeouts;
use jmap_client::websocket::tungstenite;
use jmap_client::websocket::tungstenite::Message;
use jmap_client::websocket::tungstenite::handshake::server::{Callback, Request, Response};
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::mail::Mailbox;
use jmap_proto::session::{CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL};
use jmap_proto::state::State;
use jmap_proto::{Id, push::StateChange};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct HeaderCapture {
    authorization: Option<String>,
    subprotocol: Option<String>,
}

struct TestPushWsServer {
    port: u16,
    running: Arc<AtomicBool>,
    header_captures: Arc<Mutex<Vec<HeaderCapture>>>,
    connections: Arc<AtomicUsize>,
    server_thread: Option<thread::JoinHandle<()>>,
}

impl TestPushWsServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(tungstenite::WebSocket<std::net::TcpStream>, usize) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test ws listener");
        let port = listener.local_addr().unwrap().port();
        let running = Arc::new(AtomicBool::new(true));
        let header_captures = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));

        let s_running = Arc::clone(&running);
        let s_captures = Arc::clone(&header_captures);
        let s_connections = Arc::clone(&connections);
        let handler = Arc::new(handler);

        let server_thread = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set nonblocking ws listener");
            while s_running.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_nonblocking(false);

                let conn_idx = s_connections.fetch_add(1, Ordering::SeqCst);
                let captures_slot = Arc::clone(&s_captures);

                struct CaptureCallback(Arc<Mutex<Vec<HeaderCapture>>>);
                impl Callback for CaptureCallback {
                    fn on_request(
                        self,
                        request: &Request,
                        response: Response,
                    ) -> Result<Response, tungstenite::handshake::server::ErrorResponse>
                    {
                        let auth = request
                            .headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        let subproto = request
                            .headers()
                            .get("sec-websocket-protocol")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        self.0.lock().unwrap().push(HeaderCapture {
                            authorization: auth,
                            subprotocol: subproto,
                        });
                        let mut resp = response;
                        resp.headers_mut().insert(
                            "Sec-WebSocket-Protocol",
                            tungstenite::http::HeaderValue::from_static("jmap"),
                        );
                        Ok(resp)
                    }
                }

                let ws = match tungstenite::accept_hdr(stream, CaptureCallback(captures_slot)) {
                    Ok(ws) => ws,
                    Err(_) => continue,
                };

                let h = Arc::clone(&handler);
                thread::spawn(move || {
                    h(ws, conn_idx);
                });
            }
        });

        Self {
            port,
            running,
            header_captures,
            connections,
            server_thread: Some(server_thread),
        }
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/jmap/ws", self.port)
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for TestPushWsServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

struct TestSessionHttpServer {
    origin: String,
    eventsource_hits: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestSessionHttpServer {
    fn start(
        capabilities: Value,
        account_capabilities: Value,
        event_source_url: Option<String>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test http listener");
        let port = listener.local_addr().unwrap().port();
        let origin = format!("http://127.0.0.1:{port}");
        let es_url = event_source_url.unwrap_or_else(|| format!("{origin}/eventsource"));
        let eventsource_hits = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicBool::new(true));

        let s_origin = origin.clone();
        let s_es_hits = Arc::clone(&eventsource_hits);
        let s_running = Arc::clone(&running);

        let handle = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set nonblocking http listener");
            while s_running.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

                let mut buf = [0u8; 4096];
                let mut req_bytes = Vec::new();
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    req_bytes.extend_from_slice(&buf[..n]);
                    if req_bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let req_str = String::from_utf8_lossy(&req_bytes);
                let path = req_str
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                if path.starts_with("/eventsource") {
                    s_es_hits.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.flush();
                    continue;
                }

                let session = json!({
                    "username": "tester@example.com",
                    "apiUrl": format!("{s_origin}/api"),
                    "downloadUrl": format!("{s_origin}/download/{{accountId}}/{{blobId}}/{{name}}"),
                    "uploadUrl": format!("{s_origin}/upload/{{accountId}}"),
                    "eventSourceUrl": es_url,
                    "state": "session-state-1",
                    "accounts": {
                        "acc-1": {
                            "name": "tester@example.com",
                            "isPersonal": true,
                            "isReadOnly": false,
                            "accountCapabilities": account_capabilities
                        }
                    },
                    "primaryAccounts": {
                        "urn:ietf:params:jmap:core": "acc-1"
                    },
                    "capabilities": capabilities
                });
                let body = serde_json::to_vec(&session).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        Self {
            origin,
            eventsource_hits,
            running,
            handle: Some(handle),
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }

    fn eventsource_hits(&self) -> usize {
        self.eventsource_hits.load(Ordering::SeqCst)
    }
}

impl Drop for TestSessionHttpServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn core_capabilities(extra: Option<Value>) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "urn:ietf:params:jmap:core".to_owned(),
        json!({
            "maxSizeUpload": 50_000_000,
            "maxConcurrentUpload": 4,
            "maxSizeRequest": 10_000_000,
            "maxConcurrentRequests": 4,
            "maxCallsInRequest": 16,
            "maxObjectsInGet": 500,
            "maxObjectsInSet": 500,
            "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;unicode-casemap"]
        }),
    );
    if let Some(ws) = extra {
        map.insert("urn:ietf:params:jmap:websocket".to_owned(), ws);
    }
    Value::Object(map)
}

fn plain_object() -> *mut GObject {
    // SAFETY: `G_TYPE_OBJECT` is instantiable and takes no construct properties.
    let object =
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
    assert!(
        !object.is_null(),
        "g_object_new_with_properties returned NULL"
    );
    object
}

fn state_change(account: &str, kind: &str, state: &str) -> StateChange {
    let mut types = std::collections::BTreeMap::new();
    types.insert(kind.to_owned(), State::new(state));
    let mut changed = std::collections::BTreeMap::new();
    changed.insert(Id::new(account), types);
    StateChange::new(changed)
}

#[test]
fn start_for_chooses_websocket_when_advertised_with_supports_push() {
    static REFRESHES: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn schedule_refresh(_object: *mut GObject) {
        REFRESHES.fetch_add(1, Ordering::SeqCst);
    }

    REFRESHES.store(0, Ordering::SeqCst);
    let enable_frames = Arc::new(Mutex::new(Vec::new()));
    let e_frames = Arc::clone(&enable_frames);

    let ws_server = TestPushWsServer::start(move |mut ws, _conn_idx| {
        if let Ok(Message::Text(text)) = ws.read() {
            let val: Value = serde_json::from_str(&text).unwrap();
            e_frames.lock().unwrap().push(val);

            let change = json!({
                "@type": "StateChange",
                "changed": {
                    "acc-1": {
                        "ContactCard": "ws-state-1"
                    }
                }
            });
            ws.send(Message::Text(change.to_string().into())).unwrap();
        }
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let caps = core_capabilities(Some(json!({
        "url": ws_server.ws_url(),
        "supportsPush": true
    })));
    let http_server = TestSessionHttpServer::start(caps, json!({}), None);
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let backend = plain_object();
    // SAFETY: `backend` is a live `GObject` owned by this test until `g_object_unref`.
    let mut push = unsafe {
        push::start_for(
            backend,
            &client,
            &Id::new("acc-1"),
            &["ContactCard"],
            schedule_refresh,
        )
    }
    .expect("push must start when websocket supportsPush is true");

    assert!(
        push.is_websocket(),
        "start_for must prefer WebSocket when urn:ietf:params:jmap:websocket has supportsPush: true"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while REFRESHES.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "websocket StateChange never triggered start_for refresh"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let frames = enable_frames.lock().unwrap().clone();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["@type"], "WebSocketPushEnable");
    assert_eq!(frames[0]["dataTypes"], json!(["ContactCard"]));
    assert_eq!(
        http_server.eventsource_hits(),
        0,
        "EventSource must not be contacted when WebSocket push starts"
    );

    push.stop();

    // Also verify `start_for_with` forwards matched types over WebSocket.
    let (tx, rx) = channel();
    // SAFETY: `backend` is still live.
    let mut push_with = unsafe {
        push::start_for_with(
            backend,
            &client,
            &Id::new("acc-1"),
            &["ContactCard"],
            move |_obj, matched| {
                let _ = tx.send(matched.to_vec());
            },
        )
    }
    .expect("start_for_with must start over WebSocket");
    assert!(push_with.is_websocket());
    let matched = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("start_for_with must deliver matched types from WebSocket StateChange");
    assert_eq!(matched, vec!["ContactCard".to_owned()]);
    assert_eq!(http_server.eventsource_hits(), 0);

    push_with.stop();
    // SAFETY: releases this test's sole reference to `backend`.
    unsafe { g_object_unref(backend) };
}

#[test]
fn start_for_falls_back_to_eventsource_when_websocket_not_advertised_or_fails_to_start() {
    let cases = [
        ("websocket capability absent", None),
        (
            "websocket supportsPush is false",
            Some(json!({
                "url": "ws://127.0.0.1:1/jmap/ws",
                "supportsPush": false
            })),
        ),
        (
            "websocket url is invalid",
            Some(json!({
                "url": "not-a-ws-url",
                "supportsPush": true
            })),
        ),
    ];

    for (label, ws_cap) in cases {
        let es_mock = MockServer::builder().start();
        let es_url = format!("{}/eventsource", es_mock.origin());
        let http_server =
            TestSessionHttpServer::start(core_capabilities(ws_cap), json!({}), Some(es_url));
        let client = Client::builder()
            .connect(http_server.origin(), Credentials::basic("user", "pass"))
            .unwrap();

        let backend = plain_object();
        let (tx, rx) = channel();
        // SAFETY: `backend` is a live `GObject` owned by this test until `g_object_unref`.
        let mut push = unsafe {
            push::start_for_with(
                backend,
                &client,
                &Id::new("acc-1"),
                &["ContactCard"],
                move |_obj, matched| {
                    let _ = tx.send(matched.to_vec());
                },
            )
        }
        .unwrap_or_else(|| panic!("{label}: EventSource fallback must start"));

        assert!(
            !push.is_websocket(),
            "{label}: fallback subscription must use EventSource"
        );

        es_mock.wait_for_event_source_subscriber(Duration::from_secs(5));
        es_mock.push_state_change(&state_change("acc-1", "ContactCard", "es-1"));

        let matched = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| {
            panic!("{label}: EventSource fallback never delivered StateChange")
        });
        assert_eq!(matched, vec!["ContactCard".to_owned()]);

        push.stop();
        // SAFETY: releases this test's sole reference to `backend`.
        unsafe { g_object_unref(backend) };
    }
}

#[test]
fn start_for_returns_none_on_cyrus_even_when_websocket_is_advertised() {
    unsafe extern "C" fn never_called(_object: *mut GObject) {
        panic!("Cyrus account must never schedule a push refresh");
    }

    let ws_server = TestPushWsServer::start(|mut ws, _| {
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let caps = core_capabilities(Some(json!({
        "url": ws_server.ws_url(),
        "supportsPush": true
    })));
    let account_caps = json!({
        CAPABILITY_CYRUS_MAIL: {}
    });
    let http_server = TestSessionHttpServer::start(caps, account_caps, None);
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let backend = plain_object();
    // SAFETY: `backend` is a live `GObject` owned by this test until `g_object_unref`.
    let push = unsafe {
        push::start_for(
            backend,
            &client,
            &Id::new("acc-1"),
            &["Email", "Mailbox"],
            never_called,
        )
    };
    assert!(
        push.is_none(),
        "Cyrus accounts must keep push disabled even when websocket supportsPush is true"
    );

    thread::sleep(Duration::from_millis(100));
    assert_eq!(ws_server.connection_count(), 0);
    assert_eq!(http_server.eventsource_hits(), 0);

    // SAFETY: releases this test's sole reference to `backend`.
    unsafe { g_object_unref(backend) };
}

#[test]
fn websocket_push_reconnects_after_drop_connection_resumes_push_state_and_refreshes_headers() {
    let enable_frames = Arc::new(Mutex::new(Vec::new()));
    let e_frames = Arc::clone(&enable_frames);

    let ws_server = TestPushWsServer::start(move |mut ws, conn_idx| {
        if let Ok(Message::Text(text)) = ws.read() {
            let val: Value = serde_json::from_str(&text).unwrap();
            e_frames.lock().unwrap().push(val);

            if conn_idx == 0 {
                let first = json!({
                    "@type": "StateChange",
                    "changed": {
                        "acc-1": {
                            "ContactCard": "s1"
                        }
                    },
                    "pushState": "ps-1"
                });
                ws.send(Message::Text(first.to_string().into())).unwrap();
            } else {
                // Repeat "s1" (must not cause another refresh), then send "s2" (must refresh).
                let repeat = json!({
                    "@type": "StateChange",
                    "changed": {
                        "acc-1": {
                            "ContactCard": "s1"
                        }
                    },
                    "pushState": "ps-1"
                });
                ws.send(Message::Text(repeat.to_string().into())).unwrap();
                let next = json!({
                    "@type": "StateChange",
                    "changed": {
                        "acc-1": {
                            "ContactCard": "s2"
                        }
                    },
                    "pushState": "ps-2"
                });
                ws.send(Message::Text(next.to_string().into())).unwrap();
            }
        }
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let caps = core_capabilities(Some(json!({
        "url": ws_server.ws_url(),
        "supportsPush": true
    })));
    let http_server = TestSessionHttpServer::start(caps, json!({}), None);
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let timeouts = WebSocketPushTimeouts {
        connect: Duration::from_secs(5),
        ping_interval: Duration::from_secs(30),
        ping_timeout: Duration::from_secs(5),
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(50),
    };

    let refreshes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&refreshes);
    let mut push = PushRefresh::start_for_client_with_timeouts(
        &client,
        &Id::new("acc-1"),
        &["ContactCard"],
        timeouts,
        move |_types| {
            counter.fetch_add(1, Ordering::SeqCst);
        },
    )
    .expect("push must start over WebSocket");
    assert!(push.is_websocket());

    let deadline = Instant::now() + Duration::from_secs(5);
    while refreshes.load(Ordering::SeqCst) < 1 {
        assert!(Instant::now() < deadline, "first refresh never arrived");
        thread::sleep(Duration::from_millis(10));
    }

    push.set_headers(vec![(
        "Authorization".to_owned(),
        "Bearer fresh-token".to_owned(),
    )]);
    push.drop_connection();

    let deadline = Instant::now() + Duration::from_secs(5);
    while refreshes.load(Ordering::SeqCst) < 2 {
        assert!(
            Instant::now() < deadline,
            "post-reconnect refresh never arrived"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Give any incorrectly counted repeat ("s1") time to arrive.
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        2,
        "repeated state across reconnect must not trigger an extra refresh"
    );

    let captures = ws_server.header_captures.lock().unwrap().clone();
    assert!(captures.len() >= 2);
    assert_eq!(
        captures[0].authorization.as_deref(),
        Some("Basic dXNlcjpwYXNz")
    );
    assert_eq!(captures[0].subprotocol.as_deref(), Some("jmap"));
    assert_eq!(
        captures[1].authorization.as_deref(),
        Some("Bearer fresh-token")
    );

    let frames = enable_frames.lock().unwrap().clone();
    assert!(frames.len() >= 2);
    assert!(frames[0].get("pushState").is_none());
    assert_eq!(frames[1]["pushState"], "ps-1");
    assert_eq!(frames[1]["dataTypes"], json!(["ContactCard"]));

    push.stop();
}

#[test]
fn websocket_push_stops_cleanly_and_ignores_pushes_after_stop_or_backend_unref() {
    let ws_ready = Arc::new(AtomicBool::new(false));
    let ready_flag = Arc::clone(&ws_ready);
    let send_after_stop = Arc::new(AtomicBool::new(false));
    let send_trigger = Arc::clone(&send_after_stop);

    let ws_server = TestPushWsServer::start(move |mut ws, _conn_idx| {
        if let Ok(Message::Text(_)) = ws.read() {
            ready_flag.store(true, Ordering::SeqCst);
        }
        while !send_trigger.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
        }
        let change = json!({
            "@type": "StateChange",
            "changed": {
                "acc-1": {
                    "ContactCard": "after-stop"
                }
            }
        });
        let _ = ws.send(Message::Text(change.to_string().into()));
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let caps = core_capabilities(Some(json!({
        "url": ws_server.ws_url(),
        "supportsPush": true
    })));
    let http_server = TestSessionHttpServer::start(caps, json!({}), None);
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let backend = plain_object();
    let refreshes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&refreshes);
    // SAFETY: `backend` is a live `GObject` owned by this test until `g_object_unref`.
    let mut push = unsafe {
        push::start_for_with(
            backend,
            &client,
            &Id::new("acc-1"),
            &["ContactCard"],
            move |_obj, _types| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        )
    }
    .expect("push must start over WebSocket");
    assert!(push.is_websocket());

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ws_ready.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "websocket push never enabled");
        thread::sleep(Duration::from_millis(10));
    }

    let started = Instant::now();
    push.stop();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "stopping a WebSocket push must return promptly"
    );

    send_after_stop.store(true, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(250));
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        0,
        "pushes after stop() must not invoke refresh"
    );
    // SAFETY: releases this test's sole reference to `backend`.
    unsafe { g_object_unref(backend) };

    // Part B: backend GObject finalized before second StateChange arrives.
    let send_second = Arc::new(AtomicBool::new(false));
    let trigger_second = Arc::clone(&send_second);
    let ws_server_b = TestPushWsServer::start(move |mut ws, _conn_idx| {
        if let Ok(Message::Text(_)) = ws.read() {
            let first = json!({
                "@type": "StateChange",
                "changed": {
                    "acc-1": {
                        "ContactCard": "before-unref"
                    }
                }
            });
            ws.send(Message::Text(first.to_string().into())).unwrap();
            while !trigger_second.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(10));
            }
            let second = json!({
                "@type": "StateChange",
                "changed": {
                    "acc-1": {
                        "ContactCard": "after-unref"
                    }
                }
            });
            let _ = ws.send(Message::Text(second.to_string().into()));
        }
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let caps_b = core_capabilities(Some(json!({
        "url": ws_server_b.ws_url(),
        "supportsPush": true
    })));
    let http_server_b = TestSessionHttpServer::start(caps_b, json!({}), None);
    let client_b = Client::builder()
        .connect(http_server_b.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let backend_b = plain_object();
    let refreshes_b = Arc::new(AtomicUsize::new(0));
    let counter_b = Arc::clone(&refreshes_b);
    // SAFETY: `backend_b` is a live `GObject` owned by this test until `g_object_unref`.
    let mut push_b = unsafe {
        push::start_for_with(
            backend_b,
            &client_b,
            &Id::new("acc-1"),
            &["ContactCard"],
            move |_obj, _types| {
                counter_b.fetch_add(1, Ordering::SeqCst);
            },
        )
    }
    .expect("push must start over WebSocket");

    let deadline = Instant::now() + Duration::from_secs(5);
    while refreshes_b.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "first push never arrived");
        thread::sleep(Duration::from_millis(10));
    }

    // SAFETY: releases the sole strong reference to `backend_b` so WeakBackend stops invoking action.
    unsafe { g_object_unref(backend_b) };
    send_second.store(true, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(250));
    assert_eq!(
        refreshes_b.load(Ordering::SeqCst),
        1,
        "action must not run after backend GObject is finalized"
    );
    push_b.stop();
}

fn connect_live_write_client() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("set JMAP_LIVE_SERVER_WRITE_PASSWORD alongside JMAP_LIVE_SERVER_WRITE_USER");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not connect live write client to JMAP_LIVE_SERVER_URL"),
    )
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn second_client_change_reaches_push_refresh_over_websocket_against_the_real_server() {
    let Some(client1) = connect_live_write_client() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping live WebSocket PushRefresh test"
        );
        return;
    };
    let client2 = connect_live_write_client().expect("connect second live write client");

    let account_id = client1
        .primary_account(CAPABILITY_MAIL)
        .expect("live account needs mail capability");
    let is_cyrus = client1
        .session()
        .accounts
        .get(&account_id)
        .is_some_and(|account| account.has_capability(CAPABILITY_CYRUS_MAIL));

    let backend = plain_object();
    let (tx, rx) = channel();
    // SAFETY: `backend` is a live `GObject` owned by this test until `g_object_unref`.
    let started = unsafe {
        push::start_for_with(
            backend,
            &client1,
            &account_id,
            &["Mailbox"],
            move |_obj, matched| {
                let _ = tx.send(matched.to_vec());
            },
        )
    };

    if is_cyrus {
        assert!(
            started.is_none(),
            "Cyrus accounts must keep PushRefresh disabled (Finding 17 and Finding 22)"
        );
        eprintln!(
            "live Cyrus check: push::start_for_with returned None as required (Finding 17 / Finding 22)"
        );
        // SAFETY: releases this test's sole reference to `backend`.
        unsafe { g_object_unref(backend) };
        return;
    }

    let mut push = started.expect("Stalwart live server must start PushRefresh over WebSocket");
    assert!(
        push.is_websocket(),
        "PushRefresh must use WebSocket when session advertises supportsPush: true"
    );

    thread::sleep(Duration::from_millis(300));

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("agent-b72-wspush-{suffix}");
    let created = client2
        .mailbox_create(&account_id, &Mailbox::new(name.clone()))
        .expect("second client Mailbox/set create failed");
    let mailbox_id = created.id.clone().expect("server assigned mailbox id");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_create = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
            Ok(matched) if matched.iter().any(|t| t == "Mailbox") => {
                saw_create = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    assert!(
        saw_create,
        "PushRefresh action never received Mailbox StateChange over WebSocket after second-client create"
    );

    client2
        .mailbox_destroy(&account_id, &mailbox_id)
        .expect("second client Mailbox/set destroy failed");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_destroy = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
            Ok(matched) if matched.iter().any(|t| t == "Mailbox") => {
                saw_destroy = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    assert!(
        saw_destroy,
        "PushRefresh action never received Mailbox StateChange over WebSocket after second-client destroy"
    );

    eprintln!(
        "live Stalwart WebSocket PushRefresh delivered create and destroy StateChange for mailbox {name}"
    );
    push.stop();
    // SAFETY: releases this test's sole reference to `backend`.
    unsafe { g_object_unref(backend) };
}
