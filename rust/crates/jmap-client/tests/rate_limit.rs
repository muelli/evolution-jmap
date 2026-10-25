// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for HTTP 429 and limit error handling without duplicate writes.
//!
//! Validates that idempotent calls back off and retry.
//! Validates that non-idempotent calls return rate limit errors without retrying.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jmap_client::transport::{
    CancelFlag, HttpRequest, HttpResponse, Transport, TransportError, UreqTransport,
};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::contacts::ContactCard;
use serde_json::json;

/// Intercepting transport that can simulate rate limits and commit-then-429 behavior.
#[derive(Clone)]
struct RateLimitInterceptor {
    inner: Arc<UreqTransport>,
    inject_429_for_jmap: Arc<AtomicUsize>,
    inject_429_for_session: Arc<AtomicUsize>,
    inject_429_for_upload: Arc<AtomicUsize>,
    inject_429_for_download: Arc<AtomicUsize>,
    inject_method_limit_for_contact_get: Arc<AtomicUsize>,
    retry_after_seconds: u64,
    total_jmap_requests: Arc<AtomicUsize>,
    total_session_requests: Arc<AtomicUsize>,
    total_upload_requests: Arc<AtomicUsize>,
    total_download_requests: Arc<AtomicUsize>,
}

impl RateLimitInterceptor {
    fn new(retry_after_seconds: u64) -> Self {
        Self {
            inner: Arc::new(UreqTransport::default()),
            inject_429_for_jmap: Arc::new(AtomicUsize::new(0)),
            inject_429_for_session: Arc::new(AtomicUsize::new(0)),
            inject_429_for_upload: Arc::new(AtomicUsize::new(0)),
            inject_429_for_download: Arc::new(AtomicUsize::new(0)),
            inject_method_limit_for_contact_get: Arc::new(AtomicUsize::new(0)),
            retry_after_seconds,
            total_jmap_requests: Arc::new(AtomicUsize::new(0)),
            total_session_requests: Arc::new(AtomicUsize::new(0)),
            total_upload_requests: Arc::new(AtomicUsize::new(0)),
            total_download_requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn set_contact_get_method_limit_count(&self, count: usize) {
        self.inject_method_limit_for_contact_get
            .store(count, Ordering::SeqCst);
    }

    fn set_jmap_429_count(&self, count: usize) {
        self.inject_429_for_jmap.store(count, Ordering::SeqCst);
    }

    fn set_session_429_count(&self, count: usize) {
        self.inject_429_for_session.store(count, Ordering::SeqCst);
    }

    fn set_upload_429_count(&self, count: usize) {
        self.inject_429_for_upload.store(count, Ordering::SeqCst);
    }

    fn set_download_429_count(&self, count: usize) {
        self.inject_429_for_download.store(count, Ordering::SeqCst);
    }

    fn jmap_requests(&self) -> usize {
        self.total_jmap_requests.load(Ordering::SeqCst)
    }

    fn session_requests(&self) -> usize {
        self.total_session_requests.load(Ordering::SeqCst)
    }

    fn upload_requests(&self) -> usize {
        self.total_upload_requests.load(Ordering::SeqCst)
    }

    fn download_requests(&self) -> usize {
        self.total_download_requests.load(Ordering::SeqCst)
    }
}

impl Transport for RateLimitInterceptor {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_session = request.url.ends_with("/.well-known/jmap");
        let is_jmap = request.url.ends_with("/jmap") && !is_session;
        let is_upload = request.url.contains("/upload/");
        let is_download = request.url.contains("/download/");

        if is_session {
            self.total_session_requests.fetch_add(1, Ordering::SeqCst);
            let remaining = self.inject_429_for_session.load(Ordering::SeqCst);
            if remaining > 0 {
                self.inject_429_for_session.fetch_sub(1, Ordering::SeqCst);
                jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(
                    self.retry_after_seconds,
                )));
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "Rate limit exceeded on session fetch",
                    }))
                    .unwrap(),
                    final_url: request.url.to_string(),
                });
            }
        }

        if is_upload {
            self.total_upload_requests.fetch_add(1, Ordering::SeqCst);
            let response = self.inner.execute(request)?;
            let remaining = self.inject_429_for_upload.load(Ordering::SeqCst);
            if remaining > 0 {
                self.inject_429_for_upload.fetch_sub(1, Ordering::SeqCst);
                jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(
                    self.retry_after_seconds,
                )));
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "Rate limit exceeded on blob upload",
                    }))
                    .unwrap(),
                    final_url: response.final_url,
                });
            }
            return Ok(response);
        }

        if is_download {
            self.total_download_requests.fetch_add(1, Ordering::SeqCst);
            let remaining = self.inject_429_for_download.load(Ordering::SeqCst);
            if remaining > 0 {
                self.inject_429_for_download.fetch_sub(1, Ordering::SeqCst);
                jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(
                    self.retry_after_seconds,
                )));
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "Rate limit exceeded on blob download",
                    }))
                    .unwrap(),
                    final_url: request.url.to_string(),
                });
            }
            return self.inner.execute(request);
        }

        if is_jmap {
            self.total_jmap_requests.fetch_add(1, Ordering::SeqCst);

            let method_limit_remaining = self
                .inject_method_limit_for_contact_get
                .load(Ordering::SeqCst);
            let contact_card_req = if method_limit_remaining > 0 {
                serde_json::from_slice::<jmap_proto::request::Request>(
                    request.body.unwrap_or_default(),
                )
                .ok()
                .filter(|req| req.method_calls.iter().any(|c| c.name == "ContactCard/get"))
            } else {
                None
            };
            if let Some(req) = contact_card_req {
                self.inject_method_limit_for_contact_get
                    .fetch_sub(1, Ordering::SeqCst);
                let call_id = req.method_calls[0].call_id.clone();
                let body = serde_json::json!({
                    "sessionState": "state-1",
                    "methodResponses": [
                        ["error", { "type": "limit", "description": "rate limited" }, call_id]
                    ]
                });
                return Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/json".to_owned()),
                    body: serde_json::to_vec(&body).unwrap(),
                    final_url: request.url.to_string(),
                });
            }

            // Execute the request on the real mock server first.
            // This ensures writes are committed to the mock server state.
            let response = self.inner.execute(request)?;

            let remaining = self.inject_429_for_jmap.load(Ordering::SeqCst);
            if remaining > 0 {
                self.inject_429_for_jmap.fetch_sub(1, Ordering::SeqCst);
                jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(
                    self.retry_after_seconds,
                )));
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "Rate limit exceeded (Http.rateLimitAuthenticated)",
                    }))
                    .unwrap(),
                    final_url: response.final_url,
                });
            }

            return Ok(response);
        }

        self.inner.execute(request)
    }
}

