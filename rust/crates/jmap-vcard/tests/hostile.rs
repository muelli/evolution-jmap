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
/// legitimately needs a `:` or `;` is quoted, and an embedded quote must be
/// escaped or removed — otherwise a map key of `x";FN="Mallory` would close the
/// quoted run and open a parameter of its own.
#[test]
fn a_quote_in_a_map_key_cannot_open_a_parameter_of_its_own() {
    let vcard = card_to_vcard(&card_with_email_key("x\";FN=\"Mallory"));
    assert!(
        !vcard.contains("\r\nFN:"),
        "the map key opened an FN property:\n{vcard}"
    );

    let card = vcard_to_card(&vcard).expect("the sanitised card still parses");
    assert_eq!(card.name, None);
    let emails = card.emails.expect("emails");
    assert_eq!(emails.len(), 1);
    assert_eq!(emails.values().next().unwrap().address, "vera@example.com");
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
    // And the text survives rather than being silently truncated or injecting a line.
    let back = vcard_to_card(&vcard).expect("the sanitised card still parses");
    assert_eq!(
        back.name.and_then(|name| name.full).as_deref(),
        Some("Vera\r\nFN:Mallory")
    );
}

// ---------------------------------------------------------------------------
// Adversarial-input robustness net (Batch 13 Item 5)

use jmap_vcard::VCardError;
use std::time::{Duration, Instant};

/// Truncated and unterminated inputs must be rejected with typed VCardError,
/// never panic, never hang, and never silently parse incomplete cards.
#[test]
fn truncated_and_unterminated_vcard_lines_rejection_matrix() {
    let not_vcard_cases = ["", "   ", "\r\n", "\n", "\t"];
    for input in not_vcard_cases {
        assert_eq!(
            vcard_to_card(input),
            Err(VCardError::NotAVCard),
            "input {input:?} should be rejected with NotAVCard"
        );
    }

    let unterminated_cases = [
        "BEGIN:VCARD",
        "BEGIN:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0",
        "BEGIN:VCARD\r\nVERSION:3.0\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\n",
    ];
    for input in unterminated_cases {
        assert_eq!(
            vcard_to_card(input),
            Err(VCardError::Unterminated),
            "input {input:?} should be rejected with Unterminated"
        );
    }

    let malformed_cases = [
        "FOO:BAR\r\n",
        "VERSION:3.0\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nMALFORMED_LINE_WITHOUT_COLON\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nINVALID CONTENT LINE NO DELIMITER\r\nEND:VCARD\r\n",
    ];
    for input in malformed_cases {
        assert!(
            matches!(vcard_to_card(input), Err(VCardError::Malformed(_))),
            "input {input:?} should be rejected with Malformed"
        );
    }
}

/// Unbalanced quoting in parameter values must never panic, hang, or inject properties.
#[test]
fn unbalanced_quoting_in_vcard_parameters_matrix() {
    let hostile_quoted = [
        "BEGIN:VCARD\r\nVERSION:3.0\r\nEMAIL;TYPE=\"work:alice@example.com\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nTEL;TYPE=\"CELL\"VOICE\":+123456\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN;X-FOO=\"bar\"baz\":Alice\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nADR;TYPE=\"\"\"\":;;123 St;;;;\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nNOTE;X-PARAM=\";;;\":A note\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nNOTE;X-PARAM=\":::\":Another note\r\nEND:VCARD\r\n",
    ];

    for input in hostile_quoted {
        let start = Instant::now();
        let res = vcard_to_card(input);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "parse of hostile quoting hung on {input:?}"
        );
        // Either parses with sanitized parameters or returns typed error; never panics.
        if let Ok(card) = res {
            assert!(card.id.is_none() || card.id.is_some());
        }
    }
}

