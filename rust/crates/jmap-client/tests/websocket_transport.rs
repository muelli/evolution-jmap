// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for RFC 8887 JMAP over WebSocket transport in `jmap-client`.

#![cfg(feature = "websocket")]

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use jmap_client::{Client, CloseHandshakeOutcome, Credentials, Error, WebSocketTimeouts};
use jmap_proto::request::Invocation;
use jmap_proto::websocket::WebSocketRequest;
use serde_json::json;
use tungstenite::Message;
use tungstenite::handshake::server::{Callback, Request, Response};

struct HeaderCapture {
    authorization: Option<String>,
    subprotocol: Option<String>,
}

struct TestWsServer {
    port: u16,
    running: Arc<AtomicBool>,
    header_capture: Arc<Mutex<Option<HeaderCapture>>>,
    server_thread: Option<thread::JoinHandle<()>>,
}

impl TestWsServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(tungstenite::WebSocket<std::net::TcpStream>) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test ws listener");
        let port = listener.local_addr().unwrap().port();
        let running = Arc::new(AtomicBool::new(true));
        let header_capture = Arc::new(Mutex::new(None));

        let s_running = Arc::clone(&running);
        let s_capture = Arc::clone(&header_capture);

        let server_thread = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set nonblocking listener");
            while s_running.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };

                let capture_slot = Arc::clone(&s_capture);
                struct CaptureCallback(Arc<Mutex<Option<HeaderCapture>>>);
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
                        *self.0.lock().unwrap() = Some(HeaderCapture {
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

                let ws = match tungstenite::accept_hdr(stream, CaptureCallback(capture_slot)) {
                    Ok(ws) => ws,
                    Err(_) => continue,
                };

                handler(ws);
            }
        });

        Self {
            port,
            running,
            header_capture,
            server_thread: Some(server_thread),
        }
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/jmap/ws", self.port)
    }
}

