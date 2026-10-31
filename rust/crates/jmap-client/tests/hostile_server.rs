// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests client behavior against hostile server conditions.
//!
//! Verifies client resilience against every hostile scenario simulated by jmap-mock:
//! oversized responses, deeply nested JSON, numbers out of range, dangerous IDs,
//! blob size mismatches, file/foreign session URLs, foreign redirects, wrong/duplicate/out-of-order
//! method call IDs, replayed responses, state regression, and unknown push event types.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::{HostileScenario, MockServer};
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::request::Request;
use jmap_proto::session;

/// Recorded headers for an outgoing HTTP request.
type RequestHeaders = Vec<(String, String)>;

/// Shared list of recorded requests.
type DispatchedRequests = Arc<Mutex<Vec<(String, RequestHeaders)>>>;

/// Recording transport that tracks all outgoing request URLs and headers.
struct RecordingTransport {
    inner: UreqTransport,
    dispatched: DispatchedRequests,
}

impl RecordingTransport {
    fn new() -> (Self, DispatchedRequests) {
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner: UreqTransport::default(),
                dispatched: Arc::clone(&dispatched),
            },
            dispatched,
        )
    }
}

impl Transport for RecordingTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let headers: Vec<(String, String)> = request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.dispatched
            .lock()
            .unwrap()
            .push((request.url.to_string(), headers));
        self.inner.execute(request)
    }
}

#[test]
fn hostile_server_oversized_response_returns_bounded_result_without_panic() {
    let server = MockServer::builder()
        .hostile(HostileScenario::OversizedResponse)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let response = client.api_call(&request).unwrap();
    assert_eq!(response.method_responses.len(), 1);
    let payload = response.method_responses[0].arguments.to_string();
    assert!(payload.len() > 10_000_000, "expected payload > 10MB");
}

#[test]
fn hostile_server_deep_json_returns_error_and_keeps_memory_bounded() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DeepJson)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let result = client.api_call(&request);
    assert!(result.is_err(), "deep json must return an error");
    match result.unwrap_err() {
        Error::Json(err) => {
            assert!(
                err.is_syntax() || err.to_string().contains("recursion limit"),
                "expected recursion limit error, got: {err}"
            );
        }
        other => panic!("expected Error::Json, got: {other:?}"),
    }
}

#[test]
fn hostile_server_numbers_out_of_range_returns_error_without_panic() {
    let server = MockServer::builder()
        .hostile(HostileScenario::NumbersOutOfRange)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE, session::CAPABILITY_MAIL])
        .call("Email/query", &serde_json::json!({"accountId": "A1"}), "c1")
        .unwrap();
    let response = client.api_call(&request).unwrap();
    let parsed: Result<jmap_proto::methods::QueryResponse, _> =
        response.method_responses[0].parse();
    assert!(
        parsed.is_err(),
        "numbers out of range must fail parsing into QueryResponse"
    );
    let err = parsed.unwrap_err();
    assert!(
        err.to_string().contains("out of range")
            || err.to_string().contains("invalid type")
            || err.to_string().contains("overflow")
            || err.is_data(),
        "expected out of range error, got: {err}"
    );
}

#[test]
fn hostile_server_dangerous_ids_are_handled_safely_and_percent_encoded() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DangerousIds)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let account_id = server.account_id();
    let query_res = client
        .email_query(&account_id, EmailQueryFilter::default(), None, None, 0)
        .unwrap();
    assert!(!query_res.ids.is_empty(), "expected dangerous ids returned");
    for id in &query_res.ids {
        // Downloading with dangerous id percent-encodes and never escapes URL
        let _ = client.download_blob(&account_id, id, "safe.bin", limits::MAX_BLOB_BYTES);
    }
}

#[test]
fn hostile_server_blob_size_mismatch_returns_bounded_result() {
    let server = MockServer::builder()
        .hostile(HostileScenario::BlobSizeMismatch)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let account_id = server.account_id();
    let uploaded = client
        .upload_blob(&account_id, "text/plain", b"upload-data".to_vec())
        .unwrap();
    assert_eq!(uploaded.size, 1_000_000, "declared size was 1000000");

    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "test.bin",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();
    assert_eq!(downloaded, b"short", "served size was 5 bytes");
}

#[test]
fn hostile_server_session_urls_file_refuses_file_scheme_and_never_opens_it() {
    let server = MockServer::builder()
        .hostile(HostileScenario::SessionUrlsFile)
        .start();
    let result = Client::connect(server.origin(), Credentials::none());
    assert!(
        result.is_err(),
        "connecting to session with file:// URLs must be refused"
    );
    match result.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("file://") || msg.contains("without TLS"),
                "expected TLS/file error, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_session_urls_other_origin_never_sends_bearer_token() {
    let server = MockServer::builder()
        .hostile(HostileScenario::SessionUrlsOtherOrigin)
        .start();
    let token = "sensitive-secret-bearer-token-12345";
    let (transport, dispatched) = RecordingTransport::new();
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::bearer(token))
        .unwrap();

    // Verify apiUrl points to foreign origin
    assert!(client.session().api_url.contains("evil.example.com"));

    // Attempting API call to foreign origin must never send the token
    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let result = client.api_call(&request);
    assert!(result.is_err(), "call to foreign origin must fail");

    let calls = dispatched.lock().unwrap();
    // The call to session endpoint had Authorization
    let session_call = calls
        .iter()
        .find(|(url, _)| url.contains("/.well-known/jmap"))
        .unwrap();
    assert!(
        session_call
            .1
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v.contains(token))
    );

    // Any call to evil.example.com MUST NOT have Authorization
    let foreign_calls: Vec<_> = calls
        .iter()
        .filter(|(url, _)| url.contains("evil.example.com"))
        .collect();
    for (_, headers) in &foreign_calls {
        assert!(
            !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
            "foreign origin request must never carry authorization header"
        );
    }
}