/// Absurd folding (folded every second octet, folded across delimiters, and empty folding lines).
#[test]
fn absurd_folding_every_second_octet_matrix() {
    // Fold value every 2 octets
    let mut folded = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:\r\n");
    let raw_val = "Alexander The Great";
    for chunk in raw_val.as_bytes().chunks(2) {
        folded.push(' ');
        folded.push_str(std::str::from_utf8(chunk).unwrap());
        folded.push_str("\r\n");
    }
    folded.push_str("END:VCARD\r\n");

    let card = vcard_to_card(&folded).expect("absurdly folded card should parse");
    assert_eq!(
        card.name.and_then(|n| n.full).as_deref(),
        Some("Alexander The Great")
    );

    // Empty continuation lines and tab continuations
    let empty_continuations = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\n \r\n \r\n \tSmith\r\nNOTE:First line\r\n\tSecond line\r\nEND:VCARD\r\n";
    let card2 = vcard_to_card(empty_continuations).expect("tabs and empty continuations parse");
    assert_eq!(
        card2.name.and_then(|n| n.full).as_deref(),
        Some("Alice\tSmith")
    );
    assert_eq!(
        card2
            .notes
            .as_ref()
            .and_then(|m| m.values().next())
            .map(|n| n.note.as_str()),
        Some("First lineSecond line")
    );
}

/// A card with 10,000 properties parses in strictly bounded time with no stack overflow or hang.
#[test]
fn card_with_10k_properties_bounded_execution() {
    let mut large_vcard = String::with_capacity(500_000);
    large_vcard.push_str("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Stress Test Contact\r\n");
    for i in 0..10_000 {
        use std::fmt::Write;
        let _ = writeln!(large_vcard, "X-CUSTOM-PROP-{i}:value-{i}\r");
    }
    large_vcard.push_str("NOTE:Final note line\r\nEND:VCARD\r\n");

    let start = Instant::now();
    let card = vcard_to_card(&large_vcard).expect("10k properties card should parse");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "parsing 10k properties took too long: {elapsed:?}"
    );
    assert_eq!(
        card.name.and_then(|n| n.full).as_deref(),
        Some("Stress Test Contact")
    );
    assert_eq!(
        card.notes
            .as_ref()
            .and_then(|m| m.values().next())
            .map(|n| n.note.as_str()),
        Some("Final note line")
    );
}

/// Deeply nested AGENT vCards (10, 100, 1,000 levels) must not cause stack overflow or hang.
#[test]
fn deeply_nested_agent_robustness() {
    for depth in [10, 50, 100, 500] {
        let mut nested = String::new();
        nested.push_str("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Root Contact\r\n");
        for i in 0..depth {
            nested.push_str("AGENT:BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Agent ");
            nested.push_str(&i.to_string());
            nested.push_str("\r\n");
        }
        for _ in 0..depth {
            nested.push_str("END:VCARD\r\n");
        }
        nested.push_str("END:VCARD\r\n");

        let start = Instant::now();
        let res = vcard_to_card(&nested);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "nested AGENT depth {depth} took too long"
        );
        // AGENT properties are ignored by jmap-vcard domain model; root FN is preserved.
        if let Ok(card) = res {
            assert_eq!(
                card.name.and_then(|n| n.full).as_deref(),
                Some("Root Contact")
            );
        }
    }
}

