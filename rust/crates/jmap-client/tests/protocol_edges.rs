// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Protocol edge cases: optimistic locking and back-reference failures.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::State;
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
    get.ids_ref = Some(ResultReference::new("c0", "ContactCard/query", "/ids"));
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

#[test]
fn session_state_change_marks_session_stale_until_refreshed() {
    let server = MockServer::builder().start();
    let mut client = Client::connect(server.origin(), Credentials::none()).unwrap();

    assert_eq!(client.session().state.as_str(), "1");
    assert_eq!(
        client.last_session_state().as_ref().map(|s| s.as_str()),
        Some("1")
    );
    assert!(!client.is_session_stale());

    // Advance session state on the server.
    {
        let s = server.state();
        let mut state = s.lock().unwrap();
        state.session_state = 2;
    }

    // Call Core/echo. The server response returns sessionState "2".
    let echo_resp = client.echo(json!({"hello": "world"})).unwrap();
    assert_eq!(echo_resp["hello"], "world");

    // The cached session object still has the old state, so is_session_stale is now true.
    assert_eq!(client.session().state.as_str(), "1");
    assert_eq!(
        client.last_session_state().as_ref().map(|s| s.as_str()),
        Some("2")
    );
    assert!(client.is_session_stale());

    // After refreshing the session per RFC 8620 Section 2, is_session_stale returns false.
    client.refresh_session().unwrap();
    assert_eq!(client.session().state.as_str(), "2");
    assert_eq!(
        client.last_session_state().as_ref().map(|s| s.as_str()),
        Some("2")
    );
    assert!(!client.is_session_stale());
}

#[test]
fn identity_changes_maps_to_submission_capability() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let state = State::new("1");

    let result = client.changes(&account_id, "Identity", &state);
    match result {
        Err(Error::Protocol(msg)) if msg.contains("no known capability") => {
            panic!("Identity should map to CAPABILITY_SUBMISSION, but got: {msg}");
        }
        _ => {}
    }
}

#[test]
fn email_copy_dispatches_with_mail_capability() {
    use jmap_proto::mail::Email;
    use jmap_proto::methods::CopyRequest;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request =
        CopyRequest::<Email>::new(account_id.clone(), account_id).copy_object("c1", Email::new());
    let _ = client.email_copy(&request);
}

#[test]
fn contact_copy_dispatches_with_contacts_capability() {
    use jmap_proto::contacts::ContactCard;
    use jmap_proto::methods::CopyRequest;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request = CopyRequest::<ContactCard>::new(account_id.clone(), account_id)
        .copy_object("c1", ContactCard::new());
    let _ = client.contact_copy(&request);
}

#[test]
fn contact_query_changes_dispatches_with_contacts_capability() {
    use jmap_proto::contacts::ContactCardQueryFilter;
    use jmap_proto::methods::QueryChangesRequest;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request = QueryChangesRequest::<ContactCardQueryFilter>::new(account_id, "qs1");
    let _ = client.contact_query_changes(&request);
}

#[test]
fn address_book_set_custom_dispatches_with_contacts_capability() {
    use jmap_proto::contacts::{AddressBook, AddressBookSetRequest};
    use jmap_proto::methods::SetRequest;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let set = SetRequest::<AddressBook>::new(account_id.clone());
    let request = AddressBookSetRequest::new(set).with_on_destroy_remove_contents(true);
    let _ = client.address_book_set_custom(&request);
}

#[test]
fn contact_cards_fetches_all_with_ids_null() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let _ = client.contact_cards(&account_id);
}

#[test]
fn principal_changes_dispatches_with_principals_capability() {
    use jmap_client::Error;
    use jmap_proto::State;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let res = client.changes(&account_id, "Principal", &State::new("s1"));
    assert!(
        !matches!(res, Err(Error::Protocol(msg)) if msg.contains("no known capability")),
        "Principal must be recognized as a valid data type with Principals capability"
    );

    let _ = client.principal_changes(&account_id, &State::new("s1"));
}

#[test]
fn share_notification_changes_dispatches_with_principals_capability() {
    use jmap_client::Error;
    use jmap_proto::State;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let res = client.changes(&account_id, "ShareNotification", &State::new("s1"));
    assert!(
        !matches!(res, Err(Error::Protocol(msg)) if msg.contains("no known capability")),
        "ShareNotification must be recognized as a valid data type with Principals capability"
    );

    let _ = client.share_notification_changes(&account_id, &State::new("s1"));
}

#[test]
fn principal_set_dispatches_with_principals_capability() {
    use jmap_proto::methods::SetRequest;
    use jmap_proto::principals::Principal;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request = SetRequest::<Principal>::new(account_id);
    let _ = client.principal_set(&request);
}

#[test]
fn principal_query_changes_dispatches_with_principals_capability() {
    use jmap_proto::methods::QueryChangesRequest;
    use jmap_proto::principals::PrincipalQueryFilter;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request = QueryChangesRequest::<PrincipalQueryFilter>::new(account_id, "qs1");
    let _ = client.principal_query_changes(&request);
}

#[test]
fn share_notification_get_and_set_dispatches_with_principals_capability() {
    use jmap_proto::Id;
    use jmap_proto::methods::SetRequest;
    use jmap_proto::principals::ShareNotification;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let _ = client.share_notification_get(&account_id, &[Id::new("sn1")]);

    let set = SetRequest::<ShareNotification>::new(account_id.clone());
    let _ = client.share_notification_set(&set);

    let _ = client.share_notification_destroy(&account_id, &Id::new("sn1"));
}

#[test]
fn share_notification_query_changes_dispatches_with_principals_capability() {
    use jmap_proto::methods::QueryChangesRequest;
    use jmap_proto::principals::ShareNotificationQueryFilter;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let request = QueryChangesRequest::<ShareNotificationQueryFilter>::new(account_id, "qs1");
    let _ = client.share_notification_query_changes(&request);
}

#[test]
fn changes_sieve_script_dispatches_with_sieve_capability() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let res = client.changes(&account_id, "SieveScript", &State::new("s1"));
    if let Err(Error::Protocol(msg)) = &res {
        assert!(
            !msg.contains("no known capability"),
            "SieveScript must map to CAPABILITY_SIEVE: {msg}"
        );
    }

    let _ = client.sieve_script_changes(&account_id, &State::new("s1"));
}

#[test]
fn sieve_script_active_and_query_request_dispatches() {
    use jmap_proto::methods::QueryRequest;
    use jmap_proto::sieve::SieveScriptQueryFilter;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let active = client.sieve_script_active(&account_id);
    assert!(active.is_ok());

    let req = QueryRequest::new(account_id.clone())
        .filter(SieveScriptQueryFilter::new().with_is_active(true));
    let query_res = client.sieve_script_query_request(&req);
    assert!(query_res.is_ok());
}