/// Helper building a test server with a seeded calendar.
fn server_with_calendar() -> (MockServer, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let calendar = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    (server, account_id, calendar)
}

/// Helper building a test server with a seeded address book.
fn server_with_address_book() -> (MockServer, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };
    (server, account_id, book)
}

#[test]
fn method_level_limit_on_contact_get_is_retried_and_succeeds() {
    let (server, account_id, book) = server_with_address_book();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    let card = ContactCard::simple(book, "Test User", "test@example.com");
    let created = client
        .contact_create(&account_id, &card)
        .expect("initial contact create succeeds");

    interceptor.set_contact_get_method_limit_count(1);
    let requests_before = interceptor.jmap_requests();

    let fetched = client
        .contact_get(&account_id, &[created.id.unwrap()])
        .expect("contact_get succeeds after retry");

    assert_eq!(fetched.list.len(), 1);
    assert_eq!(interceptor.jmap_requests() - requests_before, 2);
}

#[test]
fn session_fetch_retries_on_429_and_succeeds() {
    let server = MockServer::builder().start();
    let interceptor = RateLimitInterceptor::new(0);
    interceptor.set_session_429_count(1);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds after retrying session 429");

    assert_eq!(interceptor.session_requests(), 2);
    assert!(client.session().accounts.contains_key(&server.account_id()));
}

#[test]
fn idempotent_event_get_retries_on_429_and_succeeds() {
    let (server, account_id, calendar) = server_with_calendar();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    let event = CalendarEvent::simple(calendar, "Meeting", "2026-10-19T10:00:00", "PT1H");
    let created = client
        .event_create(&account_id, &event)
        .expect("initial create succeeds");

    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let fetched = client
        .event_get(&account_id, &[created.id.unwrap()])
        .expect("event_get succeeds after retry");

    assert_eq!(fetched.list.len(), 1);
    assert_eq!(interceptor.jmap_requests() - requests_before, 2);
}

#[test]
fn idempotent_event_query_retries_on_429_and_succeeds() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let query_res = client
        .event_query(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        )
        .expect("event_query succeeds after retry");

    assert_eq!(interceptor.jmap_requests() - requests_before, 2);
    assert!(query_res.ids.is_empty());
}

