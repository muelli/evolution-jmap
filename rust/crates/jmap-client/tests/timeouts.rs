// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verification of bounded timeouts across blocking calls and hung server resilience.
//!
//! Every blocking call must have a bounded timeout by default (connect, read, total)
//! so that an unresponsive or hung server cannot block the caller forever.

use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use jmap_client::eventsource::{EventSourceSubscription, EventSourceTimeouts, SharedHeaders};
use jmap_client::transport::{CancelFlag, UreqTransport};
use jmap_client::{ClientBuilder, Credentials, Error};
use jmap_proto::Id;

/// Spawn a local TCP server that accepts connections and never sends any data back.
fn spawn_hung_server() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    (listener, port)
}

/// Spawn a local session server that answers session discovery and points
/// all resource endpoints (API, upload, download, eventsource) to the hung server port.
fn spawn_session_server(hung_port: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let hung_url = format!("http://127.0.0.1:{hung_port}");
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let session_json = serde_json::json!({
                "capabilities": {
                    "urn:ietf:params:jmap:core": {
                        "maxSizeUpload": 50000000,
                        "maxConcurrentUpload": 4,
                        "maxSizeRequest": 10000000,
                        "maxConcurrentRequests": 4,
                        "maxCallsInRequest": 16,
                        "maxObjectsInGet": 500,
                        "maxObjectsInSet": 500,
                        "collationAlgorithms": ["i;ascii-casemap", "i;octet"]
                    },
                    "urn:ietf:params:jmap:mail": {}
                },
                "accounts": {
                    "a1": {
                        "name": "test@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:mail": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:core": "a1",
                    "urn:ietf:params:jmap:mail": "a1"
                },
                "username": "test@example.com",
                "apiUrl": format!("{hung_url}/api"),
                "downloadUrl": format!("{hung_url}/download/{{accountId}}/{{blobId}}/{{name}}"),
                "uploadUrl": format!("{hung_url}/upload/{{accountId}}"),
                "eventSourceUrl": format!("{hung_url}/eventsource"),
                "state": "s-1"
            });
            let body = session_json.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

#[test]
fn client_builder_exposes_connect_read_and_total_timeouts() {
    let builder = ClientBuilder::default()
        .connect_timeout(Duration::from_millis(150))
        .read_timeout(Duration::from_millis(250))
        .timeout(Duration::from_millis(500));

    let (listener, port) = spawn_hung_server();
    let thread_handle = thread::spawn(move || {
        let (_stream, _) = listener.accept().expect("accepted");
        thread::sleep(Duration::from_secs(5));
    });

    let start = Instant::now();
    let result = builder.connect(&format!("http://127.0.0.1:{port}"), Credentials::none());
    let elapsed = start.elapsed();

    assert!(result.is_err(), "connecting to hung server should fail");
    assert!(
        elapsed < Duration::from_secs(2),
        "connect should time out promptly according to configured timeout, took {elapsed:?}"
    );

    let _ = thread_handle.join();
}

#[test]
fn ureq_transport_with_timeouts_is_constructible() {
    let _transport = UreqTransport::with_timeouts(
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(300),
    );
}

#[test]
fn client_connect_to_hung_server_times_out() {
    let (listener, port) = spawn_hung_server();
    let thread_handle = thread::spawn(move || {
        let (_stream, _) = listener.accept().expect("accepted");
        // Keep the connection open and never respond.
        thread::sleep(Duration::from_secs(5));
    });

    let start = Instant::now();
    let result = ClientBuilder::default()
        .timeout(Duration::from_millis(200))
        .connect(&format!("http://127.0.0.1:{port}"), Credentials::none());
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Transport(_))),
        "expected transport error, got {result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(150) && elapsed < Duration::from_secs(2),
        "hung connect took {elapsed:?}, expected ~200ms"
    );

    let _ = thread_handle.join();
}

#[test]
fn client_call_to_hung_server_times_out() {
    let (hung_listener, hung_port) = spawn_hung_server();
    let hung_handle = thread::spawn(move || {
        while let Ok((_stream, _)) = hung_listener.accept() {
            thread::sleep(Duration::from_secs(5));
        }
    });

    let session_port = spawn_session_server(hung_port);
    let client = ClientBuilder::default()
        .connect_timeout(Duration::from_millis(100))
        .read_timeout(Duration::from_millis(200))
        .timeout(Duration::from_millis(300))
        .connect(
            &format!("http://127.0.0.1:{session_port}"),
            Credentials::none(),
        )
        .expect("connected to session server");

    let start = Instant::now();
    let result = client.api_call(&jmap_proto::request::Request::new(Vec::<String>::new()));
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Transport(_))),
        "expected transport error, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "hung call took {elapsed:?}, expected bounded timeout"
    );

    drop(hung_handle);
}

