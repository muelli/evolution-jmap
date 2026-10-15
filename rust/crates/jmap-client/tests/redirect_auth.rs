// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Session discovery through a same-host redirect, as a real server that
//! serves `/.well-known/jmap` via a `307` to a separate path does (Stalwart
//! among them). The first live test against such a server failed with "no
//! primary account": `ureq`'s default redirect policy drops the
//! `Authorization` header even on a same-host hop, so the redirect target
//! was fetched anonymously and answered with an empty account list.
//!
//! Deliberately exercises the real `UreqTransport` (the crate's default),
//! not a fake in-memory one: the bug is in how that transport follows a
//! redirect, which only a real HTTP round trip through `ureq` reproduces.
//!
//! The second half of this file is the download-path counterpart:
//! a real Fastmail account's blob download came back as
//! ~105 KB of `www.fastmail.com`'s own marketing homepage, not the message:
//! consistent with a *cross*-host redirect on the download hop, which
//! `UreqTransport`'s `SameHost` policy already denies `Authorization`
//! (correctly: it is genuinely a different, unnamed host), but which the
//! client used to hand back as if it were the blob regardless. Unlike the
//! session redirect above, this one is deliberately cross-host: a second
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

/// The body an unrelated public page answers with, standing in for
/// Fastmail's own marketing homepage, the real content a live download
/// redirect came back with (see this file's module doc).
const FOREIGN_PAGE: &[u8] = b"<html>this is not your blob</html>";

/// A bare HTTP responder for the redirect target the download hop should
/// never be trusted through: it answers every request 200 with
/// [`FOREIGN_PAGE`], regardless of what the request carries: an
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

/// A rebase that actually changes `downloadUrl`'s origin (the advertised
/// origin differs from the one the client connected through) followed by a
/// cross-origin-redirect refusal must name
/// `JMAP_LIVE_SERVER_REBASE_URLS` in the error; otherwise a poisoned
/// account's rebase looks like an ordinary client bug, which is exactly what
/// cost a full debugging session before the env var was found by hand.
#[test]
fn a_rebased_then_refused_download_names_the_rebase_env_var() {
    let foreign = ForeignHost::start();
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .advertise_origin("https://stale-advertised-host.example")
        .download_via_redirect_to(foreign.origin())
        .start();
    let account_id = server.account_id();
    let blob_id = Id::new("b1");
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.blobs.insert(
            blob_id.clone(),
            Blob {
                content_type: "message/rfc822".to_owned(),
                data: b"the actual message, never reached here".to_vec(),
            },
        );
    }

    let client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("connect through the real origin, session names a different one");

    let result = client.download_blob(&account_id, &blob_id, "message.eml", limits::MAX_BLOB_BYTES);

    match result {
        Err(err @ Error::CrossOriginRedirect { .. }) => {
            let message = err.to_string();
            assert!(
                message.contains("JMAP_LIVE_SERVER_REBASE_URLS"),
                "a rebased-then-refused download's error should name the env \
                 var responsible, not look like an ordinary cross-origin- \
                 redirect bug: {message}"
            );
        }
        other => panic!("expected Error::CrossOriginRedirect, got {other:?}"),
    }
}

/// The `Accept` smell: a blob is never JSON, so a download declaring
/// `Accept: application/json`
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

