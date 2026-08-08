// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The server-controlled half of the address book boundary, checked against the
//! C that consumes it.
//!
//! `jmap-vcard`'s own `tests/hostile.rs` pins the text this crate renders. This
//! file is the other half of the same finding: what `EVCard` — the parser that
//! actually decides what lands in the user's address book — makes of that text.
//! A property the server managed to inject would be invisible in a string
//! comparison and perfectly visible here.
//!
//! See `docs/AUDIT-FFI.md`, findings F1 and F7.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::sync::RwLock;

use eds_sys::{E_CONTACT_FULL_NAME, E_CONTACT_UID, e_contact_get_const};
use jmap_backend_book::marshal;
use jmap_backend_core::instance::Slot;
use jmap_book_sync::BookSync;
use jmap_proto::contacts::{ContactCard, ContactEmail};

/// The card a hostile server sends: the `emails` map key carries a line break
/// and a full `FN` line, aimed at the `X-JMAP-KEY` parameter this mapping
/// round-trips it through.
fn hostile_card() -> ContactCard {
    let mut emails = BTreeMap::new();
    emails.insert(
        "e1\r\nFN:Mallory <attacker@example.invalid>\r\nX-TAIL".to_owned(),
        ContactEmail {
            address: "vera@example.com".to_owned(),
            ..ContactEmail::default()
        },
    );
    ContactCard {
        id: Some("C1".into()),
        // No `name`, deliberately: a card that names nobody renders no `FN`, so
        // the injected one would be the only one and would *be* the display
        // name Evolution shows. With a legitimate `FN` already on the card,
        // `EVCard` keeps the first and the injection is invisible from here
        // even though it happened.
        emails: Some(emails),
        ..ContactCard::default()
    }
}

/// The exploit, run all the way into `EContact`: the contact has no display
/// name, so a display name appearing at all is one the map key smuggled in.
#[test]
fn a_map_key_cannot_give_a_nameless_contact_a_display_name() {
    let vcard = jmap_vcard::card_to_vcard(&hostile_card());
    let contact = marshal::contact_from_vcard(&vcard);
    assert!(!contact.is_null(), "the sanitised vCard did not parse");

    // SAFETY: `contact` is the reference `contact_from_vcard` just handed over,
    // and `e_contact_get_const` borrows a string the contact owns.
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

/// And it cannot add a *second* card either, which is the other thing a CRLF
/// buys: `END:VCARD` followed by a `BEGIN:VCARD` of the server's choosing.
#[test]
fn a_map_key_cannot_terminate_the_card_early() {
    let mut emails = BTreeMap::new();
    emails.insert(
        "e1\r\nEND:VCARD\r\nBEGIN:VCARD\r\nFN:Mallory\r\nEND:VCARD\r\nX".to_owned(),
        ContactEmail {
            address: "vera@example.com".to_owned(),
            ..ContactEmail::default()
        },
    );
    let card = ContactCard {
        id: Some("C1".into()),
        emails: Some(emails),
        ..ContactCard::default()
    };

    // Counted as *lines*: the text survives inside the quoted parameter, which
    // is data and stays data — what must not happen is it starting a line.
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
    // SAFETY: as above.
    unsafe {
        assert!(
            e_contact_get_const(contact, E_CONTACT_FULL_NAME).is_null(),
            "the injected card's FN reached EContact"
        );
        marshal::contact_unref(contact);
    }
}

/// F7: the threading claim the instance struct rests on, made a compile error
/// rather than a comment.
///
/// EDS dispatches a meta backend's read-only vfuncs from more than one thread,
/// and `with_connection` hands each of them a `&BookSync` taken from the same
/// [`Slot`]. Nothing in the type system checks that: the instance arrives as a
/// raw pointer and is turned into a `&` by hand, so the compiler never sees the
/// sharing and would not object if `BookSync` — or the `Client` and boxed
/// `Transport` inside it — grew an `Rc` or a `RefCell`. This is that check,
/// written where a change to `jmap-client` would trip over it.
#[test]
fn the_connection_an_instance_holds_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<BookSync>();
    assert_send_sync::<jmap_client::Client>();
    assert_send_sync::<Slot<RwLock<Option<BookSync>>>>();
}
