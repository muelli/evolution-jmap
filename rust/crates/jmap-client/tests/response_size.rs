// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests client response body limits.
//!
//! RFC 8620 bounds request and upload sizes but leaves response ceilings to client implementation.
//! Every request specifies an explicit maximum response size.
//! Exceeding the ceiling returns `Error::ResponseTooLarge` rather than silent truncation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::MockServer;

/// Test blob size exceeding 10 MiB to verify large download handling.
const OVER_THE_OLD_DEFAULT: usize = 11 * 1024 * 1024 + 7;

/// [`Transport`] wrapper that records the response ceiling on each request.
struct Recording {
    inner: UreqTransport,
    seen: Arc<Mutex<Vec<(String, u64)>>>,
}

impl Transport for Recording {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        self.seen
            .lock()
            .unwrap()
            .push((request.url.to_owned(), request.max_response_bytes));
        self.inner.execute(request)
    }
}

/// Creates a test blob of `size` bytes with varying octet values.
fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|n| (n % 251) as u8).collect()
}

/// Verifies that blobs larger than 10 MiB download completely.
#[test]
fn a_blob_larger_than_the_dependency_default_arrives_whole() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let data = payload(OVER_THE_OLD_DEFAULT);
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", data.clone())
        .unwrap();

    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "big.bin",
            limits::MAX_BLOB_BYTES,
        )
        .expect("a blob over 10 MiB is not over this client's ceiling");
    assert_eq!(downloaded.len(), data.len());
    assert_eq!(downloaded, data);
}

/// Verifies that responses exceeding the ceiling return `Error::ResponseTooLarge`.
#[test]
fn a_body_over_the_ceiling_is_refused_by_the_number_it_passed() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let data = payload(64 * 1024);
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", data.clone())
        .unwrap();

    let ceiling = (data.len() - 1) as u64;
    let error = client
        .download_blob(&account_id, &uploaded.blob_id, "big.bin", ceiling)
        .expect_err("one octet over the ceiling is over it");
    match error {
        Error::ResponseTooLarge { limit } => assert_eq!(limit, ceiling),
        other => panic!("expected ResponseTooLarge, got {other:?}"),
    }
}

/// Verifies that a response body exactly matching the ceiling is accepted.
#[test]
fn a_body_of_exactly_the_ceiling_arrives() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let data = payload(64 * 1024);
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", data.clone())
        .unwrap();

    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "exact.bin",
            data.len() as u64,
        )
        .expect("a body of exactly the ceiling is within it");
    assert_eq!(downloaded, data);
}

/// Verifies that API requests apply [`limits::MAX_API_RESPONSE_BYTES`].
#[test]
fn every_request_states_the_ceiling_its_response_is_held_to() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let seen = Arc::new(Mutex::new(Vec::new()));

    let client = Client::builder()
        .transport(Recording {
            inner: UreqTransport::new(Duration::from_secs(10)),
            seen: Arc::clone(&seen),
        })
        .connect(server.origin(), Credentials::none())
        .unwrap();
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", payload(1024))
        .unwrap();
    client
        .download_blob(&account_id, &uploaded.blob_id, "b.bin", 4096)
        .unwrap();

    let seen = seen.lock().unwrap();
    let session = seen.first().expect("connecting fetches the session");
    assert!(
        session.0.ends_with("/.well-known/jmap"),
        "first request is the session document, got {}",
        session.0
    );
    assert_eq!(session.1, limits::MAX_API_RESPONSE_BYTES);

    // Upload responses use the API ceiling, while downloads use the caller ceiling.
    let download = seen.last().expect("the download was made");
    assert_eq!(download.1, 4096);
    assert!(
        seen[..seen.len() - 1]
            .iter()
            .all(|(_, limit)| *limit == limits::MAX_API_RESPONSE_BYTES),
        "every JSON response is held to the API ceiling: {seen:?}"
    );
}
