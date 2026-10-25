// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book boundary checks against C consumption.
//!
//! Verify that `EVCard` safely parses server-provided map keys.
//! Keys must not inject vCard properties.
//! Instance connections must implement `Send + Sync`.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::sync::RwLock;

use eds_sys::{E_CONTACT_FULL_NAME, E_CONTACT_UID, e_contact_get_const};
use jmap_backend_book::marshal;
use jmap_backend_core::instance::Slot;
use jmap_book_sync::BookSync;
use jmap_proto::contacts::{ContactCard, ContactEmail};

/// Hostile contact card with injected CRLF and `FN` in the email map key.
fn hostile_card() -> ContactCard {
    let mut emails = BTreeMap::new();
    emails.insert(
        "e1\r\nFN:Mallory <attacker@example.invalid>\r\nX-TAIL".to_owned(),
        {
            let mut v = ContactEmail::default();
            v.address = "vera@example.com".to_owned();
            v
        },
    );
    {
        let mut card = ContactCard::default();
        card.id = Some("C1".into());
        // Omit `name` so the card renders no legitimate `FN` property.
        // An injected `FN` would become the display name in Evolution.
        card.emails = Some(emails);
        card
    }
}

/// Verify that an injected `FN` in a map key does not appear in `EContact`.
#[test]
fn a_map_key_cannot_give_a_nameless_contact_a_display_name() {
    let vcard = jmap_vcard::card_to_vcard(&hostile_card());
    let contact = marshal::contact_from_vcard(&vcard);
    assert!(!contact.is_null(), "the sanitised vCard did not parse");

    // SAFETY: `contact` is a valid pointer from `contact_from_vcard`.
    // `e_contact_get_const` borrows a string owned by the contact.
    unsafe {
        let read = |field| {
            let raw = e_contact_get_const(contact, field).cast::<i8>();
            (!raw.is_null()).then(|| CStr::from_ptr(raw).to_string_lossy().into_owned())
        };

        assert_eq!(read(E_CONTACT_UID).as_deref(), Some("C1"));
        assert_eq!(
            read(E_CONTACT_FULL_NAME),
            None,
            "the injected FN line reached EContact"
        );

        marshal::contact_unref(contact);
    }
}

/// Verify that CRLF injection cannot inject a second vCard.
#[test]
fn a_map_key_cannot_terminate_the_card_early() {
    let mut emails = BTreeMap::new();
    emails.insert(
        "e1\r\nEND:VCARD\r\nBEGIN:VCARD\r\nFN:Mallory\r\nEND:VCARD\r\nX".to_owned(),
        {
            let mut v = ContactEmail::default();
            v.address = "vera@example.com".to_owned();
            v
        },
    );
    let card = {
        let mut card = ContactCard::default();
        card.id = Some("C1".into());
        card.emails = Some(emails);
        card
    };

    // Parameter text must not start a new vCard line.
    let vcard = jmap_vcard::card_to_vcard(&card);
    let lines = |name: &str| {
        vcard
            .split("\r\n")
            .filter(|line| line.eq_ignore_ascii_case(name))
            .count()
    };
    assert_eq!(lines("BEGIN:VCARD"), 1, "a second card appeared:\n{vcard}");
    assert_eq!(lines("END:VCARD"), 1, "the card was ended twice:\n{vcard}");

    let contact = marshal::contact_from_vcard(&vcard);
    assert!(!contact.is_null());
    // SAFETY: `contact` is a valid pointer from `contact_from_vcard`.
    unsafe {
        assert!(
            e_contact_get_const(contact, E_CONTACT_FULL_NAME).is_null(),
            "the injected card's FN reached EContact"
        );
        marshal::contact_unref(contact);
    }
}

/// Threading safety check: verifies `BookSync` and its components implement `Send + Sync`.
#[test]
fn the_connection_an_instance_holds_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<BookSync>();
    assert_send_sync::<jmap_client::Client>();
    assert_send_sync::<Slot<RwLock<Option<BookSync>>>>();
}
