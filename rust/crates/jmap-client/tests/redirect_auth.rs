// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests session discovery and blob downloads through HTTP redirects.
//!
//! Verifies that same-host redirects preserve authentication headers.
//! Also verifies that cross-host redirects do not leak credentials or accept foreign responses.

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

/// Body representing a foreign web page returned by an unauthenticated redirect.
const FOREIGN_PAGE: &[u8] = b"<html>this is not your blob</html>";

/// HTTP responder simulating an untrusted redirect target.
/// Always returns HTTP 200 with [`FOREIGN_PAGE`] to simulate unauthenticated content.
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

/// Verifies that refusing a cross-origin redirect mentions `JMAP_LIVE_SERVER_REBASE_URLS`.
/// This diagnostic guides users when advertised origins conflict with connection URLs.
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

/// Verifies that blob downloads do not declare `Accept: application/json`.
/// A blob request must allow arbitrary MIME types in RFC 7231 content negotiation.
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

/// Redirect target that serves its own session document without asking for credentials.
/// Records whether any request to it carried `Authorization`.
struct ForeignSessionHost {
    origin: String,
    saw_authorization: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ForeignSessionHost {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.2:0").expect("bind foreign session host");
        let port = server
            .server_addr()
            .to_ip()
            .expect("foreign session host has ip")
            .port();
        let origin = format!("http://127.0.0.2:{port}");
        let saw_authorization = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let origin = origin.clone();
            let saw_authorization = Arc::clone(&saw_authorization);
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            if request
                                .headers()
                                .iter()
                                .any(|h| h.field.equiv("Authorization"))
                            {
                                saw_authorization.store(true, Ordering::SeqCst);
                            }
                            let body = format!(
                                r#"{{"capabilities":{{"urn:ietf:params:jmap:core":{{"maxSizeUpload":10000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-casemap"]}},"urn:ietf:params:jmap:mail":{{}}}},"accounts":{{"a1":{{"name":"agent@example.com","isPersonal":true,"isReadOnly":false,"accountCapabilities":{{"urn:ietf:params:jmap:mail":{{}}}}}}}},"primaryAccounts":{{"urn:ietf:params:jmap:mail":"a1"}},"username":"agent","apiUrl":"{origin}/jmap/api","downloadUrl":"{origin}/jmap/download","uploadUrl":"{origin}/jmap/upload","eventSourceUrl":"{origin}/jmap/events","state":"s1"}}"#
                            );
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
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        Self {
            origin,
            saw_authorization,
            stop,
            handle: Some(handle),
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for ForeignSessionHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A session document from another origin does not get the credentials, even when it answers 200.
#[test]
fn a_session_document_from_a_foreign_redirect_target_gets_no_credentials() {
    let foreign = ForeignSessionHost::start();
    let server = RedirectingServer::start(&format!("{}/jmap/session", foreign.origin()));

    let result = Client::connect(server.origin(), Credentials::basic("agent", "sekret"));

    match result {
        Err(Error::RedirectCredentialsWithheld {
            requested,
            followed,
        }) => {
            assert_eq!(requested, server.origin());
            assert_eq!(followed, foreign.origin());
        }
        Ok(client) => {
            let _ = client.mailbox_get(&Id::new("a1"));
            panic!("the client accepted a session document from a foreign redirect target");
        }
        Err(other) => panic!("expected Error::RedirectCredentialsWithheld, got {other:?}"),
    }
    assert!(
        !foreign.saw_authorization.load(Ordering::SeqCst),
        "the foreign redirect target received the credentials"
    );
}

/// The mock's hostile discovery mode sends `/.well-known/jmap` to a foreign host that serves a session.
#[test]
fn a_well_known_redirect_to_a_foreign_session_host_gets_no_credentials() {
    let foreign = ForeignSessionHost::start();
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .well_known_redirect_to(foreign.origin())
        .start();

    let result = Client::connect(server.origin(), Credentials::basic("agent", "sekret"));

    match result {
        Err(Error::RedirectCredentialsWithheld {
            requested,
            followed,
        }) => {
            assert_eq!(requested, server.origin());
            assert_eq!(followed, foreign.origin());
        }
        Ok(_) => panic!("the client accepted a session document from a foreign redirect target"),
        Err(other) => panic!("expected Error::RedirectCredentialsWithheld, got {other:?}"),
    }
    assert!(
        !foreign.saw_authorization.load(Ordering::SeqCst),
        "the foreign redirect target received the credentials"
    );
}

/// A foreign JMAP server that knows the same credentials still gets no method call.
#[test]
fn a_well_known_redirect_to_another_jmap_server_sends_it_no_method_call() {
    let foreign = MockServer::builder()
        .basic_auth("agent", "sekret")
        .session_via_redirect()
        .start();
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .well_known_redirect_to(foreign.origin())
        .start();

    let result = Client::connect(server.origin(), Credentials::basic("agent", "sekret"));

    assert!(
        matches!(result, Err(Error::RedirectCredentialsWithheld { .. })),
        "expected Error::RedirectCredentialsWithheld, got {result:?}"
    );
    assert_eq!(
        foreign.api_requests(),
        0,
        "the foreign server got an API request"
    );
    assert!(foreign.method_calls().is_empty());
    assert_eq!(server.api_requests(), 0);
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

/// The mock's hostile advertise mode names every endpoint on a plain-http origin that is not loopback.
const PLAIN_HTTP_FOREIGN_ORIGIN: &str = "http://jmap-downgrade.invalid";

/// The client refuses the plain-http endpoints at connect, so no method call can carry the credentials.
#[test]
fn a_session_that_advertises_a_foreign_plain_http_origin_is_refused_before_any_call() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .advertise_origin(PLAIN_HTTP_FOREIGN_ORIGIN)
        .start();

    let result = Client::builder()
        .rebase_urls_to_origin(false)
        .connect(server.origin(), Credentials::basic("agent", "sekret"));

    match result {
        Err(Error::Protocol(message)) => {
            assert!(message.contains("apiUrl"), "{message}");
            assert!(message.contains(PLAIN_HTTP_FOREIGN_ORIGIN), "{message}");
        }
        Ok(_) => panic!("the client accepted plain-http endpoints on a foreign origin"),
        Err(other) => panic!("expected Error::Protocol, got {other:?}"),
    }
    assert_eq!(server.api_requests(), 0);
}

/// Rebasing moves the endpoints back to the configured origin before the check.
#[test]
fn rebasing_accepts_a_session_that_advertises_a_foreign_plain_http_origin() {
    let server = MockServer::builder()
        .basic_auth("agent", "sekret")
        .advertise_origin(PLAIN_HTTP_FOREIGN_ORIGIN)
        .start();

    let client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(server.origin(), Credentials::basic("agent", "sekret"))
        .expect("the rebased endpoints are on the configured origin");

    let sent = serde_json::json!({"advertised": "plain http"});
    assert_eq!(client.echo(sent.clone()).unwrap(), sent);
    assert_eq!(server.api_requests(), 1);
}

/// A redirect location with an uppercase scheme resolves to the target server.
#[test]
fn a_redirect_location_with_uppercase_scheme_resolves_to_absolute_target() {
    let foreign = UnauthorizedForeignHost::start();
    let foreign_without_scheme = foreign
        .origin()
        .strip_prefix("http://")
        .expect("plain http origin");
    let upper_target = format!("HTTP://{foreign_without_scheme}/jmap/session");
    let server = RedirectingServer::start(&upper_target);

    let result = Client::connect(server.origin(), Credentials::basic("agent", "sekret"));

    match result {
        Err(Error::RedirectCredentialsWithheld {
            requested,
            followed,
        }) => {
            assert_eq!(requested, server.origin());
            assert_eq!(followed, foreign.origin());
        }
        other => panic!("expected Error::RedirectCredentialsWithheld, got {other:?}"),
    }
}