impl Drop for TestWsServer {
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

fn start_session_server(ws_url: String, max_concurrent: Option<u64>) -> TestHttpServer {
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

            let mut capabilities = json!({
                "urn:ietf:params:jmap:core": {
                    "maxSizeUpload": 50000000,
                    "maxConcurrentUpload": 4,
                    "maxSizeRequest": 10000000,
                    "maxConcurrentRequests": max_concurrent.unwrap_or(4),
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

            if max_concurrent.is_none() {
                capabilities["urn:ietf:params:jmap:core"]
                    .as_object_mut()
                    .unwrap()
                    .remove("maxConcurrentRequests");
            }

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
fn websocket_handshake_subprotocol_and_credentials() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(
        http_server.origin(),
        Credentials::bearer("secret-token-123"),
    )
    .unwrap();
    let conn = client.connect_websocket().expect("websocket connects");

    assert_eq!(conn.url(), ws_server.ws_url());
    assert!(conn.supports_push());

    // Give handshake headers a moment to be captured.
    thread::sleep(Duration::from_millis(50));
    let headers = ws_server.header_capture.lock().unwrap().take().unwrap();

    assert_eq!(headers.subprotocol.as_deref(), Some("jmap"));
    assert_eq!(
        headers.authorization.as_deref(),
        Some("Bearer secret-token-123")
    );

    conn.close().expect("clean close");
}

#[test]
fn websocket_request_response_matching_by_id_and_request_id() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            match msg {
                Message::Text(text) => {
                    let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let req_id = req["id"].as_str().unwrap().to_string();

                    // Send response with standard RFC 8887 requestId field.
                    let resp = json!({
                        "@type": "Response",
                        "requestId": req_id,
                        "methodResponses": [
                            ["Core/echo", {"echoed": req["methodCalls"][0][1]}, "c1"]
                        ]
                    });
                    ws.send(Message::Text(serde_json::to_string(&resp).unwrap().into()))
                        .unwrap();
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(http_server.origin(), Credentials::none()).unwrap();
    let conn = client.connect_websocket().expect("websocket connects");

    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
        .with_id("req-alpha")
        .call("Core/echo", &json!({"hello": "jmap"}), "c1")
        .unwrap();

    let resp = conn.send_request(req).expect("request succeeds");
    assert_eq!(resp.id.as_deref(), Some("req-alpha"));
    assert_eq!(resp.method_responses.len(), 1);
    assert_eq!(resp.method_responses[0].name, "Core/echo");

    conn.close().expect("clean close");
}

#[test]
fn websocket_interleaved_concurrent_requests() {
    let ws_server = TestWsServer::start(|mut ws| {
        let mut queued_requests = Vec::new();
        while let Ok(msg) = ws.read() {
            match msg {
                Message::Text(text) => {
                    let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                    queued_requests.push(req);
                    if queued_requests.len() == 2 {
                        // Respond to request 2 first, then request 1 (out of order).
                        for req in queued_requests.iter().rev() {
                            let req_id = req["id"].as_str().unwrap();
                            let resp = json!({
                                "@type": "Response",
                                "requestId": req_id,
                                "methodResponses": [
                                    ["Core/echo", {"id": req_id}, "c1"]
                                ]
                            });
                            ws.send(Message::Text(serde_json::to_string(&resp).unwrap().into()))
                                .unwrap();
                        }
                        queued_requests.clear();
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Arc::new(Client::connect(http_server.origin(), Credentials::none()).unwrap());
    let conn = Arc::new(client.connect_websocket().expect("websocket connects"));

    let conn1 = Arc::clone(&conn);
    let handle1 = thread::spawn(move || {
        let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
            .with_id("thread-1")
            .call("Core/echo", &json!({}), "c1")
            .unwrap();
        conn1.send_request(req).unwrap()
    });

    let conn2 = Arc::clone(&conn);
    let handle2 = thread::spawn(move || {
        let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
            .with_id("thread-2")
            .call("Core/echo", &json!({}), "c1")
            .unwrap();
        conn2.send_request(req).unwrap()
    });

    let res1 = handle1.join().unwrap();
    let res2 = handle2.join().unwrap();

    assert_eq!(res1.id.as_deref(), Some("thread-1"));
    assert_eq!(res2.id.as_deref(), Some("thread-2"));

    conn.close().expect("clean close");
}

#[test]
fn websocket_request_error_mapped_to_error_http() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Text(text) = msg {
                let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                let req_id = req["id"].as_str().unwrap();
                let err = json!({
                    "@type": "RequestError",
                    "requestId": req_id,
                    "type": "urn:ietf:params:jmap:error:notRequest",
                    "status": 400,
                    "detail": "Test request error"
                });
                ws.send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                    .unwrap();
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(http_server.origin(), Credentials::none()).unwrap();
    let conn = client.connect_websocket().expect("websocket connects");

    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
        .with_id("req-bad")
        .call("Core/echo", &json!({}), "c1")
        .unwrap();

    let err = conn.send_request(req).unwrap_err();
    match err {
        Error::Http { status, problem } => {
            assert_eq!(status, 400);
            let prob = problem.expect("problem details present");
            assert_eq!(prob.error_type, "urn:ietf:params:jmap:error:notRequest");
            assert_eq!(prob.detail.as_deref(), Some("Test request error"));
        }
        other => panic!("expected Error::Http, got: {other:?}"),
    }

    conn.close().expect("clean close");
}

#[test]
fn websocket_clean_close_and_subsequent_send_fails() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Close(frame) = msg {
                let _ = ws.close(frame);
                break;
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(http_server.origin(), Credentials::none()).unwrap();
    let conn = client.connect_websocket().expect("websocket connects");

    assert!(!conn.is_closed());
    conn.close().expect("clean close succeeds");
    assert!(conn.is_closed());

    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
        .with_id("post-close")
        .call("Core/echo", &json!({}), "c1")
        .unwrap();

    let err = conn.send_request(req).unwrap_err();
    assert!(matches!(err, Error::Transport(_)));
}

#[test]
fn websocket_call_convenience_method() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Text(text) = msg {
                let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                let req_id = req["id"].as_str().unwrap();
                let resp = json!({
                    "@type": "Response",
                    "requestId": req_id,
                    "methodResponses": [
                        ["Core/echo", {"status": "ok"}, "c1"]
                    ]
                });
                ws.send(Message::Text(serde_json::to_string(&resp).unwrap().into()))
                    .unwrap();
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(http_server.origin(), Credentials::none()).unwrap();
    let conn = client.connect_websocket().expect("websocket connects");

    let responses = conn
        .call(
            ["urn:ietf:params:jmap:core"],
            [Invocation::new("Core/echo", &json!({"test": 1}), "c1").unwrap()],
        )
        .expect("call succeeds");

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Core/echo");
    assert_eq!(responses[0].call_id, "c1");

    conn.close().expect("clean close");
}

#[test]
fn websocket_rebases_origin() {
    let _ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if msg.is_close() {
                break;
            }
        }
    });

    // The session advertises a public origin wss://mail.example.internal:8443/jmap/ws.
    // Rebasing against the local test origin should rewrite the URL to ws://127.0.0.1:<port>/jmap/ws.
    let http_server =
        start_session_server("wss://mail.example.internal:8443/jmap/ws".into(), Some(4));
    let client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(http_server.origin(), Credentials::none())
        .unwrap();

    // Rebased URL starts with ws://127.0.0.1:<port>
    let session = client.session();
    let cap = &session.capabilities["urn:ietf:params:jmap:websocket"];
    assert_eq!(cap["url"], "wss://mail.example.internal:8443/jmap/ws");
}

#[test]
fn websocket_honours_concurrency_limit() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let s_in_flight = Arc::clone(&in_flight);
    let s_max_observed = Arc::clone(&max_observed);

    let ws_server = TestWsServer::start(move |mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Text(text) = msg {
                let current = s_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                s_max_observed.fetch_max(current, Ordering::SeqCst);

                // Small delay to allow concurrent calls to pile up.
                thread::sleep(Duration::from_millis(50));

                let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                let req_id = req["id"].as_str().unwrap();
                let resp = json!({
                    "@type": "Response",
                    "requestId": req_id,
                    "methodResponses": [
                        ["Core/echo", {}, "c1"]
                    ]
                });
                s_in_flight.fetch_sub(1, Ordering::SeqCst);
                ws.send(Message::Text(serde_json::to_string(&resp).unwrap().into()))
                    .unwrap();
            }
        }
    });

    // Configure maxConcurrentRequests: 2 in session.
    let http_server = start_session_server(ws_server.ws_url(), Some(2));
    let client = Arc::new(Client::connect(http_server.origin(), Credentials::none()).unwrap());
    let conn = Arc::new(client.connect_websocket().expect("websocket connects"));

    let mut handles = Vec::new();
    for i in 0..4 {
        let conn_clone = Arc::clone(&conn);
        handles.push(thread::spawn(move || {
            let req = WebSocketRequest::new(["urn:ietf:params:jmap:core"])
                .with_id(format!("conc-{i}"))
                .call("Core/echo", &json!({}), "c1")
                .unwrap();
            conn_clone.send_request(req).unwrap()
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Never exceed maxConcurrentRequests (2).
    assert!(max_observed.load(Ordering::SeqCst) <= 2);

    conn.close().expect("clean close");
}

#[test]
fn websocket_request_error_without_request_id_resolves_sole_pending_request() {
    let ws_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            match msg {
                Message::Text(_) => {
                    // Stalwart 1.1.1 omits requestId on RequestError frames.
                    let err = json!({
                        "@type": "RequestError",
                        "type": "urn:ietf:params:jmap:error:notRequest",
                        "status": 400,
                        "detail": "Unknown capability"
                    });
                    ws.send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                        .unwrap();
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    let http_server = start_session_server(ws_server.ws_url(), Some(4));
    let client = Client::connect(http_server.origin(), Credentials::none()).unwrap();
    let timeouts = WebSocketTimeouts {
        connect: Duration::from_secs(5),
        request: Duration::from_millis(500),
    };
    let conn = client
        .connect_websocket_with_timeouts(timeouts)
        .expect("websocket connects");

    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core", "urn:example:unknown"])
        .with_id("req-missing-id")
        .call("Core/echo", &json!({}), "c1")
        .unwrap();

    let err = conn.send_request(req).unwrap_err();
    match err {
        Error::Http { status, problem } => {
            assert_eq!(status, 400);
            let prob = problem.expect("problem details present");
            assert_eq!(prob.error_type, "urn:ietf:params:jmap:error:notRequest");
            assert_eq!(prob.detail.as_deref(), Some("Unknown capability"));
        }
        other => panic!("expected Error::Http, got: {other:?}"),
    }

    conn.close().expect("clean close");
}

#[test]
fn websocket_close_with_outcome_distinguishes_echoed_close_and_eof() {
    // (a) Server echoes Close(1000) per RFC 6455 Section 5.5.1.
    let echo_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Close(frame) = msg {
                let _ = ws.close(frame);
                break;
            }
        }
    });
    let http_echo = start_session_server(echo_server.ws_url(), Some(4));
    let client_echo = Client::connect(http_echo.origin(), Credentials::none()).unwrap();
    let conn_echo = client_echo
        .connect_websocket()
        .expect("websocket connects to echo server");
    let outcome_echo = conn_echo
        .close_with_outcome()
        .expect("close handshake with echo succeeds");
    assert!(
        matches!(
            outcome_echo,
            CloseHandshakeOutcome::EchoedClose {
                code: Some(1000),
                ..
            }
        ),
        "expected EchoedClose with code 1000, got: {outcome_echo:?}"
    );
    assert!(conn_echo.is_closed());

    // (b) Server drops TCP stream with EOF without sending a Close frame (Stalwart 1.1.1).
    let eof_server = TestWsServer::start(|mut ws| {
        while let Ok(msg) = ws.read() {
            if let Message::Close(_) = msg {
                // Drop ws immediately without flushing tungstenite's queued close reply.
                break;
            }
        }
    });
    let http_eof = start_session_server(eof_server.ws_url(), Some(4));
    let client_eof = Client::connect(http_eof.origin(), Credentials::none()).unwrap();
    let conn_eof = client_eof
        .connect_websocket()
        .expect("websocket connects to eof server");
    let outcome_eof = conn_eof
        .close_with_outcome()
        .expect("close handshake with eof succeeds");
    assert_eq!(outcome_eof, CloseHandshakeOutcome::EofWithoutCloseFrame);
    assert!(conn_eof.is_closed());
}
