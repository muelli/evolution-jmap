// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Concurrency limits (`maxConcurrentRequests` and `maxConcurrentUpload`, RFC 8620 §2).
//!
//! A conforming client must not issue more concurrent requests or uploads
//! than the server advertises in its core capability.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Duration;

use jmap_client::transport::{CancelFlag, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_proto::Id;
use serde_json::json;

struct TrackingTransport {
    session_json: RwLock<String>,
    current_requests: AtomicUsize,
    peak_requests: AtomicUsize,
    current_uploads: AtomicUsize,
    peak_uploads: AtomicUsize,
    barrier: Option<Arc<Barrier>>,
    request_delay: Duration,
}

impl TrackingTransport {
    fn new(
        max_concurrent_requests: Option<u64>,
        max_concurrent_upload: Option<u64>,
        request_delay: Duration,
    ) -> Self {
        let mut core = json!({
            "maxSizeUpload": 50000000,
            "maxSizeRequest": 10000000,
            "maxCallsInRequest": 16,
            "maxObjectsInGet": 500,
            "maxObjectsInSet": 128,
            "collationAlgorithms": ["i;ascii-casemap"],
        });
        if let Some(limit) = max_concurrent_requests {
            core["maxConcurrentRequests"] = json!(limit);
        }
        if let Some(limit) = max_concurrent_upload {
            core["maxConcurrentUpload"] = json!(limit);
        }

        let session = json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": core,
                "urn:ietf:params:jmap:mail": {},
            },
            "accounts": {
                "acc1": {
                    "name": "user@example.com",
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": {
                        "urn:ietf:params:jmap:core": {},
                        "urn:ietf:params:jmap:mail": {},
                    }
                }
            },
            "primaryAccounts": {
                "urn:ietf:params:jmap:mail": "acc1",
            },
            "username": "user@example.com",
            "apiUrl": "https://mail.example.internal/jmap/api",
            "downloadUrl": "https://mail.example.internal/jmap/download/{blobId}",
            "uploadUrl": "https://mail.example.internal/jmap/upload/{accountId}",
            "eventSourceUrl": "https://mail.example.internal/jmap/events",
            "state": "session-state-1"
        });

        Self {
            session_json: RwLock::new(session.to_string()),
            current_requests: AtomicUsize::new(0),
            peak_requests: AtomicUsize::new(0),
            current_uploads: AtomicUsize::new(0),
            peak_uploads: AtomicUsize::new(0),
            barrier: None,
            request_delay,
        }
    }

    fn set_session_json(&self, session: serde_json::Value) {
        *self.session_json.write().unwrap() = session.to_string();
    }

    fn update_peak(current: &AtomicUsize, peak: &AtomicUsize) {
        let cur = current.load(Ordering::SeqCst);
        let mut prev_peak = peak.load(Ordering::SeqCst);
        while cur > prev_peak {
            match peak.compare_exchange_weak(prev_peak, cur, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(actual) => prev_peak = actual,
            }
        }
    }
}

impl Transport for TrackingTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        if req.url.ends_with("/.well-known/jmap") || req.url.ends_with("/session") {
            return Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: self.session_json.read().unwrap().clone().into_bytes(),
                final_url: req.url.to_owned(),
            });
        }

        let is_upload = req.url.contains("/upload/");
        let current = if is_upload {
            &self.current_uploads
        } else {
            &self.current_requests
        };
        let peak = if is_upload {
            &self.peak_uploads
        } else {
            &self.peak_requests
        };

        current.fetch_add(1, Ordering::SeqCst);
        Self::update_peak(current, peak);

        if let Some(barrier) = &self.barrier {
            barrier.wait();
        } else if !self.request_delay.is_zero() {
            thread::sleep(self.request_delay);
        }

        current.fetch_sub(1, Ordering::SeqCst);

        if is_upload {
            Ok(HttpResponse {
                status: 201,
                content_type: Some("application/json".to_owned()),
                body: json!({
                    "accountId": "acc1",
                    "blobId": "b1",
                    "type": "text/plain",
                    "size": req.body.map(|b| b.len()).unwrap_or(0),
                })
                .to_string()
                .into_bytes(),
                final_url: req.url.to_owned(),
            })
        } else {
            let call_id = req
                .body
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
                .and_then(|v| {
                    v.get("methodCalls")?
                        .get(0)?
                        .get(2)?
                        .as_str()
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "c0".to_owned());

            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: json!({
                    "sessionState": "session-state-1",
                    "methodResponses": [
                        ["Core/echo", {"req": "ok"}, call_id]
                    ]
                })
                .to_string()
                .into_bytes(),
                final_url: req.url.to_owned(),
            })
        }
    }
}

#[test]
fn concurrent_requests_are_bounded_to_max_concurrent_requests() {
    let transport = Arc::new(TrackingTransport::new(
        Some(2),
        Some(4),
        Duration::from_millis(50),
    ));

    struct SharedTransport(Arc<TrackingTransport>);
    impl Transport for SharedTransport {
        fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            self.0.execute(req)
        }
    }

    let client = Arc::new(
        Client::builder()
            .transport(SharedTransport(Arc::clone(&transport)))
            .connect("https://mail.example.internal", Credentials::none())
            .expect("connect succeeds"),
    );

    assert_eq!(client.session().max_concurrent_requests(), Some(2));

    let mut handles = Vec::new();
    for i in 0..10 {
        let c = Arc::clone(&client);
        handles.push(thread::spawn(move || c.echo(json!({"thread": i}))));
    }

    for handle in handles {
        let res = handle.join().expect("thread join");
        assert!(res.is_ok(), "echo failed: {res:?}");
    }

    let peak = transport.peak_requests.load(Ordering::SeqCst);
    assert!(
        peak <= 2,
        "peak concurrent requests ({peak}) exceeded advertised limit (2)"
    );
}

