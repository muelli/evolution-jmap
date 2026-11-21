// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for RFC 8887 WebSocket robustness, frame size limits, and inbound frame handling.

#![cfg(feature = "websocket")]

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use jmap_client::websocket::{
    DEFAULT_MAX_FRAME_SIZE, DEFAULT_MAX_MESSAGE_SIZE, InboundFrameAction, WebSocketConnection,
    WebSocketLimits, WebSocketTimeouts, parse_inbound_message, parse_inbound_text,
};
use jmap_client::{Client, Credentials, Error};
use jmap_proto::websocket::WebSocketRequest;
use serde_json::json;

struct JmapSubprotocolCallback;

impl tungstenite::handshake::server::Callback for JmapSubprotocolCallback {
    fn on_request(
        self,
        _request: &tungstenite::handshake::server::Request,
        mut response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        response.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            tungstenite::http::HeaderValue::from_static("jmap"),
        );
        Ok(response)
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
                    "supportsPush": false
                }
            });

            let session = json!({
                "username": "tester@example.com",
                "apiUrl": format!("{s_origin}/api"),
                "downloadUrl": format!("{s_origin}/download/{{accountId}}/{{blobId}}/{{name}}"),
                "uploadUrl": format!("{s_origin}/upload/{{accountId}}"),
                "eventSourceUrl": format!("{s_origin}/eventsource"),
                "state": "initial-state",
                "accounts": {},
                "primaryAccounts": {},
                "capabilities": capabilities
            });

            let body = serde_json::to_vec(&session).unwrap();
            let response = tiny_http::Response::new(
                tiny_http::StatusCode(200),
                vec![
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                ],
                std::io::Cursor::new(body),
                None,
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
fn websocket_limits_defaults_and_custom() {
    let limits = WebSocketLimits::default();
    assert_eq!(limits.max_message_size, DEFAULT_MAX_MESSAGE_SIZE);
    assert_eq!(limits.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    assert_eq!(DEFAULT_MAX_MESSAGE_SIZE, 64 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_FRAME_SIZE, 4 * 1024 * 1024);

    let custom = WebSocketLimits {
        max_message_size: 1024,
        max_frame_size: 512,
    };
    assert_eq!(custom.max_message_size, 1024);
    assert_eq!(custom.max_frame_size, 512);
    assert_eq!(custom, custom);
    let debug_str = format!("{custom:?}");
    assert!(debug_str.contains("WebSocketLimits"));
}

#[test]
fn parse_inbound_frame_malformed_json_and_unknown_types() {
    assert_eq!(parse_inbound_text(""), InboundFrameAction::Ignored);
    assert_eq!(
        parse_inbound_text("not json at all"),
        InboundFrameAction::Ignored
    );
    assert_eq!(
        parse_inbound_text("{broken json"),
        InboundFrameAction::Ignored
    );
    assert_eq!(parse_inbound_text("12345"), InboundFrameAction::Ignored);
    assert_eq!(
        parse_inbound_text(r#"{"hello": "world"}"#),
        InboundFrameAction::Ignored
    );
    assert_eq!(
        parse_inbound_text(r#"{"@type": "UnknownServerType", "id": "1"}"#),
        InboundFrameAction::Ignored
    );
    assert_eq!(
        parse_inbound_text(r#"{"@type": 12345}"#),
        InboundFrameAction::Ignored
    );
}

#[test]
fn parse_inbound_frame_responses_errors_and_state_changes() {
    let resp_text = r#"{
        "@type": "Response",
        "requestId": "req-1",
        "methodResponses": [["Core/echo", {"message": "ok"}, "c1"]],
        "sessionState": "state-123"
    }"#;
    let action = parse_inbound_text(resp_text);
    match action {
        InboundFrameAction::Response { id, response } => {
            assert_eq!(id.as_deref(), Some("req-1"));
            assert_eq!(
                response.session_state.as_ref().map(|s| s.as_str()),
                Some("state-123")
            );
            assert_eq!(response.method_responses.len(), 1);
        }
        other => panic!("expected Response action, got {other:?}"),
    }

    let err_text = r#"{
        "@type": "RequestError",
        "requestId": "req-2",
        "type": "urn:ietf:params:jmap:error:notJSON",
        "status": 400,
        "detail": "Invalid JSON syntax"
    }"#;
    let action = parse_inbound_text(err_text);
    match action {
        InboundFrameAction::RequestError { id, error } => {
            assert_eq!(id.as_deref(), Some("req-2"));
            assert_eq!(error.error_type, "urn:ietf:params:jmap:error:notJSON");
            assert_eq!(error.status, Some(400));
            assert_eq!(error.detail.as_deref(), Some("Invalid JSON syntax"));
        }
        other => panic!("expected RequestError action, got {other:?}"),
    }

    let push_text = r#"{
        "@type": "StateChange",
        "changed": {
            "acc1": {"Mailbox": "state-mailbox-1"}
        },
        "pushState": "pstate-abc"
    }"#;
    let action = parse_inbound_text(push_text);
    match action {
        InboundFrameAction::StateChange {
            state_change,
            push_state,
        } => {
            assert_eq!(push_state.as_deref(), Some("pstate-abc"));
            assert!(
                state_change
                    .changed
                    .contains_key(&jmap_proto::Id::new("acc1"))
            );
        }
        other => panic!("expected StateChange action, got {other:?}"),
    }
}

#[test]
fn parse_inbound_message_control_and_binary_frames() {
    let bin_msg = tungstenite::Message::Binary(vec![0xde, 0xad, 0xbe, 0xef].into());
    assert_eq!(parse_inbound_message(&bin_msg), InboundFrameAction::Ignored);

    let ping_msg = tungstenite::Message::Ping(vec![1, 2, 3].into());
    assert_eq!(
        parse_inbound_message(&ping_msg),
        InboundFrameAction::Ping(vec![1, 2, 3])
    );

    let pong_msg = tungstenite::Message::Pong(vec![4, 5, 6].into());
    assert_eq!(
        parse_inbound_message(&pong_msg),
        InboundFrameAction::Pong(vec![4, 5, 6])
    );

    let close_msg = tungstenite::Message::Close(Some(tungstenite::protocol::CloseFrame {
        code: tungstenite::protocol::frame::coding::CloseCode::Normal,
        reason: "Normal closure".into(),
    }));
    assert_eq!(
        parse_inbound_message(&close_msg),
        InboundFrameAction::Close(Some("Normal closure".into()))
    );
}

#[test]
fn websocket_robust_against_interleaved_and_malformed_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    let server_thread = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept connection");
        let mut ws = tungstenite::accept_hdr(stream, JmapSubprotocolCallback)
            .expect("accept websocket handshake");

        // Server receives req-1 and req-2
        let first_msg = ws.read().expect("read first request");
        let second_msg = ws.read().expect("read second request");

        let first_text = first_msg.into_text().unwrap();
        let second_text = second_msg.into_text().unwrap();

        assert!(first_text.contains("req-1") || second_text.contains("req-1"));
        assert!(first_text.contains("req-2") || second_text.contains("req-2"));

        // Interleave frames:
        // 1. Send response for req-2 first (out of order)
        let resp2 = serde_json::json!({
            "@type": "Response",
            "requestId": "req-2",
            "methodResponses": [["Core/echo", {"id": "two"}, "call-2"]],
            "sessionState": "s2"
        });
        ws.send(tungstenite::Message::Text(resp2.to_string().into()))
            .unwrap();

        // 2. Send an unknown frame type
        let unknown = serde_json::json!({
            "@type": "UnknownServerFrame",
            "foo": "bar"
        });
        ws.send(tungstenite::Message::Text(unknown.to_string().into()))
            .unwrap();

        // 3. Send malformed JSON text frame
        ws.send(tungstenite::Message::Text(
            "this is not valid json at all {[[".into(),
        ))
        .unwrap();

        // 4. Send a binary frame
        ws.send(tungstenite::Message::Binary(
            vec![0xca, 0xfe, 0xba, 0xbe].into(),
        ))
        .unwrap();

        // 5. Send an unmatched response ID
        let unmatched = serde_json::json!({
            "@type": "Response",
            "requestId": "unmatched-id-999",
            "methodResponses": [],
            "sessionState": "s_other"
        });
        ws.send(tungstenite::Message::Text(unmatched.to_string().into()))
            .unwrap();

        // 6. Send response for req-1
        let resp1 = serde_json::json!({
            "@type": "Response",
            "requestId": "req-1",
            "methodResponses": [["Core/echo", {"id": "one"}, "call-1"]],
            "sessionState": "s1"
        });
        ws.send(tungstenite::Message::Text(resp1.to_string().into()))
            .unwrap();
    });

    let ws_url = format!("ws://{addr}/ws");
    let http_server = start_session_server(ws_url);

    let client = Client::connect(http_server.origin(), Credentials::none()).expect("build client");

    let conn = WebSocketConnection::connect(&client).expect("connect ws");
    let conn_arc = std::sync::Arc::new(conn);

    let c1 = std::sync::Arc::clone(&conn_arc);
    let handle1 = thread::spawn(move || {
        let mut req = WebSocketRequest::new(["urn:ietf:params:jmap:core"]);
        req.id = Some("req-1".into());
        c1.send_request(req)
    });

    let c2 = std::sync::Arc::clone(&conn_arc);
    let handle2 = thread::spawn(move || {
        let mut req = WebSocketRequest::new(["urn:ietf:params:jmap:core"]);
        req.id = Some("req-2".into());
        c2.send_request(req)
    });

    let res1 = handle1.join().unwrap().expect("req-1 succeeded");
    let res2 = handle2.join().unwrap().expect("req-2 succeeded");

    assert_eq!(res1.id.as_deref(), Some("req-1"));
    assert_eq!(res1.session_state.as_ref().map(|s| s.as_str()), Some("s1"));
    assert_eq!(res2.id.as_deref(), Some("req-2"));
    assert_eq!(res2.session_state.as_ref().map(|s| s.as_str()), Some("s2"));

    server_thread.join().unwrap();
}

