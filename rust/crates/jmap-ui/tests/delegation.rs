// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `apply_calendar_delegation`/`apply_address_book_delegation` against the
//! mock server (ROADMAP item 164): a level grants the mapped rights, and
//! `None` revokes them again, the same shape `jmap-client/tests/calendars.rs`
//! and `contacts.rs` already use for raw `shareWith` patches.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::principals::Principal;
use jmap_ui::delegation::{
    AddressBookDelegationLevel, CalendarDelegationLevel, Delegate, PrincipalSearch,
    account_delegates, apply_address_book_delegation, apply_calendar_delegation,
    commit_address_book_delegation, commit_calendar_delegation, search_principals,
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

#[test]
fn account_delegates_reads_back_grants_on_a_calendar_and_a_book() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder().bearer_token("alice-token").start();
    let account_id = server.account_id();
    let (calendar, book) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        (
            account.seed_calendar("Work", false),
            account.seed_address_book("Contacts", false),
        )
    };
    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();

    assert_eq!(account_delegates(&alice, &account_id).unwrap(), vec![]);

    apply_calendar_delegation(
        &alice,
        &account_id,
        &calendar,
        &bob,
        CalendarDelegationLevel::Reviewer,
    )
    .unwrap();
    apply_address_book_delegation(
        &alice,
        &account_id,
        &book,
        &bob,
        AddressBookDelegationLevel::Author,
    )
    .unwrap();

    assert_eq!(
        account_delegates(&alice, &account_id).unwrap(),
        vec![Delegate {
            principal_id: bob.clone(),
            calendar_level: CalendarDelegationLevel::Reviewer,
            address_book_level: AddressBookDelegationLevel::Author,
        }]
    );

    apply_calendar_delegation(
        &alice,
        &account_id,
        &calendar,
        &bob,
        CalendarDelegationLevel::None,
    )
    .unwrap();

    assert_eq!(
        account_delegates(&alice, &account_id).unwrap(),
        vec![Delegate {
            principal_id: bob,
            calendar_level: CalendarDelegationLevel::None,
            address_book_level: AddressBookDelegationLevel::Author,
        }]
    );
}

#[test]
fn search_principals_finds_a_matching_principal_by_text() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let bob = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state.account_mut(&account_id).unwrap().seed_principal(
            Principal::new("Bob Example")
                .with_type("individual")
                .with_email("bob@example.com"),
        )
    };

    let alice = Client::connect(server.origin(), Credentials::none()).unwrap();

    let found = search_principals(&alice, &account_id, "bob").unwrap();
    match found {
        PrincipalSearch::Found(principals) => {
            assert_eq!(principals.len(), 1);
            assert_eq!(principals[0].id.as_ref(), Some(&bob));
            assert_eq!(principals[0].email.as_deref(), Some("bob@example.com"));
        }
        PrincipalSearch::QueryRefused => panic!("expected a match, got QueryRefused"),
    }
}

#[test]
fn search_principals_finds_nothing_for_an_unknown_search() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let alice = Client::connect(server.origin(), Credentials::none()).unwrap();

    let found = search_principals(&alice, &account_id, "nobody").unwrap();
    assert_eq!(found, PrincipalSearch::Found(vec![]));
}

#[test]
fn search_principals_falls_back_to_manual_entry_when_the_server_refuses_the_query() {
    let server = MockServer::builder().disable_principal_query().start();
    let account_id = server.account_id();
    let alice = Client::connect(server.origin(), Credentials::none()).unwrap();

    let found = search_principals(&alice, &account_id, "bob").unwrap();
    assert_eq!(found, PrincipalSearch::QueryRefused);
}

#[test]
fn committing_a_calendar_level_grants_it_on_every_own_calendar() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_calendar("Work", false);
        account.seed_calendar("Home", false);
    }

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Reviewer)
        .unwrap();

    let bob_calendars = bob_client.calendars(&account_id).unwrap();
    assert_eq!(bob_calendars.len(), 2, "both calendars must be shared");
    for calendar in &bob_calendars {
        let rights = calendar.my_rights.clone().unwrap();
        assert_eq!(rights.may_read_items, Some(true));
        assert_ne!(rights.may_write_own, Some(true), "Reviewer is read only");
    }

    commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::None).unwrap();

    match bob_client.calendars(&account_id) {
        Err(Error::Method(method_error)) => assert_eq!(method_error.error_type, "forbidden"),
        other => panic!("expected forbidden after revoke, got {other:?}"),
    }
}

#[test]
fn committing_an_address_book_level_grants_it_on_every_own_book() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.seed_address_book("Work", false);
        account.seed_address_book("Personal", false);
    }

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    commit_address_book_delegation(
        &alice,
        &account_id,
        &bob,
        AddressBookDelegationLevel::Author,
    )
    .unwrap();

    let bob_books = bob_client.address_books(&account_id).unwrap();
    assert_eq!(bob_books.len(), 2, "both address books must be shared");
    for book in &bob_books {
        let rights = book.my_rights.clone().unwrap();
        assert_eq!(rights.may_write, Some(true));
    }

    commit_address_book_delegation(&alice, &account_id, &bob, AddressBookDelegationLevel::None)
        .unwrap();

    match bob_client.address_books(&account_id) {
        Err(Error::Method(method_error)) => assert_eq!(method_error.error_type, "forbidden"),
        other => panic!("expected forbidden after revoke, got {other:?}"),
    }
}
