// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for RFC 8887 JMAP over WebSocket push notifications in `jmap-client`.

#![cfg(feature = "websocket")]

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use jmap_client::websocket::{WebSocketPushItem, WebSocketPushTimeouts};
use jmap_client::{Client, Credentials};
use jmap_proto::State;
use serde_json::json;
use tungstenite::Message;
use tungstenite::handshake::server::{Callback, Request, Response};

struct HeaderCapture {
    authorization: Option<String>,
    subprotocol: Option<String>,
}

struct TestPushWsServer {
    port: u16,
    running: Arc<AtomicBool>,
    header_captures: Arc<Mutex<Vec<HeaderCapture>>>,
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
        let connection_counter = Arc::new(AtomicUsize::new(0));

        let s_running = Arc::clone(&running);
        let s_captures = Arc::clone(&header_captures);
        let handler = Arc::new(handler);

        let server_thread = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set nonblocking listener");
            while s_running.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };

                let conn_idx = connection_counter.fetch_add(1, Ordering::SeqCst);
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
            server_thread: Some(server_thread),
        }
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/jmap/ws", self.port)
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

struct TestHttpServer {
    origin: String,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestHttpServer {
    fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_session_server(ws_url: String) -> TestHttpServer {
    let server =
        Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind session http server"));
    let origin = format!("http://{}", server.server_addr());
    let running = Arc::new(AtomicBool::new(true));

    let s_server = Arc::clone(&server);
    let s_running = Arc::clone(&running);
    let s_ws_url = ws_url;
    let s_origin = origin.clone();

    let handle = thread::spawn(move || {
        while s_running.load(Ordering::Relaxed) {
            let request = match s_server.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(req)) => req,
                Ok(None) => continue,
                Err(_) => break,
            };

            let capabilities = json!({
                "urn:ietf:params:jmap:core": {
                    "maxSizeUpload": 50000000,
                    "maxConcurrentUpload": 4,
                    "maxSizeRequest": 10000000,
                    "maxConcurrentRequests": 4,
                    "maxCallsInRequest": 16,
                    "maxObjectsInGet": 500,
                    "maxObjectsInSet": 500,
                    "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;unicode-casemap"]
                },
                "urn:ietf:params:jmap:websocket": {
                    "url": s_ws_url,
                    "supportsPush": true
                }
            });

            let session = json!({
                "username": "tester@example.com",
                "apiUrl": format!("{s_origin}/api"),
                "downloadUrl": format!("{s_origin}/download/{{accountId}}/{{blobId}}/{{name}}"),
                "uploadUrl": format!("{s_origin}/upload/{{accountId}}"),
                "eventSourceUrl": format!("{s_origin}/eventsource/"),
                "state": "session-state-1",
                "accounts": {
                    "acc-1": {
                        "name": "tester@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {}
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:core": "acc-1"
                },
                "capabilities": capabilities
            });

            let body = serde_json::to_vec(&session).unwrap();
            let response = tiny_http::Response::new(
                tiny_http::StatusCode(200),
                vec![
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                ],
                body.as_slice(),
                Some(body.len()),
                None,
            );
            let _ = request.respond(response);
        }
    });

    TestHttpServer {
        origin,
        running,
        handle: Some(handle),
    }
}