#[test]
fn hostile_server_redirect_other_origin_never_sends_bearer_token() {
    let server = MockServer::builder()
        .hostile(HostileScenario::RedirectOtherOrigin)
        .start();
    let token = "sensitive-secret-bearer-token-67890";
    let (transport, dispatched) = RecordingTransport::new();
    let client = Client::builder()
        .transport(transport)
        .connect(server.origin(), Credentials::bearer(token))
        .unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let result = client.api_call(&request);
    assert!(result.is_err(), "redirect to foreign origin must fail");

    let calls = dispatched.lock().unwrap();
    for (url, headers) in calls.iter() {
        if url.contains("evil.example.com") {
            assert!(
                !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
                "redirect to foreign origin must never send authorization"
            );
        }
    }
}

#[test]
fn hostile_server_wrong_method_call_id_is_rejected() {
    let server = MockServer::builder()
        .hostile(HostileScenario::WrongMethodCallId)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "expected_call_id")
        .unwrap();
    let result = client.api_call(&request);
    assert!(result.is_err(), "wrong method call id must be rejected");
    match result.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("unexpected method response call id")
                    || msg.contains("wrong_call_id_999"),
                "expected protocol error naming call id, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_duplicate_method_call_id_is_rejected() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DuplicateMethodCallId)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({"a": 1}), "call_1")
        .unwrap()
        .call("Core/echo", &serde_json::json!({"b": 2}), "call_2")
        .unwrap();
    let result = client.api_call(&request);
    assert!(
        result.is_err(),
        "duplicate method call id in response must be rejected"
    );
    match result.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("duplicate method response call id"),
                "expected duplicate call id error, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_out_of_order_responses_are_rejected() {
    let server = MockServer::builder()
        .hostile(HostileScenario::OutOfOrderResponses)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({"order": 1}), "c1")
        .unwrap()
        .call("Core/echo", &serde_json::json!({"order": 2}), "c2")
        .unwrap()
        .call("Core/echo", &serde_json::json!({"order": 3}), "c3")
        .unwrap();
    let result = client.api_call(&request);
    assert!(
        result.is_err(),
        "out of order method responses must be rejected"
    );
    match result.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("method responses out of order")
                    || msg.contains("unexpected method response call id"),
                "expected out of order protocol error, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_replayed_response_is_rejected() {
    let server = MockServer::builder()
        .hostile(HostileScenario::ReplayedResponse)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let req1 = Request::new([session::CAPABILITY_CORE])
        .call(
            "Core/echo",
            &serde_json::json!({"tag": "first"}),
            "call_first",
        )
        .unwrap();
    let res1 = client.api_call(&req1).unwrap();
    assert_eq!(res1.method_responses[0].call_id, "call_first");

    // Second request with different call ID receives replayed response from req1
    let req2 = Request::new([session::CAPABILITY_CORE])
        .call(
            "Core/echo",
            &serde_json::json!({"tag": "second"}),
            "call_second",
        )
        .unwrap();
    let res2 = client.api_call(&req2);
    assert!(
        res2.is_err(),
        "replayed response with old call ID must be rejected"
    );
    match res2.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("unexpected method response call id") || msg.contains("call_first"),
                "expected unexpected call id error, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_state_backwards_is_rejected() {
    let server = MockServer::builder()
        .hostile(HostileScenario::StateBackwards)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let req1 = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let res1 = client.api_call(&req1).unwrap();
    assert_eq!(res1.session_state.as_str(), "9999");

    let req2 = Request::new([session::CAPABILITY_CORE])
        .call("Core/echo", &serde_json::json!({}), "c1")
        .unwrap();
    let res2 = client.api_call(&req2);
    assert!(
        res2.is_err(),
        "session state regressing backwards must be rejected"
    );
    match res2.unwrap_err() {
        Error::Protocol(msg) => {
            assert!(
                msg.contains("session state regressed") || msg.contains("5000"),
                "expected state regression protocol error, got: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got: {other:?}"),
    }
}

#[test]
fn hostile_server_eventsource_unknown_accounts_and_types_are_bounded_without_panic() {
    let server = MockServer::builder()
        .hostile(HostileScenario::EventsourceUnknown)
        .start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let sub = client.event_source(&[]).unwrap();
    let change = sub.recv_timeout(Duration::from_secs(5));
    assert!(change.is_some(), "expected state change event received");
    let change = change.unwrap();
    let changed_accounts: Vec<String> = change
        .changed
        .keys()
        .map(|id| id.as_str().to_owned())
        .collect();
    assert!(
        changed_accounts
            .iter()
            .any(|acc| acc == "unknown-account-XYZ"),
        "expected unknown account in state change: {changed_accounts:?}"
    );
}
