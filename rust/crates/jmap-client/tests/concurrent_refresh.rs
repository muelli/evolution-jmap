// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for refreshing stale sessions through a shared Client reference (RFC 8620 Section 2).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use jmap_client::{Client, Credentials};
use jmap_proto::request::Request;
use serde_json::json;

#[test]
fn refresh_session_callable_through_shared_reference() {
    let server = jmap_mock::MockServer::builder().start();
    let client = Arc::new(Client::connect(server.origin(), Credentials::none()).unwrap());

    assert_eq!(client.session().state.as_str(), "1");
    assert!(!client.is_session_stale());

    // Advance session state on the server.
    {
        let state_lock = server.state();
        let mut state = state_lock.lock().unwrap();
        state.session_state = 2;
    }

    // Call Core/echo. Response sessionState "2" marks the session stale.
    let echo = client.echo(json!({"ping": "pong"})).unwrap();
    assert_eq!(echo["ping"], "pong");
    assert!(client.is_session_stale());

    // refresh_session must work through a shared &Client / Arc<Client> reference.
    client.refresh_session().expect("session refresh succeeds");

    assert_eq!(client.session().state.as_str(), "2");
    assert!(!client.is_session_stale());
}

#[test]
fn concurrent_requests_during_refresh_honour_api_url_without_deadlock() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind test server");
    let origin = format!("http://{}", server.server_addr());

    let session_count = Arc::new(AtomicUsize::new(0));
    let v1_count = Arc::new(AtomicUsize::new(0));
    let v2_count = Arc::new(AtomicUsize::new(0));
    let running = Arc::new(AtomicBool::new(true));

    let s_session_count = Arc::clone(&session_count);
    let s_v1_count = Arc::clone(&v1_count);
    let s_v2_count = Arc::clone(&v2_count);
    let s_running = Arc::clone(&running);
    let s_origin = origin.clone();

    let server_thread = thread::spawn(move || {
        while s_running.load(Ordering::Relaxed) {
            let request = match server.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(req)) => req,
                Ok(None) => continue,
                Err(_) => break,
            };

            let url = request.url().to_string();
            let method = request.method().clone();

            if url == "/.well-known/jmap" && method == tiny_http::Method::Get {
                let count = s_session_count.fetch_add(1, Ordering::SeqCst);
                let (state, api_path) = if count == 0 {
                    ("state-1", "/api-v1")
                } else {
                    ("state-2", "/api-v2")
                };

                let body = json!({
                    "username": "user@example.com",
                    "apiUrl": format!("{s_origin}{api_path}"),
                    "downloadUrl": format!("{s_origin}/download/{{accountId}}/{{blobId}}/{{name}}"),
                    "uploadUrl": format!("{s_origin}/upload/{{accountId}}"),
                    "eventSourceUrl": format!("{s_origin}/eventsource"),
                    "state": state,
                    "capabilities": {
                        "urn:ietf:params:jmap:core": {
                            "maxSizeUpload": 50000000,
                            "maxConcurrentUpload": 4,
                            "maxSizeRequest": 10000000,
                            "maxConcurrentRequests": 8,
                            "maxCallsInRequest": 16,
                            "maxObjectsInGet": 500,
                            "maxObjectsInSet": 500,
                            "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap"]
                        }
                    },
                    "accounts": {
                        "A1": {
                            "name": "user@example.com",
                            "isPersonal": true,
                            "isReadOnly": false,
                            "accountCapabilities": {
                                "urn:ietf:params:jmap:core": {}
                            }
                        }
                    },
                    "primaryAccounts": {
                        "urn:ietf:params:jmap:core": "A1"
                    }
                })
                .to_string();

                let response = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);
            } else if url == "/api-v1" && method == tiny_http::Method::Post {
                s_v1_count.fetch_add(1, Ordering::SeqCst);
                // Artificial small delay to simulate processing during refresh.
                thread::sleep(Duration::from_millis(5));
                let body = json!({
                    "sessionState": "state-2",
                    "methodResponses": [
                        ["Core/echo", {"message": "from-v1"}, "c0"]
                    ]
                })
                .to_string();
                let response = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);
            } else if url == "/api-v2" && method == tiny_http::Method::Post {
                s_v2_count.fetch_add(1, Ordering::SeqCst);
                let body = json!({
                    "sessionState": "state-2",
                    "methodResponses": [
                        ["Core/echo", {"message": "from-v2"}, "c0"]
                    ]
                })
                .to_string();
                let response = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);
            } else {
                let _ = request.respond(tiny_http::Response::empty(404));
            }
        }
    });

    let client = Arc::new(Client::connect(&origin, Credentials::none()).expect("connect succeeds"));
    assert_eq!(client.session().state.as_str(), "state-1");
    assert!(client.session().api_url.ends_with("/api-v1"));
    assert!(!client.is_session_stale());

    // Worker thread keeps issuing requests.
    let worker_client = Arc::clone(&client);
    let worker_running = Arc::clone(&running);
    let worker_results = Arc::new(Mutex::new(Vec::new()));
    let s_worker_results = Arc::clone(&worker_results);

    let worker_handle = thread::spawn(move || {
        while worker_running.load(Ordering::Relaxed) {
            let request = Request::new(["urn:ietf:params:jmap:core"])
                .call("Core/echo", &json!({"ping": true}), "c0")
                .unwrap();
            match worker_client.api_call(&request) {
                Ok(resp) => {
                    let first = resp.method_responses.first().map(|m| m.arguments.clone());
                    s_worker_results.lock().unwrap().push(first);
                }
                Err(err) => {
                    // Record any unexpected error.
                    panic!("concurrent request failed: {err:?}");
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    });

    // Wait for at least one response to be recorded from /api-v1.
    while worker_results.lock().unwrap().is_empty() {
        thread::sleep(Duration::from_millis(5));
    }

    // Client detects stale session due to sessionState "state-2".
    assert!(client.is_session_stale());

    // Refresh session on thread 1 through shared reference while worker thread continues.
    client
        .refresh_session()
        .expect("refresh succeeds concurrently");

    // Session must be updated to state-2 and apiUrl to /api-v2.
    assert_eq!(client.session().state.as_str(), "state-2");
    assert!(client.session().api_url.ends_with("/api-v2"));
    assert!(!client.is_session_stale());

    // Wait until at least one request hits /api-v2.
    let start = std::time::Instant::now();
    while v2_count.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(10));
    }

    // Stop worker and server.
    running.store(false, Ordering::Relaxed);
    worker_handle.join().expect("worker joins cleanly");
    server_thread.join().expect("server joins cleanly");

    // Verify requests were processed at both endpoints.
    assert!(
        v1_count.load(Ordering::SeqCst) > 0,
        "api-v1 received requests"
    );
    assert!(
        v2_count.load(Ordering::SeqCst) > 0,
        "api-v2 received requests after refresh"
    );

    let results = worker_results.lock().unwrap();
    let saw_v1 = results.iter().any(|v| {
        v.as_ref()
            .map(|val| val["message"] == "from-v1")
            .unwrap_or(false)
    });
    let saw_v2 = results.iter().any(|v| {
        v.as_ref()
            .map(|val| val["message"] == "from-v2")
            .unwrap_or(false)
    });
    assert!(saw_v1, "observed responses from v1");
    assert!(saw_v2, "observed responses from v2");
}

#[test]
fn live_server_refresh_session_through_shared_reference() {
    let Ok(server_url) = std::env::var("JMAP_LIVE_SERVER_URL") else {
        return;
    };
    if server_url.is_empty() {
        return;
    }
    let username = std::env::var("JMAP_LIVE_SERVER_USER").unwrap_or_else(|_| "user1".into());
    let password = std::env::var("JMAP_LIVE_SERVER_PASSWORD").unwrap_or_else(|_| "password".into());
    let credentials = Credentials::basic(username, password);

    let client = Arc::new(Client::connect(&server_url, credentials).expect("connect succeeds"));
    assert!(!client.session().state.as_str().is_empty());
    assert!(!client.is_session_stale());

    // Refresh through shared reference &Client / Arc<Client>.
    client
        .refresh_session()
        .expect("live session refresh succeeds through shared reference");
    assert!(!client.session().state.as_str().is_empty());
    assert!(!client.is_session_stale());
}
