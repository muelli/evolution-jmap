// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::contacts::ContactCardQueryFilter;
use jmap_proto::id::Id;
use jmap_proto::state::UtcDate;
use serde_json::json;

#[test]
fn contact_card_query_filter_supports_all_rfc9610_section_3_3_1_properties() {
    let filter_json = json!({
        "inAddressBook": "book-1",
        "uid": "urn:uuid:card-1",
        "hasMember": "urn:uuid:member-123",
        "kind": "group",
        "createdBefore": "2026-11-01T00:00:00Z",
        "createdAfter": "2026-01-01T00:00:00Z",
        "updatedBefore": "2026-11-01T12:00:00Z",
        "updatedAfter": "2026-10-01T00:00:00Z",
        "text": "general text",
        "name": "Alice Smith",
        "name/given": "Alice",
        "name/surname": "Smith",
        "name/surname2": "Jones",
        "nickname": "Ali",
        "organization": "Example Corp",
        "email": "alice@example.com",
        "phone": "+1234567890",
        "onlineService": "matrix:@alice:example.com",
        "address": "123 Main St",
        "note": "Important client"
    });

    let filter: ContactCardQueryFilter = serde_json::from_value(filter_json.clone()).expect(
        "ContactCardQueryFilter must deserialize all standard RFC 9610 §3.3.1 filter fields",
    );

    assert_eq!(filter.in_address_book, Some(Id::from("book-1")));
    assert_eq!(filter.uid.as_deref(), Some("urn:uuid:card-1"));
    assert_eq!(filter.has_member.as_deref(), Some("urn:uuid:member-123"));
    assert_eq!(filter.kind.as_deref(), Some("group"));
    assert_eq!(
        filter.created_before,
        Some(UtcDate::new("2026-11-01T00:00:00Z"))
    );
    assert_eq!(
        filter.created_after,
        Some(UtcDate::new("2026-01-01T00:00:00Z"))
    );
    assert_eq!(
        filter.updated_before,
        Some(UtcDate::new("2026-11-01T12:00:00Z"))
    );
    assert_eq!(
        filter.updated_after,
        Some(UtcDate::new("2026-10-01T00:00:00Z"))
    );
    assert_eq!(filter.text.as_deref(), Some("general text"));
    assert_eq!(filter.name.as_deref(), Some("Alice Smith"));
    assert_eq!(filter.name_given.as_deref(), Some("Alice"));
    assert_eq!(filter.name_surname.as_deref(), Some("Smith"));
    assert_eq!(filter.name_surname2.as_deref(), Some("Jones"));
    assert_eq!(filter.nickname.as_deref(), Some("Ali"));
    assert_eq!(filter.organization.as_deref(), Some("Example Corp"));
    assert_eq!(filter.email.as_deref(), Some("alice@example.com"));
    assert_eq!(filter.phone.as_deref(), Some("+1234567890"));
    assert_eq!(
        filter.online_service.as_deref(),
        Some("matrix:@alice:example.com")
    );
    assert_eq!(filter.address.as_deref(), Some("123 Main St"));
    assert_eq!(filter.note.as_deref(), Some("Important client"));

    // Serialize back and verify all keys are present with exact wire names
    let serialized = serde_json::to_value(&filter).expect("serialize ContactCardQueryFilter");
    assert_eq!(serialized, filter_json);
}

#[test]
fn contact_card_query_filter_builder_methods() {
    let filter = ContactCardQueryFilter::in_address_book("book-2")
        .uid("uid-2")
        .has_member("member-2")
        .kind("individual")
        .created_before(UtcDate::new("2026-12-01T00:00:00Z"))
        .created_after(UtcDate::new("2026-01-01T00:00:00Z"))
        .updated_before(UtcDate::new("2026-12-01T12:00:00Z"))
        .updated_after(UtcDate::new("2026-06-01T00:00:00Z"))
        .text("search text")
        .name("Bob")
        .name_given("Bob")
        .name_surname("Builder")
        .name_surname2("Junior")
        .nickname("Bobby")
        .organization("Acme Corp")
        .email("bob@example.com")
        .phone("+1987654321")
        .online_service("sip:bob@example.com")
        .address("456 Market St")
        .note("General contractor");

    assert_eq!(filter.in_address_book, Some(Id::from("book-2")));
    assert_eq!(filter.uid.as_deref(), Some("uid-2"));
    assert_eq!(filter.has_member.as_deref(), Some("member-2"));
    assert_eq!(filter.kind.as_deref(), Some("individual"));
    assert_eq!(
        filter.created_before,
        Some(UtcDate::new("2026-12-01T00:00:00Z"))
    );
    assert_eq!(
        filter.created_after,
        Some(UtcDate::new("2026-01-01T00:00:00Z"))
    );
    assert_eq!(
        filter.updated_before,
        Some(UtcDate::new("2026-12-01T12:00:00Z"))
    );
    assert_eq!(
        filter.updated_after,
        Some(UtcDate::new("2026-06-01T00:00:00Z"))
    );
    assert_eq!(filter.text.as_deref(), Some("search text"));
    assert_eq!(filter.name.as_deref(), Some("Bob"));
    assert_eq!(filter.name_given.as_deref(), Some("Bob"));
    assert_eq!(filter.name_surname.as_deref(), Some("Builder"));
    assert_eq!(filter.name_surname2.as_deref(), Some("Junior"));
    assert_eq!(filter.nickname.as_deref(), Some("Bobby"));
    assert_eq!(filter.organization.as_deref(), Some("Acme Corp"));
    assert_eq!(filter.email.as_deref(), Some("bob@example.com"));
    assert_eq!(filter.phone.as_deref(), Some("+1987654321"));
    assert_eq!(
        filter.online_service.as_deref(),
        Some("sip:bob@example.com")
    );
    assert_eq!(filter.address.as_deref(), Some("456 Market St"));
    assert_eq!(filter.note.as_deref(), Some("General contractor"));
}
