// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::contacts::{AddressBook, ContactCard, ContactCardQueryFilter};
use jmap_proto::methods::{Comparator, QueryRequest, QueryResponse};
use jmap_proto::state::UtcDate;

#[test]
fn contact_card_query_sorts_by_created_and_updated() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let book = client
        .address_book_create(&account_id, &AddressBook::new("Book"))
        .unwrap();
    let book_id = book.id.unwrap();

    let mut card1 = ContactCard::new();
    card1.address_book_ids = Some([(book_id.clone(), true)].into_iter().collect());
    card1.created = Some(UtcDate::new("2026-01-01T00:00:00Z"));
    card1.updated = Some(UtcDate::new("2026-06-01T00:00:00Z"));
    let c1 = client.contact_create(&account_id, &card1).unwrap();

    let mut card2 = ContactCard::new();
    card2.address_book_ids = Some([(book_id.clone(), true)].into_iter().collect());
    card2.created = Some(UtcDate::new("2026-02-01T00:00:00Z"));
    card2.updated = Some(UtcDate::new("2026-05-01T00:00:00Z"));
    let c2 = client.contact_create(&account_id, &card2).unwrap();

    let id1 = c1.id.unwrap();
    let id2 = c2.id.unwrap();

    // Query sorted by created ascending: c1, then c2
    let req_created_asc = QueryRequest::<ContactCardQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::ascending("created")]);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req_created_asc,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![id1.clone(), id2.clone()]);

    // Query sorted by created descending: c2, then c1
    let req_created_desc = QueryRequest::<ContactCardQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::descending("created")]);
    let res_desc: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req_created_desc,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res_desc.ids, vec![id2.clone(), id1.clone()]);

    // Query sorted by updated ascending: c2 (May), then c1 (June)
    let req_updated_asc = QueryRequest::<ContactCardQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::ascending("updated")]);
    let res_updated: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req_updated_asc,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res_updated.ids, vec![id2, id1]);
}

#[test]
fn contact_card_query_filters_by_rfc9610_properties() {
    use jmap_proto::contacts::{
        Address, ContactEmail, ContactPhone, Name, NameComponent, Nickname, Note, OnlineService,
        Organization,
    };
    use std::collections::BTreeMap;

    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let book = client
        .address_book_create(&account_id, &AddressBook::new("FilterBook"))
        .unwrap();
    let book_id = book.id.unwrap();

    let mut card = ContactCard::new();
    card.address_book_ids = Some([(book_id.clone(), true)].into_iter().collect());
    card.created = Some(UtcDate::new("2026-03-01T00:00:00Z"));
    card.updated = Some(UtcDate::new("2026-03-15T00:00:00Z"));
    card.name = Some(Name::new("Alice Smith").with_components(vec![
        NameComponent::new("given", "Alice"),
        NameComponent::new("surname", "Smith"),
    ]));
    card.nicknames = Some(BTreeMap::from([("n1".to_string(), Nickname::new("Ali"))]));
    card.emails = Some(BTreeMap::from([(
        "e1".to_string(),
        ContactEmail::new("alice@example.org"),
    )]));
    card.phones = Some(BTreeMap::from([(
        "p1".to_string(),
        ContactPhone::new("+15551234"),
    )]));
    let mut org = Organization::default();
    org.name = Some("Acme Widgets".to_string());
    card.organizations = Some(BTreeMap::from([("o1".to_string(), org)]));
    card.notes = Some(BTreeMap::from([(
        "not1".to_string(),
        Note::new("VIP client"),
    )]));
    let mut addr = Address::default();
    addr.full = Some("100 Market St".to_string());
    card.addresses = Some(BTreeMap::from([("a1".to_string(), addr)]));
    let mut svc = OnlineService::default();
    svc.service = Some("Matrix".to_string());
    svc.user = Some("@alice:matrix.org".to_string());
    card.online_services = Some(BTreeMap::from([("s1".to_string(), svc)]));

    let created_card = client.contact_create(&account_id, &card).unwrap();
    let card_id = created_card.id.unwrap();

    // Query with filter matching given name
    let filter = ContactCardQueryFilter::default().name_given("Alice");
    let req = QueryRequest::new(account_id.clone()).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![card_id.clone()]);

    // Query with filter matching surname
    let filter = ContactCardQueryFilter::default().name_surname("Smith");
    let req = QueryRequest::new(account_id.clone()).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![card_id.clone()]);

    // Query with filter matching nickname
    let filter = ContactCardQueryFilter::default().nickname("Ali");
    let req = QueryRequest::new(account_id.clone()).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![card_id.clone()]);

    // Query with filter matching note
    let filter = ContactCardQueryFilter::default().note("VIP");
    let req = QueryRequest::new(account_id.clone()).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![card_id.clone()]);

    // Query with filter matching createdBefore / createdAfter
    let filter = ContactCardQueryFilter::default()
        .created_after(UtcDate::new("2026-02-01T00:00:00Z"))
        .created_before(UtcDate::new("2026-04-01T00:00:00Z"));
    let req = QueryRequest::new(account_id.clone()).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.ids, vec![card_id.clone()]);

    // Query with non-matching filter returns empty
    let filter = ContactCardQueryFilter::default().name_given("NonExistent");
    let req = QueryRequest::new(account_id).filter(filter);
    let res: QueryResponse = serde_json::from_value(
        client
            .single_call(
                &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
                "ContactCard/query",
                &req,
            )
            .unwrap(),
    )
    .unwrap();
    assert!(res.ids.is_empty());
}