#[test]
fn websocket_push_enables_with_data_types_and_receives_state_change() {
    let received_enable = Arc::new(Mutex::new(None));
    let r_enable = Arc::clone(&received_enable);

    let ws_server = TestPushWsServer::start(move |mut ws, _conn_idx| {
        if let Ok(Message::Text(text)) = ws.read() {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            *r_enable.lock().unwrap() = Some(val);

            // Emit a StateChange frame
            let change = json!({
                "@type": "StateChange",
                "changed": {
                    "acc-1": {
                        "Mailbox": "state-mbx-1",
                        "Email": "state-email-1"
                    }
                }
            });
            ws.send(Message::Text(change.to_string().into())).unwrap();
        }

        // Keep connection open until closed
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url());
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let mut sub = client.websocket_push(&["Mailbox", "Email"]).unwrap();

    // Verify StateChange delivery
    let change = sub
        .recv_timeout(Duration::from_secs(3))
        .expect("state change must arrive");
    let acc_changes = change.changed.get(&jmap_proto::Id::new("acc-1")).unwrap();
    assert_eq!(
        acc_changes.get("Mailbox"),
        Some(&State::from("state-mbx-1"))
    );
    assert_eq!(
        acc_changes.get("Email"),
        Some(&State::from("state-email-1"))
    );

    // Verify WebSocketPushEnable frame sent to server
    let enable_frame = received_enable.lock().unwrap().clone().unwrap();
    assert_eq!(enable_frame["@type"], "WebSocketPushEnable");
    assert_eq!(enable_frame["dataTypes"], json!(["Mailbox", "Email"]));
    assert!(enable_frame.get("pushState").is_none());

    sub.stop();
}

#[test]
fn websocket_push_resumes_with_push_state_on_reconnect() {
    let enable_frames = Arc::new(Mutex::new(Vec::new()));
    let e_frames = Arc::clone(&enable_frames);

    let ws_server = TestPushWsServer::start(move |mut ws, conn_idx| {
        if let Ok(Message::Text(text)) = ws.read() {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            e_frames.lock().unwrap().push(val);

            if conn_idx == 0 {
                // First connection: send StateChange with pushState then close connection
                let change = json!({
                    "@type": "StateChange",
                    "changed": {
                        "acc-1": {
                            "Mailbox": "state-1"
                        }
                    },
                    "pushState": "token-ps-42"
                });
                ws.send(Message::Text(change.to_string().into())).unwrap();
                thread::sleep(Duration::from_millis(50));
                let _ = ws.close(None);
            } else {
                // Reconnection: send second StateChange with updated pushState
                let change = json!({
                    "@type": "StateChange",
                    "changed": {
                        "acc-1": {
                            "Mailbox": "state-2"
                        }
                    },
                    "pushState": "token-ps-43"
                });
                ws.send(Message::Text(change.to_string().into())).unwrap();
                while let Ok(msg) = ws.read() {
                    if msg.is_close() {
                        break;
                    }
                }
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url());
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let timeouts = WebSocketPushTimeouts {
        connect: Duration::from_secs(5),
        ping_interval: Duration::from_secs(30),
        ping_timeout: Duration::from_secs(5),
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(100),
    };

    let mut sub = client
        .websocket_push_with_timeouts(&["Mailbox"], timeouts)
        .unwrap();

    // 1. First event arrives with initial pushState
    let first = sub
        .recv_item_timeout(Duration::from_secs(3))
        .expect("first item arrives");
    match first {
        WebSocketPushItem::State(change) => {
            let acc = change.changed.get(&jmap_proto::Id::new("acc-1")).unwrap();
            assert_eq!(acc.get("Mailbox"), Some(&State::from("state-1")));
        }
        _ => panic!("expected state change first"),
    }
    assert_eq!(sub.push_state().as_deref(), Some("token-ps-42"));

    // 2. Observe Reconnected notification carrying the pushState
    let second = sub
        .recv_item_timeout(Duration::from_secs(5))
        .expect("reconnected item arrives");
    match second {
        WebSocketPushItem::Reconnected {
            push_state,
            reconnect_count,
        } => {
            assert_eq!(push_state.as_deref(), Some("token-ps-42"));
            assert_eq!(reconnect_count, 1);
        }
        WebSocketPushItem::State(s) => panic!("unexpected state item: {:?}", s),
    }

    // 3. Post-reconnect StateChange arrives with updated pushState
    let third = sub
        .recv_item_timeout(Duration::from_secs(3))
        .expect("post-reconnect change arrives");
    match third {
        WebSocketPushItem::State(change) => {
            let acc = change.changed.get(&jmap_proto::Id::new("acc-1")).unwrap();
            assert_eq!(acc.get("Mailbox"), Some(&State::from("state-2")));
        }
        _ => panic!("expected state change after reconnect"),
    }
    assert_eq!(sub.push_state().as_deref(), Some("token-ps-43"));

    // Verify second WebSocketPushEnable sent the recorded pushState
    let frames = enable_frames.lock().unwrap().clone();
    assert!(frames.len() >= 2);
    assert_eq!(frames[0].get("pushState"), None);
    assert_eq!(frames[1].get("pushState").unwrap(), "token-ps-42");

    sub.stop();
}

#[test]
fn websocket_push_ping_pong_keepalive_and_dead_connection_detection() {
    let pings_seen = Arc::new(AtomicUsize::new(0));
    let p_seen = Arc::clone(&pings_seen);

    let ws_server = TestPushWsServer::start(move |mut ws, conn_idx| {
        // Read WebSocketPushEnable
        let _ = ws.read();

        if conn_idx == 0 {
            // First connection: answer first ping with Pong, then go dead
            while let Ok(msg) = ws.read() {
                if let Message::Ping(p) = msg {
                    p_seen.fetch_add(1, Ordering::SeqCst);
                    // Send pong for the first ping
                    ws.send(Message::Pong(p)).unwrap();
                    break;
                }
            }
            // Sleep and simulate a dead socket without sending further pongs or frames
            thread::sleep(Duration::from_millis(500));
        } else {
            // Reconnected connection: answer pings normally
            while let Ok(msg) = ws.read() {
                if let Message::Ping(p) = msg {
                    p_seen.fetch_add(1, Ordering::SeqCst);
                    let _ = ws.send(Message::Pong(p));
                } else if msg.is_close() {
                    break;
                }
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url());
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let timeouts = WebSocketPushTimeouts {
        connect: Duration::from_secs(5),
        ping_interval: Duration::from_millis(40),
        ping_timeout: Duration::from_millis(100),
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(50),
    };

    let mut sub = client
        .websocket_push_with_timeouts(&["Mailbox"], timeouts)
        .unwrap();

    // Await keepalive failure and subsequent reconnection
    let item = sub
        .recv_item_timeout(Duration::from_secs(3))
        .expect("reconnection item must arrive after dead connection detection");

    match item {
        WebSocketPushItem::Reconnected {
            reconnect_count, ..
        } => {
            assert!(reconnect_count >= 1);
        }
        WebSocketPushItem::State(_) => panic!("unexpected state change"),
    }

    assert!(sub.reconnect_count() >= 1);
    assert!(pings_seen.load(Ordering::SeqCst) >= 1);

    sub.stop();
}

#[test]
fn websocket_push_set_headers_updates_credentials_on_reconnect() {
    let ws_server = TestPushWsServer::start(move |mut ws, _conn_idx| {
        let _ = ws.read();
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url());
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

    let mut sub = client
        .websocket_push_with_timeouts(&["Mailbox"], timeouts)
        .unwrap();

    // Verify initial connection captured Authorization and subprotocol
    thread::sleep(Duration::from_millis(50));
    {
        let captures = ws_server.header_captures.lock().unwrap();
        assert!(!captures.is_empty());
        assert_eq!(
            captures[0].authorization.as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
        assert_eq!(captures[0].subprotocol.as_deref(), Some("jmap"));
    }

    // Update headers and trigger connection drop
    sub.set_headers(vec![(
        "Authorization".to_string(),
        "Bearer new-token-123".to_string(),
    )]);
    sub.drop_connection();

    // Await reconnection item
    let reconnected = sub
        .recv_item_timeout(Duration::from_secs(3))
        .expect("reconnected event arrives");
    assert!(matches!(reconnected, WebSocketPushItem::Reconnected { .. }));

    // Verify reconnected attempt used the new authorization header
    {
        let captures = ws_server.header_captures.lock().unwrap();
        assert!(captures.len() >= 2);
        assert_eq!(
            captures.last().unwrap().authorization.as_deref(),
            Some("Bearer new-token-123")
        );
    }

    sub.stop();
}

#[test]
fn websocket_push_clean_stop_terminates_promptly() {
    let ws_server = TestPushWsServer::start(move |mut ws, _conn_idx| {
        let _ = ws.read();
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url());
    let client = Client::builder()
        .connect(http_server.origin(), Credentials::basic("user", "pass"))
        .unwrap();

    let mut sub = client.websocket_push(&["Mailbox"]).unwrap();
    thread::sleep(Duration::from_millis(50));

    let start = std::time::Instant::now();
    sub.stop();
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "stop must terminate promptly"
    );

    assert_eq!(sub.recv_timeout(Duration::from_millis(50)), None);
}