struct TrailingDotServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TrailingDotServer {
    fn start(redirect_to_dot: bool) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind trailing dot server");
        let port = server.server_addr().to_ip().expect("server has ip").port();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            let path = request.url().split('?').next().unwrap_or_default();
                            if path == "/.well-known/jmap" {
                                let target_host = if redirect_to_dot {
                                    "localhost."
                                } else {
                                    "localhost"
                                };
                                let location = format!("http://{target_host}:{port}/jmap/session");
                                let response = tiny_http::Response::empty(307).with_header(
                                    tiny_http::Header::from_bytes(
                                        &b"Location"[..],
                                        location.as_bytes(),
                                    )
                                    .expect("location header"),
                                );
                                let _ = request.respond(response);
                            } else if path == "/jmap/session" {
                                let has_auth = request.headers().iter().any(|h| {
                                    h.field.equiv("Authorization")
                                        && h.value.as_str().starts_with("Basic ")
                                });
                                let body = if has_auth {
                                    format!(
                                        r#"{{"capabilities":{{"urn:ietf:params:jmap:core":{{"maxSizeUpload":10000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-casemap"]}},"urn:ietf:params:jmap:mail":{{}}}},"accounts":{{"a1":{{"name":"agent@example.com","isPersonal":true,"isReadOnly":false,"accountCapabilities":{{"urn:ietf:params:jmap:mail":{{}}}}}}}},"primaryAccounts":{{"urn:ietf:params:jmap:mail":"a1"}},"username":"agent","apiUrl":"http://localhost:{port}/jmap/api","downloadUrl":"http://localhost:{port}/jmap/download","uploadUrl":"http://localhost:{port}/jmap/upload","eventSourceUrl":"http://localhost:{port}/jmap/events","state":"s1"}}"#
                                    )
                                } else {
                                    format!(
                                        r#"{{"capabilities":{{"urn:ietf:params:jmap:core":{{"maxSizeUpload":10000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-casemap"]}}}},"accounts":{{}},"primaryAccounts":{{}},"username":"","apiUrl":"http://localhost:{port}/jmap/api","downloadUrl":"http://localhost:{port}/jmap/download","uploadUrl":"http://localhost:{port}/jmap/upload","eventSourceUrl":"http://localhost:{port}/jmap/events","state":"s1"}}"#
                                    )
                                };
                                let response = tiny_http::Response::from_data(body.into_bytes())
                                    .with_status_code(200)
                                    .with_header(
                                        tiny_http::Header::from_bytes(
                                            &b"Content-Type"[..],
                                            &b"application/json"[..],
                                        )
                                        .expect("content type header"),
                                    );
                                let _ = request.respond(response);
                            } else {
                                let _ = request.respond(tiny_http::Response::empty(404));
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for TrailingDotServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct UnauthorizedForeignHost {
    origin: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl UnauthorizedForeignHost {
    fn start() -> Self {
        let server =
            tiny_http::Server::http("127.0.0.2:0").expect("bind unauthorized foreign host");
        let port = server
            .server_addr()
            .to_ip()
            .expect("unauthorized host has ip")
            .port();
        let origin = format!("http://127.0.0.2:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            let response = tiny_http::Response::empty(401).with_header(
                                tiny_http::Header::from_bytes(
                                    &b"WWW-Authenticate"[..],
                                    &b"Bearer"[..],
                                )
                                .expect("www-authenticate header"),
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

impl Drop for UnauthorizedForeignHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct RedirectingServer {
    origin: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RedirectingServer {
    fn start(target: &str) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind redirecting server");
        let port = server.server_addr().to_ip().expect("server has ip").port();
        let origin = format!("http://127.0.0.1:{port}");
        let target = target.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            let response = tiny_http::Response::empty(302).with_header(
                                tiny_http::Header::from_bytes(&b"Location"[..], target.as_bytes())
                                    .expect("location header"),
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

impl Drop for RedirectingServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn session_fetch_survives_trailing_dot_redirect_to_dotless_host() {
    let server = TrailingDotServer::start(false);
    let client = Client::connect(
        &format!("http://localhost.:{}", server.port),
        Credentials::basic("agent", "sekret"),
    )
    .expect("authenticated session discovery should survive redirect to dotless host");

    assert!(
        client.session().primary_account(CAPABILITY_MAIL).is_some(),
        "credentials must be kept when redirecting from trailing-dot to dotless host"
    );
}

#[test]
fn session_fetch_survives_dotless_redirect_to_trailing_dot_host() {
    let server = TrailingDotServer::start(true);
    let client = Client::connect(
        &format!("http://localhost:{}", server.port),
        Credentials::basic("agent", "sekret"),
    )
    .expect("authenticated session discovery should survive redirect to trailing-dot host");

    assert!(
        client.session().primary_account(CAPABILITY_MAIL).is_some(),
        "credentials must be kept when redirecting from dotless to trailing-dot host"
    );
}

#[test]
fn cross_origin_redirect_with_credentials_returns_redirect_credentials_withheld_error() {
    let foreign = UnauthorizedForeignHost::start();
    let server = RedirectingServer::start(&format!("{}/jmap/session", foreign.origin()));

    let result = Client::connect(server.origin(), Credentials::basic("agent", "sekret"));

    match result {
        Err(
            ref err @ Error::RedirectCredentialsWithheld {
                ref requested,
                ref followed,
            },
        ) => {
            assert!(
                err.is_redirect_credentials_withheld(),
                "predicate should identify the withheld credentials error"
            );
            assert!(
                !err.is_unauthorized(),
                "withheld credentials error must not be classified as a rejected token"
            );
            assert_eq!(requested, server.origin());
            assert_eq!(followed, foreign.origin());
        }
        other => panic!("expected Error::RedirectCredentialsWithheld, got {other:?}"),
    }
}

#[test]
#[ignore = "touches live Fastmail server without credentials"]
fn fastmail_live_trailing_dot_session_redirect() {
    let result = Client::connect("https://api.fastmail.com.", Credentials::none());
    match result {
        Err(Error::Http { status: 401, .. }) => {}
        other => {
            panic!("expected Error::Http 401 on unauthenticated Fastmail redirect, got {other:?}")
        }
    }
}
