// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests request size enforcement under RFC 8620 section 2 `maxSizeRequest`.
//!
//! Exceeding the limit refuses the whole request with `urn:ietf:params:jmap:error:limit`.
//! Calls like `Email/get` with long identifier lists must split into multiple requests.

use std::time::Duration;

use jmap_client::transport::{HttpMethod, HttpRequest, Transport, UreqTransport};
use jmap_client::{Client, Credentials, Error, limits};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::request::Request;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use serde_json::{Value, json};

/// Small request limit that forces request splitting for seeded messages.
///
/// Leaves room for several identifiers per request while forcing multiple batches.
const SMALL_REQUEST: u64 = 220;

/// Number of seeded messages in the mailbox, exceeding [`SMALL_REQUEST`].
const SEEDED: usize = 40;

/// Seeds an inbox with [`SEEDED`] messages and returns their identifiers in order.
fn seed_inbox(server: &MockServer) -> Vec<Id> {
    let account_id = server.account_id();
    let state = server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&account_id).unwrap();
    let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
    (0..SEEDED)
        .map(|n| {
            account.seed_email(EmailSeed::new(
                inbox.clone(),
                ("Bob", "bob@example.com"),
                &format!("Message {n}"),
                "Body",
                "2026-08-01T10:00:00Z",
            ))
        })
        .collect()
}

/// Enforces advertised limits on the mock server.
/// Requests exceeding the limit are rejected without executing any calls.
/// Uses raw HTTP to verify server behavior independently of client checks.
#[test]
fn a_server_that_takes_a_small_request_refuses_a_larger_one() {
    let server = MockServer::builder().size_request(SMALL_REQUEST).start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let echo = json!({"hello": "w".repeat(SMALL_REQUEST as usize)});
    let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
        .call("Core/echo", &echo, "c1")
        .unwrap();
    let body = serde_json::to_vec(&request).unwrap();
    let response = UreqTransport::new(Duration::from_secs(10))
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: &client.session().api_url,
            headers: &[("Content-Type".to_owned(), "application/json".to_owned())],
            body: Some(&body),
            cancel: None,
            max_response_bytes: limits::MAX_API_RESPONSE_BYTES,
        })
        .unwrap();

    assert_eq!(response.status, 400);
    let problem: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(problem["type"], "urn:ietf:params:jmap:error:limit");
    assert_eq!(problem["limit"], "maxSizeRequest");
    assert!(
        !server.method_calls().iter().any(|name| name == "Core/echo"),
        "a refused request runs none of its calls"
    );

    // The session says so, which is what a client is meant to read.
    assert_eq!(client.session().max_size_request(), Some(SMALL_REQUEST));
    // And the account is still usable with a request that fits.
    assert!(client.mailbox_get(&account_id).is_ok());
}

/// Verifies that an identifier list exceeding the limit splits into multiple requests.
#[test]
fn a_long_email_get_is_sent_as_several_requests() {
    let server = MockServer::builder().size_request(SMALL_REQUEST).start();
    let account_id = server.account_id();
    let ids = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let emails = client.email_get(&account_id, &ids, Some(&["id"])).unwrap();

    assert_eq!(emails.len(), SEEDED, "every message comes back");
    let mut fetched: Vec<Id> = emails.into_iter().filter_map(|email| email.id).collect();
    let mut asked = ids.clone();
    fetched.sort();
    asked.sort();
    assert_eq!(
        fetched, asked,
        "and every one of them is one that was asked for"
    );

    let gets = server
        .method_calls()
        .iter()
        .filter(|name| *name == "Email/get")
        .count();
    assert!(gets > 1, "expected several Email/get calls, got {gets}");
    assert_eq!(
        server.api_requests(),
        gets,
        "one request per call — the split is across requests, not within one"
    );
}

/// Sends the full list in one request when limits allow.
/// Splitting only occurs when required by server limits.
#[test]
fn a_list_that_fits_is_one_request() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let ids = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let emails = client.email_get(&account_id, &ids, Some(&["id"])).unwrap();

    assert_eq!(emails.len(), SEEDED);
    assert_eq!(
        server
            .method_calls()
            .iter()
            .filter(|name| *name == "Email/get")
            .count(),
        1,
    );
}

/// Sends the list whole when the server advertises no request size limit.
#[test]
fn a_server_naming_no_request_size_gets_the_list_whole() {
    let server = MockServer::builder().no_size_request().start();
    let account_id = server.account_id();
    let ids = seed_inbox(&server);
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert_eq!(client.session().max_size_request(), None);
    let emails = client.email_get(&account_id, &ids, Some(&["id"])).unwrap();

    assert_eq!(emails.len(), SEEDED);
    assert_eq!(
        server
            .method_calls()
            .iter()
            .filter(|name| *name == "Email/get")
            .count(),
        1,
    );
}

/// Refuses a request early if a single identifier exceeds the limit.
#[test]
fn an_id_that_cannot_fit_alone_is_refused_without_a_request() {
    let server = MockServer::builder().size_request(SMALL_REQUEST).start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let before = server.api_requests();

    let enormous = Id::new("E".repeat(SMALL_REQUEST as usize));
    let error = client
        .email_get(&account_id, &[enormous], Some(&["id"]))
        .unwrap_err();

    match error {
        Error::RequestTooLarge { size, limit } => {
            assert_eq!(limit, SMALL_REQUEST);
            assert!(size > limit, "{size} should be over the {limit} it names");
        }
        other => panic!("expected a request-size refusal, got {other}"),
    }
    assert_eq!(
        server.api_requests(),
        before,
        "nothing was sent — the answer was in the session document"
    );
}

/// Verifies that a request whose body exactly equals `maxSizeRequest` is accepted.
#[test]
fn a_request_exactly_at_the_limit_is_sent() {
    let echo = json!({"hello": "world"});
    let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
        .call("Core/echo", &echo, "c1")
        .unwrap();
    let body_len = serde_json::to_vec(&request).unwrap().len() as u64;

    let server = MockServer::builder().size_request(body_len).start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let response = client.api_call(&request).unwrap();
    let invocation = response.responses_for("c1").next().unwrap();
    assert!(!invocation.is_error(), "a request at the limit was refused");
}

/// Rejects oversized requests locally before transmission to save a network round trip.
#[test]
fn an_oversized_request_is_refused_before_it_is_sent() {
    let server = MockServer::builder().size_request(SMALL_REQUEST).start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let before = server.api_requests();

    let echo = json!({"hello": "w".repeat(SMALL_REQUEST as usize)});
    let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
        .call("Core/echo", &echo, "c1")
        .unwrap();
    let error = client.api_call(&request).unwrap_err();

    match error {
        Error::RequestTooLarge { size, limit } => {
            assert_eq!(limit, SMALL_REQUEST);
            assert!(size > limit);
        }
        other => panic!("expected a request-size refusal, got {other}"),
    }
    assert_eq!(server.api_requests(), before, "nothing was sent");
}