#[test]
fn idempotent_event_changes_retries_on_429_and_succeeds() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    let initial_state = client
        .event_get(&account_id, &[])
        .expect("event_get state succeeds")
        .state;

    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let changes = client
        .changes(&account_id, "CalendarEvent", &initial_state)
        .expect("changes succeeds after retry");

    assert_eq!(interceptor.jmap_requests() - requests_before, 2);
    assert_eq!(changes.old_state, initial_state);
}

#[test]
fn non_idempotent_event_create_never_retries_and_returns_rate_limited() {
    let (server, account_id, calendar) = server_with_calendar();
    let interceptor = RateLimitInterceptor::new(5);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let event = CalendarEvent::simple(calendar, "Standup", "2026-10-19T10:00:00", "PT1H");
    let result = client.event_create(&account_id, &event);

    assert_eq!(
        interceptor.jmap_requests() - requests_before,
        1,
        "non-idempotent create must never retry automatically"
    );

    match result {
        Err(Error::RateLimited {
            retry_after,
            method,
            ..
        }) => {
            assert_eq!(retry_after, Some(Duration::from_secs(5)));
            assert_eq!(method.as_deref(), Some("CalendarEvent/set"));
        }
        other => panic!("expected Error::RateLimited, got {other:?}"),
    }
}

#[test]
fn commit_then_429_does_not_create_duplicate_on_server() {
    let (server, account_id, calendar) = server_with_calendar();
    let interceptor = RateLimitInterceptor::new(1);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    let event = CalendarEvent::simple(calendar, "Important Meeting", "2026-10-19T10:00:00", "PT1H");

    // Configure the interceptor to let the create reach the mock server.
    // The server commits the write, but the client receives HTTP 429.
    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let result = client.event_create(&account_id, &event);

    // The client must not retry.
    assert_eq!(
        interceptor.jmap_requests() - requests_before,
        1,
        "must not send a blind retry"
    );

    let error = result.expect_err("first attempt returned 429");
    assert!(error.is_rate_limited());
    assert_eq!(error.rate_limit_retry_after(), Some(Duration::from_secs(1)));

    // Verify exactly one event was committed to the server.
    // Querying through event_query is idempotent and will succeed.
    let query_res = client
        .event_query(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        )
        .expect("event_query succeeds");

    assert_eq!(
        query_res.ids.len(),
        1,
        "exactly one event must exist on server: no duplicate was created"
    );
}

#[test]
fn non_idempotent_email_import_never_retries_on_429() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(3);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    interceptor.set_jmap_429_count(1);
    let requests_before = interceptor.jmap_requests();

    let email_import = jmap_proto::mail::EmailImport::new(Id::new("b1"), "mb1");

    let result = client.email_import(&account_id, &email_import);

    assert_eq!(
        interceptor.jmap_requests() - requests_before,
        1,
        "email_import must not retry on rate limit"
    );

    let error = result.expect_err("email_import returned rate limit");
    assert!(error.is_rate_limited());
    assert_eq!(error.rate_limit_retry_after(), Some(Duration::from_secs(3)));
}

#[test]
fn exhausted_retries_returns_rate_limited_error() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    // Inject more 429s than the maximum retry limit (default 3 attempts).
    interceptor.set_jmap_429_count(10);
    let requests_before = interceptor.jmap_requests();

    let result = client.event_query(
        &account_id,
        jmap_proto::calendars::CalendarEventQueryFilter::default(),
    );

    assert_eq!(
        interceptor.jmap_requests() - requests_before,
        3,
        "default retry limit should be 3 attempts total"
    );

    let error = result.expect_err("exhausted retries returns error");
    assert!(error.is_rate_limited());
}

#[test]
fn cancellation_during_backoff_aborts_immediately() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(30);

    let cancel = CancelFlag::new();
    let client = Client::builder()
        .transport(interceptor.clone())
        .cancel_flag(cancel.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    interceptor.set_jmap_429_count(1);

    // Spawn a thread to cancel during the sleep.
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel_clone.cancel();
    });

    let result = client.event_query(
        &account_id,
        jmap_proto::calendars::CalendarEventQueryFilter::default(),
    );

    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn idempotent_blob_download_retries_on_429_and_succeeds() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(0);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    let uploaded = client
        .upload_blob(
            &account_id,
            "text/plain",
            b"download retry payload".to_vec(),
        )
        .expect("blob upload succeeds");

    interceptor.set_download_429_count(1);
    let requests_before = interceptor.download_requests();

    let downloaded = client
        .download_blob(&account_id, &uploaded.blob_id, "test.txt", 1024)
        .expect("blob download succeeds after retry");

    assert_eq!(downloaded, b"download retry payload");
    assert_eq!(interceptor.download_requests() - requests_before, 2);
}