#[test]
fn concurrent_uploads_are_bounded_to_max_concurrent_upload() {
    let transport = Arc::new(TrackingTransport::new(
        Some(8),
        Some(2),
        Duration::from_millis(50),
    ));

    struct SharedTransport(Arc<TrackingTransport>);
    impl Transport for SharedTransport {
        fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            self.0.execute(req)
        }
    }

    let client = Arc::new(
        Client::builder()
            .transport(SharedTransport(Arc::clone(&transport)))
            .connect("https://mail.example.internal", Credentials::none())
            .expect("connect succeeds"),
    );

    assert_eq!(client.session().max_concurrent_upload(), Some(2));

    let mut handles = Vec::new();
    for i in 0..8 {
        let c = Arc::clone(&client);
        handles.push(thread::spawn(move || {
            let account_id = Id::new("acc1");
            c.upload_blob_slice(&account_id, "text/plain", format!("payload {i}").as_bytes())
        }));
    }

    for handle in handles {
        let res = handle.join().expect("thread join");
        assert!(res.is_ok(), "upload failed: {res:?}");
    }

    let peak = transport.peak_uploads.load(Ordering::SeqCst);
    assert!(
        peak <= 2,
        "peak concurrent uploads ({peak}) exceeded advertised limit (2)"
    );
}

#[test]
fn waiting_for_concurrency_slot_aborts_on_cancellation() {
    let transport = Arc::new(TrackingTransport::new(
        Some(1),
        Some(1),
        Duration::from_millis(300),
    ));

    struct SharedTransport(Arc<TrackingTransport>);
    impl Transport for SharedTransport {
        fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            self.0.execute(req)
        }
    }

    let cancel = CancelFlag::new();
    let client = Arc::new(
        Client::builder()
            .transport(SharedTransport(Arc::clone(&transport)))
            .cancel_flag(cancel.clone())
            .connect("https://mail.example.internal", Credentials::none())
            .expect("connect succeeds"),
    );

    // Thread 1 occupies the only slot for 300ms.
    let c1 = Arc::clone(&client);
    let h1 = thread::spawn(move || c1.echo(json!({"req": 1})));

    // Ensure thread 1 has started and is executing.
    thread::sleep(Duration::from_millis(50));

    // Cancel while thread 2 tries to acquire the slot.
    let c2 = Arc::clone(&client);
    let cancel2 = cancel.clone();
    let h2 = thread::spawn(move || {
        // Cancel shortly after starting to wait.
        thread::sleep(Duration::from_millis(30));
        cancel2.cancel();
    });

    let h3 = thread::spawn(move || c2.echo(json!({"req": 2})));

    h2.join().unwrap();
    let res3 = h3.join().unwrap();
    let res1 = h1.join().unwrap();

    assert!(res1.is_ok(), "first request succeeded");
    assert!(
        matches!(res3, Err(Error::Cancelled)),
        "cancelled wait must return Error::Cancelled, got {res3:?}"
    );
}

#[test]
fn session_refresh_updates_concurrency_limits() {
    let transport = Arc::new(TrackingTransport::new(
        Some(1),
        Some(1),
        Duration::from_millis(50),
    ));

    struct SharedTransport(Arc<TrackingTransport>);
    impl Transport for SharedTransport {
        fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            self.0.execute(req)
        }
    }

    let mut client = Client::builder()
        .transport(SharedTransport(Arc::clone(&transport)))
        .connect("https://mail.example.internal", Credentials::none())
        .expect("connect succeeds");

    assert_eq!(client.session().max_concurrent_requests(), Some(1));

    // Update the advertised session on the transport to limit 4.
    let updated_session = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 50000000,
                "maxSizeRequest": 10000000,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 128,
                "collationAlgorithms": ["i;ascii-casemap"],
                "maxConcurrentRequests": 4,
                "maxConcurrentUpload": 4,
            },
            "urn:ietf:params:jmap:mail": {},
        },
        "accounts": {
            "acc1": {
                "name": "user@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:mail": {},
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "acc1",
        },
        "username": "user@example.com",
        "apiUrl": "https://mail.example.internal/jmap/api",
        "downloadUrl": "https://mail.example.internal/jmap/download/{blobId}",
        "uploadUrl": "https://mail.example.internal/jmap/upload/{accountId}",
        "eventSourceUrl": "https://mail.example.internal/jmap/events",
        "state": "session-state-2"
    });
    transport.set_session_json(updated_session);

    client.refresh_session().expect("refresh succeeds");
    assert_eq!(client.session().max_concurrent_requests(), Some(4));
    assert_eq!(client.session().max_concurrent_upload(), Some(4));

    transport.peak_requests.store(0, Ordering::SeqCst);

    let client = Arc::new(client);
    let mut handles = Vec::new();
    for i in 0..8 {
        let c = Arc::clone(&client);
        handles.push(thread::spawn(move || c.echo(json!({"thread": i}))));
    }

    for handle in handles {
        let res = handle.join().expect("thread join");
        assert!(res.is_ok());
    }

    let peak = transport.peak_requests.load(Ordering::SeqCst);
    assert!(
        peak <= 4,
        "peak concurrent requests ({peak}) exceeded new limit (4)"
    );
    assert!(
        peak > 1,
        "peak concurrent requests ({peak}) should exceed the old limit (1)"
    );
}

#[test]
fn mock_server_advertises_configured_concurrency_limits() {
    let server = jmap_mock::MockServer::builder()
        .max_concurrent_requests(2)
        .max_concurrent_upload(3)
        .start();

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert_eq!(client.session().max_concurrent_requests(), Some(2));
    assert_eq!(client.session().max_concurrent_upload(), Some(3));
}
