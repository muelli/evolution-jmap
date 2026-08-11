// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! How many octets of response body this client will take, and whose number
//! that is.
//!
//! Nothing in JMAP bounds a *response*: RFC 8620 §2 gives the session document
//! `maxSizeRequest` and `maxSizeUpload`, both about what the client sends, and
//! there is no counterpart for what comes back. So the ceiling is this
//! repository's to choose, and until it did the number in force was `ureq`'s
//! `MAX_BODY_SIZE` — 10 MiB, applied by the convenience method the transport
//! happened to call. A limit nobody chose is a limit nobody can defend: it made
//! one photo attachment the largest message this provider could open, and said
//! so nowhere.
//!
//! Every request now carries the ceiling its response is held to, and these
//! tests are about that number being ours: that it is large enough for mail the
//! old one refused, that going over it is refused *by name* rather than
//! truncated, and that a body of exactly the ceiling is inside it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::MockServer;

/// Bigger than `ureq`'s 10 MiB default and not a round number of them, so a
/// download that stops at the default cannot pass by coincidence.
const OVER_THE_OLD_DEFAULT: usize = 11 * 1024 * 1024 + 7;

/// A [`Transport`] that notes the ceiling on every request and then does the
/// request for real.
///
/// Wrapping the real transport rather than faking one keeps the session
/// document the mock's: what is being asked here is which number the *client*
/// puts on each request, and a fake that had to invent a session document would
/// bury that in fixture.
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

/// A blob of `size` octets that is not all one byte, so a short read shows up
/// as a length *and* as content.
fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|n| (n % 251) as u8).collect()
}

/// The message that used not to fit. A blob past `ureq`'s default arrives
/// whole — every octet of it, not a prefix.
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

/// Over the ceiling is an error the caller can recognise, carrying the number
/// it went over — not a transport string it would have to match on, and not a
/// short body.
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

/// The boundary, which is a real trap rather than pedantry: `ureq`'s limiting
/// reader fails on the read *after* the last octet allowed, so a limit handed
/// straight through would reject a body of exactly that length. The ceiling
/// means "this many octets are fine", so exactly the ceiling arrives.
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

/// Every request states a ceiling, and for the JSON ones it is this crate's
/// [`limits::MAX_API_RESPONSE_BYTES`] rather than whatever a dependency would
/// have applied. The session fetch counts: it is the first request an account
/// ever makes, and it was under the invisible limit like all the others.
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

    // The upload's *response* is a JSON blob descriptor, so it is held to the
    // API ceiling too; the download is held to what its caller asked for.
    let download = seen.last().expect("the download was made");
    assert_eq!(download.1, 4096);
    assert!(
        seen[..seen.len() - 1]
            .iter()
            .all(|(_, limit)| *limit == limits::MAX_API_RESPONSE_BYTES),
        "every JSON response is held to the API ceiling: {seen:?}"
    );
}