#[test]
fn non_idempotent_blob_upload_never_retries_and_returns_rate_limited() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let interceptor = RateLimitInterceptor::new(7);

    let client = Client::builder()
        .transport(interceptor.clone())
        .connect(server.origin(), Credentials::bearer("test"))
        .expect("connect succeeds");

    interceptor.set_upload_429_count(1);
    let requests_before = interceptor.upload_requests();

    let result = client.upload_blob(&account_id, "text/plain", b"upload payload".to_vec());

    assert_eq!(
        interceptor.upload_requests() - requests_before,
        1,
        "blob upload must never retry automatically"
    );

    match result {
        Err(Error::RateLimited {
            retry_after,
            method,
            ..
        }) => {
            assert_eq!(retry_after, Some(Duration::from_secs(7)));
            assert_eq!(method.as_deref(), Some("upload"));
        }
        other => panic!("expected Error::RateLimited, got {other:?}"),
    }
}

#[test]
fn stale_observed_retry_after_does_not_leak_into_subsequent_headerless_429() {
    #[derive(Clone)]
    struct TwoStepTransport {
        inner: Arc<UreqTransport>,
        step: Arc<AtomicUsize>,
    }

    impl Transport for TwoStepTransport {
        fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            let is_jmap =
                request.url.ends_with("/jmap") && !request.url.ends_with("/.well-known/jmap");
            if !is_jmap {
                return self.inner.execute(request);
            }
            let current = self.step.fetch_add(1, Ordering::SeqCst);
            if current == 0 {
                // First request sets thread-local Retry-After header duration of 9s.
                jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(9)));
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "First 429 with header",
                    }))
                    .unwrap(),
                    final_url: request.url.to_string(),
                });
            }
            // Second request returns 429 with problem retryAfter=2 and no Retry-After header.
            Ok(HttpResponse {
                status: 429,
                content_type: Some("application/problem+json".to_owned()),
                body: serde_json::to_vec(&json!({
                    "type": "urn:ietf:params:jmap:error:limit",
                    "status": 429,
                    "retryAfter": 2,
                    "detail": "Second 429 without header",
                }))
                .unwrap(),
                final_url: request.url.to_string(),
            })
        }
    }

    let (server, account_id, calendar) = server_with_calendar();
    let transport = TwoStepTransport {
        inner: Arc::new(UreqTransport::default()),
        step: Arc::new(AtomicUsize::new(0)),
    };
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::bearer("test"))
        .unwrap();

    let ev = CalendarEvent::simple(calendar, "Title", "2026-10-19T10:00:00", "PT1H");
    let err1 = client.event_create(&account_id, &ev).unwrap_err();
    assert_eq!(err1.rate_limit_retry_after(), Some(Duration::from_secs(9)));

    let err2 = client.event_create(&account_id, &ev).unwrap_err();
    assert_eq!(
        err2.rate_limit_retry_after(),
        Some(Duration::from_secs(2)),
        "second 429 must use its own retryAfter=2, not the stale 9s from the earlier request"
    );
}