#[test]
fn websocket_limits_enforced_against_oversized_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    let server_thread = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept connection");
        let mut ws = tungstenite::accept_hdr(stream, JmapSubprotocolCallback)
            .expect("accept websocket handshake");

        let _req = ws.read().expect("read request");

        // Send a frame larger than the client's configured max_frame_size (512 bytes)
        let huge_padding = "x".repeat(1024);
        let resp = serde_json::json!({
            "@type": "Response",
            "requestId": "req-limit-1",
            "methodResponses": [],
            "sessionState": huge_padding
        });
        // Server sends a frame of ~1100 bytes
        let _ = ws.send(tungstenite::Message::Text(resp.to_string().into()));
    });

    let ws_url = format!("ws://{addr}/ws");
    let http_server = start_session_server(ws_url);

    let client = Client::connect(http_server.origin(), Credentials::none()).expect("build client");

    let timeouts = WebSocketTimeouts {
        connect: Duration::from_secs(5),
        request: Duration::from_secs(5),
    };
    let limits = WebSocketLimits {
        max_message_size: 1024,
        max_frame_size: 512,
    };

    let conn = WebSocketConnection::connect_with_limits(&client, timeouts, limits)
        .expect("connect with limits");

    let mut req = WebSocketRequest::new(["urn:ietf:params:jmap:core"]);
    req.id = Some("req-limit-1".into());

    let result = conn.send_request(req);
    // Because the frame exceeded 512 bytes, tungstenite triggers a Capacity error
    // and connection closes cleanly without panic.
    assert!(result.is_err());
    match result {
        Err(Error::Transport(msg)) => {
            assert!(
                msg.contains("connection closed")
                    || msg.contains("Capacity")
                    || msg.contains("reset")
                    || msg.contains("closed"),
                "unexpected error message: {msg}"
            );
        }
        other => panic!("expected Transport error, got {other:?}"),
    }

    server_thread.join().unwrap();
}

