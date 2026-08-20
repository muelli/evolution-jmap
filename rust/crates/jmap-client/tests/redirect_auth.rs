// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Session discovery through a same-host redirect, as a real server that
//! serves `/.well-known/jmap` via a `307` to a separate path does (Stalwart
//! among them — see `docs/NIGHT-LOG.md`, "session-discovery redirect strips
//! auth"). The first live test against such a server failed with "no
//! primary account": `ureq`'s default redirect policy drops the
//! `Authorization` header even on a same-host hop, so the redirect target
//! was fetched anonymously and answered with an empty account list.
//!
//! Deliberately exercises the real `UreqTransport` (the crate's default),
//! not a fake in-memory one — the bug is in how that transport follows a
//! redirect, which only a real HTTP round trip through `ureq` reproduces.
//!
//! The second half of this file is the download-path counterpart (CURRENT
//! PRIORITY item 9): a real Fastmail account's blob download came back as
//! ~105 KB of `www.fastmail.com`'s own marketing homepage, not the message —
//! consistent with a *cross*-host redirect on the download hop, which
//! `UreqTransport`'s `SameHost` policy already denies `Authorization`
//! (correctly: it is genuinely a different, unnamed host), but which the
//! client used to hand back as if it were the blob regardless. Unlike the
//! session redirect above, this one is deliberately cross-host — a second
//! listener on a different loopback address (127.0.0.2, not 127.0.0.1) so
//! `ureq`'s own host comparison, and this test, see two different hosts the
//! way a real cross-domain redirect is, with no real network or DNS
//! involved.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::{Blob, MockServer};
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_MAIL;

#[test]
fn primary_account_resolves_through_a_same_host_session_redirect() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .session_via_redirect()
        .start();

    let client = Client::connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("authenticated session discovery should survive the redirect hop");

    assert!(
        client.session().primary_account(CAPABILITY_MAIL).is_some(),
        "the authenticated session should resolve a primary account, not the \
         anonymous one the redirect target answers with when auth is lost"
    );
}

/// The body an unrelated public page answers with — standing in for
/// Fastmail's own marketing homepage, the real content a live download
/// redirect came back with (see this file's module doc).
const FOREIGN_PAGE: &[u8] = b"<html>this is not your blob</html>";

/// A bare HTTP responder for the redirect target the download hop should
/// never be trusted through: it answers every request 200 with
/// [`FOREIGN_PAGE`], regardless of what the request carries — an
/// unauthenticated bounce is exactly what a redirect target that never saw
/// `Authorization` (stripped by `SameHost`, since it is a different host) is
/// expected to answer with, not a 401 a JMAP-oblivious host has no reason to
/// send.
struct ForeignHost {
    origin: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ForeignHost {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.2:0").expect("bind foreign host");
        let port = server
            .server_addr()
            .to_ip()
            .expect("foreign host has an IP address")
            .port();
        let origin = format!("http://127.0.0.2:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            let response = tiny_http::Response::from_data(FOREIGN_PAGE.to_vec())
                                .with_status_code(200)
                                .with_header(
                                    tiny_http::Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"text/html"[..],
                                    )
                                    .expect("content type header"),
                                );
                            let _ = request.respond(response);
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        Self {
            origin,
            stop,
            handle: Some(handle),
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for ForeignHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn a_cross_host_redirect_on_download_is_not_trusted_as_the_blob() {
    let foreign = ForeignHost::start();
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .download_via_redirect_to(foreign.origin())
        .start();
    let account_id = server.account_id();
    let blob_id = Id::new("b1");
    let real_blob = b"the actual message, never reached here".to_vec();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.blobs.insert(
            blob_id.clone(),
            Blob {
                content_type: "message/rfc822".to_owned(),
                data: real_blob.clone(),
            },
        );
    }

    let client = Client::connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("session discovery does not redirect in this test");

    let result = client.download_blob(&account_id, &blob_id, "message.eml", limits::MAX_BLOB_BYTES);

    match result {
        Err(Error::CrossOriginRedirect { .. }) => {}
        Ok(body) if body == FOREIGN_PAGE => panic!(
            "the foreign host's own unrelated page was returned as if it were \
             the blob — exactly the Fastmail failure this test reproduces"
        ),
        other => panic!("expected Error::CrossOriginRedirect, got {other:?}"),
    }
}

/// The `Accept` smell named in `docs/ROADMAP.md` CURRENT PRIORITY item 9: a
/// blob is never JSON, so a download declaring `Accept: application/json`
/// (as every other request this client makes correctly does) gives a server
/// doing RFC 7231 §5.3.2 content negotiation a legitimate reason to refuse
/// or redirect it. `download_blob` must declare something else instead.
#[test]
fn download_blob_does_not_declare_accept_application_json() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .reject_download_accept_json()
        .start();
    let account_id = server.account_id();
    let blob_id = Id::new("b1");
    let real_blob = b"the actual message".to_vec();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.blobs.insert(
            blob_id.clone(),
            Blob {
                content_type: "message/rfc822".to_owned(),
                data: real_blob.clone(),
            },
        );
    }

    let client = Client::connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("session discovery succeeds");

    let body = client
        .download_blob(&account_id, &blob_id, "message.eml", limits::MAX_BLOB_BYTES)
        .expect(
            "a download that does not claim Accept: application/json should \
             not be refused by a server doing content negotiation on it",
        );

    assert_eq!(body, real_blob);
}
