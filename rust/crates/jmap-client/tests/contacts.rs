// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contacts CRUD against the mock server (RFC 9610).

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::{ContactCard, ContactCardQueryFilter};
use serde_json::json;

fn server_with_book() -> (MockServer, Id, Id) {
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
    (server, account_id, book)
}

#[test]
fn contact_create() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let books = client.address_books(&account_id).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].id.as_ref(), Some(&book));

    let card = ContactCard::simple(book.clone(), "Vera Oldenburg", "vera@example.com");
    let created = client.contact_create(&account_id, &card).unwrap();

    let id = created.id.expect("server assigned id");
    assert!(created.uid.is_some(), "server assigns a uid");
    assert_eq!(created.card_type.as_deref(), Some("Card"));

    // White box: it is really in the store.
    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(account.contact_cards.contains(&id));
}

#[test]
fn contact_create_requires_existing_address_book() {
    let (server, account_id, _book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let card = ContactCard::simple("AB999", "Ghost", "ghost@example.com");
    match client.contact_create(&account_id, &card) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn contact_get_by_id() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Bob Builder", "bob@example.com"),
        )
        .unwrap();
    let id = created.id.unwrap();

    let response = client.contact_get(&account_id, &[id.clone()]).unwrap();
    assert_eq!(response.list.len(), 1);
    assert!(response.not_found.is_empty());
    let card = &response.list[0];
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Bob Builder")
    );
    assert_eq!(
        card.emails
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .address,
        "bob@example.com"
    );

    let missing = client.contact_get(&account_id, &[Id::new("C404")]).unwrap();
    assert!(missing.list.is_empty());
    assert_eq!(missing.not_found, vec![Id::new("C404")]);
}

#[test]
fn contact_update_and_state_advances() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Carla Craft", "carla@example.com"),
        )
        .unwrap();
    let id = created.id.unwrap();

    let state_before = client.contact_state(&account_id).unwrap();
    client
        .contact_update(&account_id, &id, json!({"name/full": "Carla Craft-Miller"}))
        .unwrap();
    let state_after = client.contact_state(&account_id).unwrap();
    assert_ne!(state_before, state_after, "state must advance on update");

    let card = client
        .contact_get(&account_id, &[id])
        .unwrap()
        .list
        .remove(0);
    assert_eq!(
        card.name.as_ref().unwrap().full.as_deref(),
        Some("Carla Craft-Miller")
    );
    // Untouched properties survive the patch.
    assert_eq!(
        card.emails
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .address,
        "carla@example.com"
    );
}

#[test]
fn contact_destroy() {
    let (server, account_id, book) = server_with_book();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Doomed Dave", "dave@example.com"),
        )
        .unwrap();
    let id = created.id.unwrap();

    client.contact_destroy(&account_id, &id).unwrap();

    let response = client.contact_get(&account_id, &[id.clone()]).unwrap();
    assert!(response.list.is_empty());
    assert_eq!(response.not_found, vec![id.clone()]);

    // Destroying again fails with notFound.
    match client.contact_destroy(&account_id, &id) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn contact_query_by_addressbook() {
    let (server, account_id, personal) = server_with_book();
    let work = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Work", false)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let private_card = client
        .contact_create(
            &account_id,
            &ContactCard::simple(personal.clone(), "Priva Te", "priva@example.com"),
        )
        .unwrap();
    let work_card = client
        .contact_create(
            &account_id,
            &ContactCard::simple(work.clone(), "Wor King", "wor@example.com"),
        )
        .unwrap();

    let personal_ids = client
        .contact_query(
            &account_id,
            ContactCardQueryFilter::in_address_book(personal),
        )
        .unwrap()
        .ids;
    assert_eq!(personal_ids, vec![private_card.id.unwrap()]);

    let work_ids = client
        .contact_query(&account_id, ContactCardQueryFilter::in_address_book(work))
        .unwrap()
        .ids;
    assert_eq!(work_ids, vec![work_card.id.unwrap()]);

    // Text search across names and addresses.
    let by_text = client
        .contact_query(
            &account_id,
            ContactCardQueryFilter {
                text: Some("wor@example.com".to_owned()),
                ..ContactCardQueryFilter::default()
            },
        )
        .unwrap()
        .ids;
    assert_eq!(by_text, work_ids);
}