mod proptests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use jmap_client::transport::{
        HttpRequest, HttpResponse, Transport, TransportError, UreqTransport,
    };
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
    use proptest::prelude::*;
    use serde_json::json;

    #[derive(Clone, Debug)]
    enum StepKind {
        Create {
            inject_429: bool,
            retry_after_secs: u64,
            via_header: bool,
        },
        Query {
            inject_429_count: usize,
            retry_after_secs: u64,
            via_header: bool,
        },
        Get {
            inject_429_count: usize,
            retry_after_secs: u64,
            via_header: bool,
        },
        Changes {
            inject_429_count: usize,
            retry_after_secs: u64,
            via_header: bool,
        },
    }

    #[derive(Clone)]
    struct SequenceInterceptor {
        inner: Arc<UreqTransport>,
        plan: Arc<Mutex<VecDeque<(bool, u64, bool)>>>,
        jmap_calls: Arc<Mutex<usize>>,
    }

    impl SequenceInterceptor {
        fn new() -> Self {
            Self {
                inner: Arc::new(UreqTransport::default()),
                plan: Arc::new(Mutex::new(VecDeque::new())),
                jmap_calls: Arc::new(Mutex::new(0)),
            }
        }

        fn schedule(&self, responses: impl IntoIterator<Item = (bool, u64, bool)>) {
            let mut q = self.plan.lock().unwrap();
            q.clear();
            q.extend(responses);
        }

        fn calls(&self) -> usize {
            *self.jmap_calls.lock().unwrap()
        }
    }

    impl Transport for SequenceInterceptor {
        fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            let is_jmap =
                request.url.ends_with("/jmap") && !request.url.ends_with("/.well-known/jmap");
            if !is_jmap {
                return self.inner.execute(request);
            }
            *self.jmap_calls.lock().unwrap() += 1;

            // Commit the operation against MockServer first (simulating commit-then-429).
            let response = self.inner.execute(request)?;

            let next = self.plan.lock().unwrap().pop_front();
            if let Some((true, retry_secs, via_header)) = next {
                let body = if via_header {
                    jmap_client::transport::set_observed_retry_after(Some(Duration::from_secs(
                        retry_secs,
                    )));
                    json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "detail": "Rate limit exceeded",
                    })
                } else {
                    json!({
                        "type": "urn:ietf:params:jmap:error:limit",
                        "status": 429,
                        "retryAfter": retry_secs,
                        "detail": "Rate limit exceeded",
                    })
                };
                return Ok(HttpResponse {
                    status: 429,
                    content_type: Some("application/problem+json".to_owned()),
                    body: serde_json::to_vec(&body).unwrap(),
                    final_url: response.final_url,
                });
            }

            Ok(response)
        }
    }

    fn step_strategy() -> impl Strategy<Value = StepKind> {
        prop_oneof![
            (any::<bool>(), 0u64..=3, any::<bool>()).prop_map(
                |(inject_429, retry_after_secs, via_header)| StepKind::Create {
                    inject_429,
                    retry_after_secs,
                    via_header,
                }
            ),
            (0usize..=4, 0u64..=2, any::<bool>()).prop_map(
                |(inject_429_count, retry_after_secs, via_header)| StepKind::Query {
                    inject_429_count,
                    retry_after_secs,
                    via_header,
                }
            ),
            (0usize..=4, 0u64..=2, any::<bool>()).prop_map(
                |(inject_429_count, retry_after_secs, via_header)| StepKind::Get {
                    inject_429_count,
                    retry_after_secs,
                    via_header,
                }
            ),
            (0usize..=4, 0u64..=2, any::<bool>()).prop_map(
                |(inject_429_count, retry_after_secs, via_header)| StepKind::Changes {
                    inject_429_count,
                    retry_after_secs,
                    via_header,
                }
            ),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::with_cases(32)
        })]

        #[test]
        fn prop_commit_then_429_sequences_never_duplicate_creates_and_retry_idempotent_calls(
            max_retries in 1u32..=4,
            steps in prop::collection::vec(step_strategy(), 1..=12),
        ) {
            let server = MockServer::builder().start();
            let account_id = server.account_id();
            let calendar_id = {
                let state = server.state();
                let mut state = state.lock().unwrap();
                state
                    .account_mut(&account_id)
                    .unwrap()
                    .seed_calendar("Personal", true)
            };

            let interceptor = SequenceInterceptor::new();
            // Cap max_retry_after to 5ms so backoff sleeps stay fast while still exercising Retry-After.
            let max_backoff = Duration::from_millis(5);
            let client = Client::builder()
                .transport(interceptor.clone())
                .max_rate_limit_retries(max_retries)
                .max_retry_after(max_backoff)
                .connect(server.origin(), Credentials::bearer("test"))
                .unwrap();

            let initial_state = client.event_get(&account_id, &[]).unwrap().state;
            let mut expected_creates = 0usize;
            let mut known_ids = Vec::new();

            for (idx, step) in steps.into_iter().enumerate() {
                let before_calls = interceptor.calls();
                match step {
                    StepKind::Create {
                        inject_429,
                        retry_after_secs,
                        via_header,
                    } => {
                        interceptor.schedule([(inject_429, retry_after_secs, via_header)]);
                        let ev = CalendarEvent::simple(
                            calendar_id.clone(),
                            &format!("Step {idx}"),
                            "2026-10-20T10:00:00",
                            "PT1H",
                        );
                        let res = client.event_create(&account_id, &ev);
                        // The interceptor commits to MockServer before returning 429.
                        expected_creates += 1;

                        // Non-idempotent create must send exactly 1 HTTP request (never retried).
                        prop_assert_eq!(
                            interceptor.calls() - before_calls,
                            1,
                            "create must never retry on 429"
                        );

                        if inject_429 {
                            let err = res.expect_err("injected 429 on create must return error");
                            prop_assert!(err.is_rate_limited());
                            prop_assert_eq!(
                                err.rate_limit_retry_after(),
                                Some(Duration::from_secs(retry_after_secs))
                            );
                        } else {
                            let created = res.expect("create without 429 succeeds");
                            known_ids.push(created.id.expect("server id"));
                        }
                    }
                    StepKind::Query {
                        inject_429_count,
                        retry_after_secs,
                        via_header,
                    } => {
                        let plan: Vec<_> = (0..inject_429_count)
                            .map(|_| (true, retry_after_secs, via_header))
                            .chain(std::iter::once((false, 0, false)))
                            .collect();
                        interceptor.schedule(plan);
                        let start = Instant::now();
                        let res = client.event_query(&account_id, CalendarEventQueryFilter::default());
                        let elapsed = start.elapsed();

                        let expected_attempts = (inject_429_count + 1).min(max_retries as usize);
                        prop_assert_eq!(interceptor.calls() - before_calls, expected_attempts);

                        if inject_429_count < max_retries as usize {
                            let q = res.expect("idempotent query succeeds within retry budget");
                            prop_assert_eq!(q.ids.len(), expected_creates);
                            if inject_429_count > 0 && retry_after_secs > 0 {
                                prop_assert!(elapsed >= max_backoff * (inject_429_count as u32));
                            }
                        } else {
                            let err = res.expect_err("query exceeding retry budget fails");
                            prop_assert!(err.is_rate_limited());
                            prop_assert_eq!(
                                err.rate_limit_retry_after(),
                                Some(Duration::from_secs(retry_after_secs))
                            );
                        }
                    }
                    StepKind::Get {
                        inject_429_count,
                        retry_after_secs,
                        via_header,
                    } => {
                        let plan: Vec<_> = (0..inject_429_count)
                            .map(|_| (true, retry_after_secs, via_header))
                            .chain(std::iter::once((false, 0, false)))
                            .collect();
                        interceptor.schedule(plan);
                        let res = client.event_get(&account_id, &known_ids);
                        let expected_attempts = (inject_429_count + 1).min(max_retries as usize);
                        prop_assert_eq!(interceptor.calls() - before_calls, expected_attempts);

                        if inject_429_count < max_retries as usize {
                            let got = res.expect("idempotent get succeeds within retry budget");
                            prop_assert_eq!(got.list.len(), known_ids.len());
                        } else {
                            let err = res.expect_err("get exceeding retry budget fails");
                            prop_assert!(err.is_rate_limited());
                            prop_assert_eq!(
                                err.rate_limit_retry_after(),
                                Some(Duration::from_secs(retry_after_secs))
                            );
                        }
                    }
                    StepKind::Changes {
                        inject_429_count,
                        retry_after_secs,
                        via_header,
                    } => {
                        let plan: Vec<_> = (0..inject_429_count)
                            .map(|_| (true, retry_after_secs, via_header))
                            .chain(std::iter::once((false, 0, false)))
                            .collect();
                        interceptor.schedule(plan);
                        let res = client.changes(&account_id, "CalendarEvent", &initial_state);
                        let expected_attempts = (inject_429_count + 1).min(max_retries as usize);
                        prop_assert_eq!(interceptor.calls() - before_calls, expected_attempts);

                        if inject_429_count < max_retries as usize {
                            let ch = res.expect("idempotent changes succeeds within retry budget");
                            prop_assert_eq!(ch.created.len(), expected_creates);
                        } else {
                            let err = res.expect_err("changes exceeding retry budget fails");
                            prop_assert!(err.is_rate_limited());
                            prop_assert_eq!(
                                err.rate_limit_retry_after(),
                                Some(Duration::from_secs(retry_after_secs))
                            );
                        }
                    }
                }
            }

            // Final verification: zero duplicate creates exist on the server.
            interceptor.schedule([]);
            let final_query = client
                .event_query(&account_id, CalendarEventQueryFilter::default())
                .unwrap();
            prop_assert_eq!(
                final_query.ids.len(),
                expected_creates,
                "server must hold exactly one event per create call, with zero duplicates"
            );
        }
    }
}