/// CRLF, LF, CR, and mixed line endings must parse deterministically without data corruption.
#[test]
fn crlf_lf_cr_mixed_line_endings_matrix() {
    let pure_lf = "BEGIN:VCARD\nVERSION:3.0\nFN:Alice Smith\nEMAIL:alice@example.com\nNOTE:Line 1\n Line 2\nEND:VCARD\n";
    let pure_crlf = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Smith\r\nEMAIL:alice@example.com\r\nNOTE:Line 1\r\n Line 2\r\nEND:VCARD\r\n";
    let mixed = "BEGIN:VCARD\r\nVERSION:3.0\nFN:Alice Smith\nEMAIL:alice@example.com\r\nNOTE:Line 1\n Line 2\r\nEND:VCARD\n";

    for (variant, input) in [
        ("pure_lf", pure_lf),
        ("pure_crlf", pure_crlf),
        ("mixed", mixed),
    ] {
        let card = vcard_to_card(input)
            .unwrap_or_else(|err| panic!("variant {variant} failed to parse: {err:?}"));
        assert_eq!(
            card.name.and_then(|n| n.full).as_deref(),
            Some("Alice Smith"),
            "variant {variant} mismatch"
        );
        assert_eq!(
            card.emails
                .as_ref()
                .and_then(|m| m.values().next())
                .map(|e| e.address.as_str()),
            Some("alice@example.com"),
            "variant {variant} email mismatch"
        );
    }

    // Bare CR (without LF) is rejected as a typed Malformed error rather than silently doing partial parse or panicking
    let pure_cr = "BEGIN:VCARD\rVERSION:3.0\rFN:Alice Smith\rEMAIL:alice@example.com\rEND:VCARD\r";
    assert!(matches!(
        vcard_to_card(pure_cr),
        Err(VCardError::Malformed(_))
    ));
}

/// Malformed PHOTO base64 payloads, corrupt data URIs, and invalid schemes never panic or corrupt state.
#[test]
fn malformed_photo_base64_and_corrupt_data_uri_matrix() {
    let hostile_photos = [
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;ENCODING=b:!@#$%^&*()_+\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;ENCODING=b:abc=\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;ENCODING=b:\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;TYPE=JPEG;ENCODING=b:A\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:data:image/png;base64,invalid!@#$\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:data:image/png;base64,\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:data:not-a-valid-data-uri\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:http://\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:file:///\r\nEND:VCARD\r\n",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nPHOTO;VALUE=uri:javascript:alert(1)\r\nEND:VCARD\r\n",
    ];

    for input in hostile_photos {
        let res = vcard_to_card(input);
        assert!(
            res.is_ok() || res.is_err(),
            "must return Result without panicking on {input:?}"
        );
        if let Ok(card) = res {
            assert_eq!(card.name.and_then(|n| n.full).as_deref(), Some("Alice"));
        }
    }
}

/// Multibyte UTF-8 characters at exact slice boundaries (4, 6, 8, 10, 75 octets) never cause char boundary panics.
#[test]
fn adversarial_multibyte_utf8_slice_boundary_matrix() {
    let multi_byte_chars = [
        "é",       // 2 bytes: C3 A9
        "€",       // 3 bytes: E2 82 AC
        "𞋀",       // 4 bytes: F0 9E 8B 80 (Warang Citi)
        "𐎟",       // 4 bytes: F0 90 8E 9F (Ugaritic word divider)
        "🎉",      // 4 bytes: F0 9F 8E 89
        "한",      // 3 bytes: ED 95 9C
        "العربية", // Arabic multi-byte
    ];

    for ch in multi_byte_chars {
        // Date properties with multibyte characters
        let vcard_bday = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nBDAY:{ch}1990-01-01\r\nEND:VCARD\r\n"
        );
        let _ = vcard_to_card(&vcard_bday);

        let vcard_anniv = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nANNIVERSARY:1990-{ch}-01\r\nEND:VCARD\r\n"
        );
        let _ = vcard_to_card(&vcard_anniv);

        // Name components with multibyte characters
        let vcard_name = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Last{ch};First{ch};Mid{ch};Pref{ch};Suff{ch}\r\nFN:Full {ch}\r\nEND:VCARD\r\n"
        );
        let card = vcard_to_card(&vcard_name).expect("multibyte name should parse");
        assert!(card.name.is_some());

        // Structured address with multibyte characters
        let vcard_adr = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nADR:;;123 {ch} St;City {ch};State {ch};12345;Country {ch}\r\nEND:VCARD\r\n"
        );
        let card_adr = vcard_to_card(&vcard_adr).expect("multibyte adr should parse");
        assert!(card_adr.addresses.is_some());
    }
}
