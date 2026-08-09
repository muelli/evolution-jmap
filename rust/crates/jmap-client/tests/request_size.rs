// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `maxSizeRequest` (RFC 8620 §2): the octets one request to `apiUrl` may
//! carry, against a server that takes only small ones.
//!
//! The sibling of `call_limits.rs`, and the same shape of failure — over the
//! limit the whole request is refused with
//! `urn:ietf:params:jmap:error:limit` and none of its calls run. What differs
//! is where the size comes from: not from the client chaining calls to save a
//! round trip, but from the one call it builds whose length is the user's data
//! rather than the client's choice, `Email/get` naming a list of ids.

use std::time::Duration;

use jmap_client::transport::{HttpMethod, HttpRequest, Transport, UreqTransport};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;
use jmap_proto::request::Request;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use serde_json::{Value, json};

/// Small enough that an `Email/get` naming every seeded message is well over
/// it, and large enough that a request naming one is well under — the band
/// where splitting is the difference between reading the mailbox and failing.
///
/// An `Email/get` naming no ids at all is 150 octets here (two capability URNs
/// and one method call), so this leaves room for a dozen or so of the mock's
/// short ids per request and forces several requests for [`SEEDED`] of them.
const SMALL_REQUEST: u64 = 220;

/// How many messages the mailbox holds. Enough that the id list alone is
/// several times [`SMALL_REQUEST`].
const SEEDED: usize = 40;

/// Seed an inbox with [`SEEDED`] messages and answer their ids in seed order.
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

/// The mock enforces the limit it advertises: a request longer than it is
/// refused whole, and nothing in it runs. Without this the tests below would
/// pass against a permissive mock while proving nothing.
///
/// Sent past the client on purpose — [`Client::api_call`] refuses an oversized
/// request itself (the test at the bottom of this file), so a client-side
/// assertion here would be about the client twice and about the server not at
/// all.
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

/// The point of the whole exercise: a list of ids too long for one request is
/// fetched as several, and the caller gets every message.
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

/// A server with room for the whole list gets it in one request. The split is
/// what the limit forces, not what the client does by default: every extra
/// request is a window in which another client can destroy a message the first
/// half of the list named.
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

/// A server naming no limit is taken at its word and sent the list whole, like
/// the other three limits: a number invented here would split requests it would
/// have taken.
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

/// The end of splitting: a single id whose request does not fit cannot be made
/// into two, so it is refused here, with both numbers, rather than sent for the
/// server to refuse.
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

/// The backstop under every caller that builds its own request: an oversized
/// one is refused before it is sent, with the two numbers, rather than spending
/// a round trip on a 400 that cannot be told from the other request-level
/// limits without reading its `limit` property.
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