#[test]
fn websocket_url_parsing_and_rebasing() {
    use jmap_client::websocket::{
        ParsedWsUrl, parse_websocket_url, rebase_websocket_url, to_http_equivalent_url,
    };

    // Standard ws:// and wss://
    let ws = parse_websocket_url("ws://example.com/ws").expect("parse ws");
    assert_eq!(
        ws,
        ParsedWsUrl {
            is_tls: false,
            host: "example.com".into(),
            port: 80,
            dial: "example.com:80".into(),
        }
    );

    let wss = parse_websocket_url("wss://secure.example.com:8443/jmap/ws?token=abc#hash")
        .expect("parse wss");
    assert_eq!(
        wss,
        ParsedWsUrl {
            is_tls: true,
            host: "secure.example.com".into(),
            port: 8443,
            dial: "secure.example.com:8443".into(),
        }
    );

    // IPv6 bracketed
    let ipv6 = parse_websocket_url("ws://[::1]:9999/ws").expect("parse ipv6");
    assert_eq!(
        ipv6,
        ParsedWsUrl {
            is_tls: false,
            host: "::1".into(),
            port: 9999,
            dial: "[::1]:9999".into(),
        }
    );

    let ipv6_default = parse_websocket_url("wss://[2001:db8::1]/ws").expect("parse ipv6 default");
    assert_eq!(
        ipv6_default,
        ParsedWsUrl {
            is_tls: true,
            host: "2001:db8::1".into(),
            port: 443,
            dial: "[2001:db8::1]:443".into(),
        }
    );

    // Invalid scheme
    assert!(parse_websocket_url("http://example.com/ws").is_err());
    assert!(parse_websocket_url("ftp://example.com/ws").is_err());
    assert!(parse_websocket_url("").is_err());

    // Invalid IPv6 brackets or junk after bracket
    assert!(parse_websocket_url("ws://[::1/ws").is_err());
    assert!(parse_websocket_url("ws://[::1]junk/ws").is_err());

    // HTTP equivalent conversion
    assert_eq!(
        to_http_equivalent_url("ws://example.com/ws"),
        "http://example.com/ws"
    );
    assert_eq!(
        to_http_equivalent_url("wss://example.com:8443/ws"),
        "https://example.com:8443/ws"
    );
    assert_eq!(to_http_equivalent_url("other://url"), "other://url");

    // Rebasing
    assert_eq!(
        rebase_websocket_url("ws://internal.local/ws", "http://gateway.example.com"),
        "ws://gateway.example.com/ws"
    );
    assert_eq!(
        rebase_websocket_url(
            "wss://internal.local:9000/ws",
            "https://gateway.example.com:443"
        ),
        "wss://gateway.example.com:443/ws"
    );
}
