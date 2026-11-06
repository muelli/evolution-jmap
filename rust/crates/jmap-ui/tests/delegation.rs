// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `apply_calendar_delegation`/`apply_address_book_delegation` against the
//! mock server (ROADMAP item 164): a level grants the mapped rights, and
//! `None` revokes them again, the same shape `jmap-client/tests/calendars.rs`
//! and `contacts.rs` already use for raw `shareWith` patches.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_ui::delegation::{
    AddressBookDelegationLevel, CalendarDelegationLevel, apply_address_book_delegation,
    apply_calendar_delegation,
};

#[test]
fn a_calendar_delegation_level_grants_and_then_revokes_access() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let work = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Work", false)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    match bob_client.calendars(&account_id) {
        Err(Error::Method(method_error)) => assert_eq!(method_error.error_type, "forbidden"),
        other => panic!("expected forbidden before any grant, got {other:?}"),
    }

    apply_calendar_delegation(
        &alice,
        &account_id,
        &work,
        &bob,
        CalendarDelegationLevel::Author,
    )
    .unwrap();

    let bob_calendars = bob_client.calendars(&account_id).unwrap();
    assert_eq!(bob_calendars.len(), 1);
    let rights = bob_calendars[0].my_rights.clone().unwrap();
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_write_own, Some(true));
    assert_eq!(rights.may_write_all, None, "Author, not Editor");

    apply_calendar_delegation(
        &alice,
        &account_id,
        &work,
        &bob,
        CalendarDelegationLevel::None,
    )
    .unwrap();

    match bob_client.calendars(&account_id) {
        Err(Error::Method(method_error)) => assert_eq!(method_error.error_type, "forbidden"),
        other => panic!("expected forbidden after revoke, got {other:?}"),
    }
}

#[test]
fn an_address_book_delegation_level_grants_and_then_revokes_access() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let work = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Work", false)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    apply_address_book_delegation(
        &alice,
        &account_id,
        &work,
        &bob,
        AddressBookDelegationLevel::Reviewer,
    )
    .unwrap();

    let rights = bob_client.address_books(&account_id).unwrap()[0]
        .my_rights
        .clone()
        .unwrap();
    assert_eq!(rights.may_read, Some(true));
    assert_ne!(rights.may_write, Some(true), "Reviewer is read only");

    apply_address_book_delegation(
        &alice,
        &account_id,
        &work,
        &bob,
        AddressBookDelegationLevel::None,
    )
    .unwrap();

    match bob_client.address_books(&account_id) {
        Err(Error::Method(method_error)) => assert_eq!(method_error.error_type, "forbidden"),
        other => panic!("expected forbidden after revoke, got {other:?}"),
    }
}
