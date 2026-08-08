// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a hostile JMAP server can make of the vCard the backend stores.
//!
//! Every string in a `ContactCard` came off the network, including the *keys*
//! of the `emails` and `phones` maps, which are JSON object names the server
//! chooses freely and which this mapping round-trips through the `X-JMAP-KEY`
//! parameter. The rendered vCard is handed straight to
//! `e_contact_new_from_vcard`, so a string that can end a content line early
//! is a string that can add a property to the user's address book.
//!
//! See `docs/AUDIT-FFI.md`, finding F1.

use std::collections::BTreeMap;

use jmap_proto::contacts::{ContactCard, ContactEmail, ContactPhone};
use jmap_vcard::{card_to_vcard, vcard_to_card};

/// The parameter values that reach the wire without escaping, and therefore
/// the ones a line break has to be stripped from.
fn card_with_email_key(key: &str) -> ContactCard {
    let mut emails = BTreeMap::new();
    emails.insert(
        key.to_owned(),
        ContactEmail {
            address: "vera@example.com".to_owned(),
            ..ContactEmail::default()
        },
    );
    ContactCard {
        id: Some("C1".into()),
        emails: Some(emails),
        ..ContactCard::default()
    }
}

/// The exploit, spelled out: a map key carrying CRLF used to close the `EMAIL`
/// line and open an `FN` line of the server's choosing, which
/// `e_contact_new_from_vcard` then reads as the contact's display name.
#[test]
fn a_crlf_in_a_map_key_cannot_add_a_property_to_the_card() {
    let vcard = card_to_vcard(&card_with_email_key("e1\r\nFN:Mallory\r\nX-TAIL"));

    assert!(
        !vcard.contains("\r\nFN:"),
        "a server-chosen map key injected an FN line:\n{vcard}"
    );
    // Every line is still a line this crate wrote: BEGIN, VERSION, UID, EMAIL,
    // END. Nothing new appeared.
    let names: Vec<&str> = vcard
        .split("\r\n")
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .map(|line| line.split([':', ';']).next().unwrap_or(line))
        .collect();
    assert_eq!(names, ["BEGIN", "VERSION", "UID", "EMAIL", "END"]);
}

/// A lone LF is the same attack without the CR, and a lone CR the same again:
/// `EVCard`'s unfolder splits on either.
#[test]
fn a_bare_lf_or_cr_in_a_map_key_is_stripped_too() {
    for key in ["e1\nFN:Mallory", "e1\rFN:Mallory", "e1\n\rFN:Mallory"] {
        let vcard = card_to_vcard(&card_with_email_key(key));
        assert_eq!(
            vcard.matches("\r\n").count(),
            5,
            "{key:?} produced extra content lines:\n{vcard}"
        );
        // The text survives inside the quoted parameter — it is data, and
        // dropping more of it than the break would be its own kind of lie —
        // but it no longer starts a line.
        assert!(
            !vcard.contains("\r\nFN:Mallory"),
            "{key:?} injected an FN line:\n{vcard}"
        );
    }
}

/// The one place the line-break strip is *not* enough on its own: a value that
/// legitimately needs a `:` or `;` is quoted, and there is no escape inside the
/// quotes, so an embedded quote has to go as well — otherwise a map key of
/// `x";FN="Mallory` would close the quoted run and open a parameter of its own.
#[test]
fn a_quote_in_a_map_key_cannot_open_a_parameter_of_its_own() {
    let vcard = card_to_vcard(&card_with_email_key("x\";FN=\"Mallory"));
    let email = jmap_vcard::syntax::parse(&vcard)
        .expect("the sanitised card still parses")
        .into_iter()
        .find(|property| property.name == "EMAIL")
        .expect("the EMAIL property");

    assert!(
        email.param_values("FN").is_empty(),
        "the map key opened a parameter of its own:\n{vcard}"
    );
    assert_eq!(email.param("X-JMAP-KEY"), Some("x;FN=Mallory"));
}

/// The phone half of the same map-key path.
#[test]
fn a_crlf_in_a_phone_map_key_cannot_add_a_property_either() {
    let mut phones = BTreeMap::new();
    phones.insert(
        "p1\r\nTEL:+1000000000".to_owned(),
        ContactPhone {
            number: "+49301234567".to_owned(),
            ..ContactPhone::default()
        },
    );
    let card = ContactCard {
        id: Some("C1".into()),
        phones: Some(phones),
        ..ContactCard::default()
    };

    let vcard = card_to_vcard(&card);
    assert_eq!(
        vcard.matches("\r\nTEL").count(),
        1,
        "a second TEL line appeared:\n{vcard}"
    );
}

/// The values that *are* escaped stay escaped, which is what keeps this fix
/// from being the only thing standing between the server and the card.
#[test]
fn a_crlf_in_a_value_is_still_escaped_rather_than_dropped() {
    let mut card = ContactCard {
        id: Some("C1".into()),
        ..ContactCard::default()
    };
    card.name = Some(jmap_proto::contacts::Name {
        full: Some("Vera\r\nFN:Mallory".to_owned()),
        ..jmap_proto::contacts::Name::default()
    });

    let vcard = card_to_vcard(&card);
    assert_eq!(
        vcard.matches("\r\nFN").count(),
        1,
        "the escaped newline still ended the line:\n{vcard}"
    );
    // And the text survives, as `\n`, rather than being silently truncated.
    let back = vcard_to_card(&vcard).expect("the sanitised card still parses");
    assert_eq!(
        back.name.and_then(|name| name.full).as_deref(),
        Some("Vera\nFN:Mallory")
    );
}
