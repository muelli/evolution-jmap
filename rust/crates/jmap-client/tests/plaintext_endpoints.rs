// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that the session document cannot move the credentials off TLS.
//!
//! The session names `apiUrl`, `downloadUrl`, `uploadUrl` and `eventSourceUrl`.
//! Each of them gets the `Authorization` header, so each must keep the transport rule of the configured host.

use std::sync::{Arc, Mutex};

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use serde_json::{Value, json};

/// Records each request (URL, `Authorization` present) and answers with one session document.
struct SessionTransport {
    seen: Arc<Mutex<Vec<(String, bool)>>>,
    session: Value,
}

impl Transport for SessionTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let authorized = request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
        self.seen
            .lock()
            .unwrap()
            .push((request.url.to_owned(), authorized));
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: serde_json::to_vec(&self.session).unwrap(),
            final_url: request.url.to_owned(),
        })
    }
}

/// A session document whose four endpoints all start with `base`.
fn session_on(base: &str) -> Value {
    json!({
        "capabilities": {},
        "accounts": {},
        "primaryAccounts": {},
        "username": "vera@example.com",
        "apiUrl": format!("{base}/jmap"),
        "downloadUrl": format!("{base}/download/{{accountId}}/{{blobId}}/{{name}}"),
        "uploadUrl": format!("{base}/upload/{{accountId}}"),
        "eventSourceUrl": format!("{base}/eventsource"),
        "state": "s0",
    })
}

fn connect(origin: &str, session: Value) -> (Result<Client, Error>, Vec<(String, bool)>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let result = Client::builder()
        .transport(SessionTransport {
            seen: seen.clone(),
            session,
        })
        .connect(origin, Credentials::basic("vera", "sekret"));
    let seen = seen.lock().unwrap().clone();
    (result, seen)
}

fn assert_refused(result: Result<Client, Error>, endpoint: &str) {
    match result {
        Err(Error::Protocol(message)) => assert!(
            message.contains(endpoint),
            "the refusal should name {endpoint}: {message}"
        ),
        Err(other) => panic!("expected Error::Protocol naming {endpoint}, got {other:?}"),
        Ok(_) => panic!("a session that names a plain-http {endpoint} must be refused"),
    }
}

#[test]
fn a_tls_session_that_advertises_plain_http_endpoints_is_refused() {
    let (result, seen) = connect(
        "https://jmap.example.com",
        session_on("http://jmap.example.com"),
    );

    assert_refused(result, "apiUrl");
    assert_eq!(
        seen,
        [("https://jmap.example.com/.well-known/jmap".to_owned(), true)],
        "no request may follow the session fetch"
    );
}

#[test]
fn each_plain_http_endpoint_alone_is_refused() {
    for (endpoint, url) in [
        ("apiUrl", "http://jmap.example.com/jmap"),
        (
            "downloadUrl",
            "http://jmap.example.com/download/{accountId}/{blobId}/{name}",
        ),
        ("uploadUrl", "http://jmap.example.com/upload/{accountId}"),
        ("eventSourceUrl", "http://jmap.example.com/eventsource"),
    ] {
        let mut session = session_on("https://jmap.example.com");
        session[endpoint] = json!(url);

        let (result, _) = connect("https://jmap.example.com", session);

        assert_refused(result, endpoint);
    }
}

#[test]
fn a_plain_http_session_cannot_move_the_endpoints_to_another_plain_http_host() {
    let (result, _) = connect(
        "http://jmap.example.com",
        session_on("http://elsewhere.example.net"),
    );

    assert_refused(result, "apiUrl");
}

#[test]
fn a_plain_http_session_keeps_endpoints_on_its_own_origin() {
    // The account settings accepted this plain-http origin; the client does not decide it again.
    let (result, _) = connect(
        "http://jmap.example.com",
        session_on("http://jmap.example.com"),
    );

    result.expect("endpoints on the configured plain-http origin should be accepted");
}

#[test]
fn plain_http_endpoints_on_loopback_are_accepted() {
    for base in [
        "http://127.0.0.1:8080",
        "http://localhost:8080",
        "http://[::1]:8080",
    ] {
        let (result, _) = connect("https://jmap.example.com", session_on(base));

        result.unwrap_or_else(|error| panic!("loopback endpoints on {base} failed: {error:?}"));
    }
}

#[test]
fn tls_endpoints_on_another_host_are_accepted() {
    let (result, _) = connect("https://example.com", session_on("https://api.example.com"));

    result.expect("TLS endpoints should be accepted on any host");
}

#[test]
fn a_session_without_an_event_source_url_is_accepted() {
    let mut session = session_on("https://jmap.example.com");
    session["eventSourceUrl"] = json!("");

    let (result, _) = connect("https://jmap.example.com", session);

    result.expect("an empty eventSourceUrl names no endpoint");
}

#[test]
fn a_loopback_look_alike_host_is_not_loopback() {
    for base in [
        "http://127.0.0.1.example.net",
        "http://localhost.example.net",
        "http://user@localhost.example.net",
    ] {
        let (result, _) = connect("https://jmap.example.com", session_on(base));

        assert_refused(result, "apiUrl");
    }
}
