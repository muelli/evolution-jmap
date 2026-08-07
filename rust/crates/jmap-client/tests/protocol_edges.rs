// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Protocol edge cases: optimistic locking and back-reference failures.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;
use jmap_proto::error::method;
use jmap_proto::methods::{GetRequest, SetRequest};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_CORE};
use serde_json::json;

#[test]
fn set_ifinstate_mismatch_rejected() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let card = ContactCard::simple(book, "Locked Out", "locked@example.com");
    let request = SetRequest::<ContactCard>::new(account_id.clone())
        .create("new", card)
        .if_in_state("state-from-another-era");

    let result = client.single_call(
        &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
        "ContactCard/set",
        &request,
    );
    match result {
        Err(Error::Method(error)) => assert_eq!(error.error_type, method::STATE_MISMATCH),
        other => panic!("expected stateMismatch, got {other:?}"),
    }

    // Nothing was created.
    let state = server.state();
    let state = state.lock().unwrap();
    assert!(state.account(&account_id).unwrap().contact_cards.is_empty());
}

#[test]
fn backreference_to_failed_call_errors() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    // c0 fails (unknown method) → c1's #ids back-reference cannot resolve.
    let mut get = GetRequest::all(account_id.clone());
    get.ids_ref = Some(ResultReference {
        result_of: "c0".to_owned(),
        name: "ContactCard/query".to_owned(),
        path: "/ids".to_owned(),
    });
    let request = Request::new([CAPABILITY_CORE, CAPABILITY_CONTACTS])
        .call("ContactCard/nonexistentQuery", &json!({}), "c0")
        .unwrap()
        .call("ContactCard/get", &get, "c1")
        .unwrap();

    let response = client.api_call(&request).unwrap();
    assert_eq!(response.method_responses.len(), 2);

    let first = &response.method_responses[0];
    assert!(first.is_error());
    let error: jmap_proto::error::MethodError = first.parse().unwrap();
    assert_eq!(error.error_type, method::UNKNOWN_METHOD);

    let second = &response.method_responses[1];
    assert!(second.is_error());
    let error: jmap_proto::error::MethodError = second.parse().unwrap();
    assert_eq!(error.error_type, method::INVALID_RESULT_REFERENCE);
}
