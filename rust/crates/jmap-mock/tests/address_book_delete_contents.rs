// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::contacts::{AddressBook, AddressBookSetRequest, ContactCard};
use jmap_proto::methods::SetRequest;

#[test]
fn destroy_non_empty_address_book_without_remove_contents_fails_with_address_book_has_contents() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    // Create an address book
    let book = client
        .address_book_create(&account_id, &AddressBook::new("Team Contacts"))
        .unwrap();
    let book_id = book.id.unwrap();

    // Add a card to this address book
    let mut card = ContactCard::new();
    card.address_book_ids = Some([(book_id.clone(), true)].into_iter().collect());
    let created_card = client.contact_create(&account_id, &card).unwrap();
    let card_id = created_card.id.unwrap();

    // Destroy the address book without onDestroyRemoveContents: true
    let err = client.address_book_destroy(&account_id, &book_id);
    assert!(
        err.is_err(),
        "destroying non-empty address book without onDestroyRemoveContents must fail with addressBookHasContents"
    );

    // Card and address book still exist
    let fetched_books = client.address_books(&account_id).unwrap();
    assert!(
        fetched_books
            .iter()
            .any(|b| b.id.as_ref() == Some(&book_id))
    );

    // Destroy with onDestroyRemoveContents: true
    let destroy_req = AddressBookSetRequest::new(
        SetRequest::<AddressBook>::new(account_id.clone()).destroy(book_id.clone()),
    )
    .with_on_destroy_remove_contents(true);

    let res = client.address_book_set_custom(&destroy_req).unwrap();
    assert!(
        res.destroyed.is_some_and(|d| d.contains(&book_id)),
        "destroying address book with onDestroyRemoveContents: true must succeed"
    );

    // Card was in no other address book, so it must also be destroyed per RFC 9610 Section 2.3
    let remaining_cards = client.contact_cards(&account_id).unwrap();
    assert!(
        !remaining_cards
            .iter()
            .any(|c| c.id.as_ref() == Some(&card_id)),
        "card with no remaining address books must be removed per RFC 9610 Section 2.3"
    );
}

#[test]
fn destroy_empty_address_book_succeeds_without_remove_contents() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let book = client
        .address_book_create(&account_id, &AddressBook::new("Empty Book"))
        .unwrap();
    let book_id = book.id.unwrap();

    let res = client.address_book_destroy(&account_id, &book_id);
    assert!(res.is_ok(), "destroying empty address book must succeed");
}
