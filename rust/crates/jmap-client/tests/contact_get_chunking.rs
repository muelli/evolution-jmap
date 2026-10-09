// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ContactCard/get` naming more ids than `maxObjectsInGet` (RFC 8620 §2)
//! must split across requests, mirroring `Client::email_get`'s own chunking
//! (`jmap-client/tests/call_limits.rs`). Found the hard way scale-testing
//! item 94 against real Stalwart: `contact_get` sent every id in one call
//! and a 500-card address book, well within reach for `jmap-book-sync`'s own
//! `list_existing`, came back `requestTooLarge` instead of a listing.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;
use jmap_proto::error::method::REQUEST_TOO_LARGE;

fn server_with_book() -> (MockServer, Id, Id) {
    let server = MockServer::builder().objects_in_get(2).start();
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

/// A server enforcing `maxObjectsInGet` would refuse a `ContactCard/get`
/// naming every id at once with `requestTooLarge` — proven here by driving
/// the raw request past `contact_get`, the same way
/// `a_server_enforcing_max_objects_in_get_refuses_a_call_exceeding_it`
/// proves it for `Email/get`. `contact_get` itself must never let a caller
/// hit this.
#[test]
fn a_server_enforcing_max_objects_in_get_refuses_a_contact_card_get_exceeding_it() {
    let (server, account_id, _book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = jmap_proto::request::Request::new([
        jmap_proto::session::CAPABILITY_CORE,
        jmap_proto::session::CAPABILITY_CONTACTS,
    ])
    .call(
        "ContactCard/get",
        &serde_json::json!({
            "accountId": account_id,
            "ids": ["id1", "id2", "id3"],
        }),
        "c1",
    )
    .unwrap();

    let response = client.api_call(&request).unwrap();
    let invocation = response.responses_for("c1").next().unwrap();
    assert!(invocation.is_error());
    let error: jmap_proto::error::MethodError = invocation.parse().unwrap();
    assert_eq!(error.error_type, REQUEST_TOO_LARGE);
}

/// `Client::contact_get` naming more ids than `maxObjectsInGet` splits across
/// requests so every card is still fetched, same guarantee `email_get` gives.
#[test]
fn contact_get_splits_across_max_objects_in_get() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let mut ids = Vec::new();
    for i in 0..3 {
        let card = ContactCard::simple(
            book.clone(),
            &format!("Card {i}"),
            &format!("c{i}@example.com"),
        );
        let created = client.contact_create(&account_id, &card).unwrap();
        ids.push(created.id.expect("server assigned id"));
    }

    let before = server.api_requests();
    let response = client.contact_get(&account_id, &ids).unwrap();

    assert_eq!(response.list.len(), 3, "every card should still be fetched");
    assert_eq!(
        server.api_requests() - before,
        2,
        "three ids with maxObjectsInGet=2 split across two requests"
    );
}

/// `contact_get` with no ids (used by `Client::contact_state` to read the
/// current state cheaply) must still reach the server for a real state, not
/// synthesize one locally just because the chunking loop saw nothing to
/// chunk.
#[test]
fn contact_get_with_no_ids_still_reaches_the_server_for_the_real_state() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let state_before = client.contact_get(&account_id, &[]).unwrap().state;

    let card = ContactCard::simple(book, "Vera Oldenburg", "vera@example.com");
    client.contact_create(&account_id, &card).unwrap();

    let state_after = client.contact_get(&account_id, &[]).unwrap().state;
    assert_ne!(
        state_before, state_after,
        "state must advance after a create, proving the empty-ids call reached the server"
    );
}