#[test]
fn client_upload_blob_to_hung_server_times_out() {
    let (hung_listener, hung_port) = spawn_hung_server();
    let hung_handle = thread::spawn(move || {
        while let Ok((_stream, _)) = hung_listener.accept() {
            thread::sleep(Duration::from_secs(5));
        }
    });

    let session_port = spawn_session_server(hung_port);
    let client = ClientBuilder::default()
        .connect_timeout(Duration::from_millis(100))
        .read_timeout(Duration::from_millis(200))
        .timeout(Duration::from_millis(300))
        .connect(
            &format!("http://127.0.0.1:{session_port}"),
            Credentials::none(),
        )
        .expect("connected to session server");

    let start = Instant::now();
    let result = client.upload_blob(&Id::new("a1"), "text/plain", b"hello".to_vec());
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Transport(_))),
        "expected transport error, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "hung upload took {elapsed:?}, expected bounded timeout"
    );

    drop(hung_handle);
}

#[test]
fn client_download_blob_to_hung_server_times_out() {
    let (hung_listener, hung_port) = spawn_hung_server();
    let hung_handle = thread::spawn(move || {
        while let Ok((_stream, _)) = hung_listener.accept() {
            thread::sleep(Duration::from_secs(5));
        }
    });

    let session_port = spawn_session_server(hung_port);
    let client = ClientBuilder::default()
        .connect_timeout(Duration::from_millis(100))
        .read_timeout(Duration::from_millis(200))
        .timeout(Duration::from_millis(300))
        .connect(
            &format!("http://127.0.0.1:{session_port}"),
            Credentials::none(),
        )
        .expect("connected to session server");

    let start = Instant::now();
    let result = client.download_blob(&Id::new("a1"), &Id::new("b1"), "test.txt", 1024);
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Transport(_))),
        "expected transport error, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "hung download took {elapsed:?}, expected bounded timeout"
    );

    drop(hung_handle);
}

#[test]
fn oauth_discover_to_hung_server_times_out() {
    let (hung_listener, hung_port) = spawn_hung_server();
    let hung_handle = thread::spawn(move || {
        while let Ok((_stream, _)) = hung_listener.accept() {
            thread::sleep(Duration::from_secs(5));
        }
    });

    let transport = UreqTransport::with_timeouts(
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(300),
    );

    let start = Instant::now();
    let result =
        jmap_client::oauth::discover(&transport, &format!("http://127.0.0.1:{hung_port}"), None);
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Transport(_))),
        "expected transport error, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "hung oauth discovery took {elapsed:?}, expected bounded timeout"
    );

    drop(hung_handle);
}

#[test]
fn eventsource_connect_to_hung_server_times_out_and_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let accept_count = Arc::new(AtomicUsize::new(0));
    let accept_count_clone = Arc::clone(&accept_count);

    let server_handle = thread::spawn(move || {
        // Accept connections and leave them hung to verify the client notices
        // the response head read timeout and reconnects rather than hanging forever.
        while let Ok((_stream, _)) = listener.accept() {
            accept_count_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    let timeouts = EventSourceTimeouts {
        connect: Duration::from_millis(100),
        read_head: Duration::from_millis(150),
        stream_read: Duration::from_secs(10),
    };

    let cancel = CancelFlag::new();
    let mut subscription = EventSourceSubscription::start_with_timeouts(
        format!("http://127.0.0.1:{port}/eventsource"),
        SharedHeaders::new(Vec::new()),
        cancel.clone(),
        timeouts,
    );

    // Wait enough for the initial connection attempt to time out on reading headers
    // and attempt at least one reconnection.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if accept_count.load(Ordering::SeqCst) >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let count = accept_count.load(Ordering::SeqCst);
    subscription.stop();
    drop(server_handle);

    assert!(
        count >= 2,
        "eventsource client should time out on hung response head and reconnect, saw {count} connection attempts"
    );
}

#[test]
fn eventsource_stream_idle_hung_server_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let accept_count = Arc::new(AtomicUsize::new(0));
    let accept_count_clone = Arc::clone(&accept_count);

    let server_handle = thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            accept_count_clone.fetch_add(1, Ordering::SeqCst);
            // Send valid HTTP chunked response headers, then hang indefinitely
            // without sending pings or chunks.
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        }
    });

    let timeouts = EventSourceTimeouts {
        connect: Duration::from_millis(100),
        read_head: Duration::from_millis(500),
        stream_read: Duration::from_millis(150),
    };

    let cancel = CancelFlag::new();
    let mut subscription = EventSourceSubscription::start_with_timeouts(
        format!("http://127.0.0.1:{port}/eventsource"),
        SharedHeaders::new(Vec::new()),
        cancel.clone(),
        timeouts,
    );

    // Stream read timeout is 150ms. Client should detect that the stream is dead,
    // end the stream, back off, and reconnect to the listener.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if accept_count.load(Ordering::SeqCst) >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let count = accept_count.load(Ordering::SeqCst);
    subscription.stop();
    drop(server_handle);

    assert!(
        count >= 2,
        "eventsource client should time out on idle hung stream and reconnect, saw {count} connection attempts"
    );
}
