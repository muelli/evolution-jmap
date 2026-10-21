// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test hostile server input for `ContactCard` data.

use std::collections::BTreeMap;

use jmap_proto::contacts::{ContactCard, ContactEmail, ContactPhone, Media};
use jmap_vcard::{card_to_vcard, vcard_to_card};

/// Strip line breaks before writing map keys as parameter text.
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

/// Build a card with one `photo` media entry under key `m1`.
fn card_with_photo_uri(uri: &str) -> ContactCard {
    let mut media = BTreeMap::new();
    media.insert(
        "m1".to_owned(),
        Media {
            kind: Some("photo".to_owned()),
            uri: uri.to_owned(),
            ..Media::default()
        },
    );
    ContactCard {
        id: Some("C1".into()),
        media: Some(media),
        ..ContactCard::default()
    }
}

/// Reject unsupported URI schemes and emit no `PHOTO` line.
#[test]
fn a_file_uri_photo_from_the_server_is_not_written_as_a_photo_line() {
    for uri in [
        "file:///home/user/.ssh/id_ed25519",
        "ftp://files.example.net/avatar.jpg",
        "smb://server/share/avatar.png",
        "://empty-scheme.example/avatar.png",
        "avatar.png",
    ] {
        let vcard = card_to_vcard(&card_with_photo_uri(uri));
        assert!(
            !vcard.contains("\r\nPHOTO"),
            "unsupported uri leaked into PHOTO line: {uri} in {vcard}"
        );
        assert!(
            !vcard.contains(uri),
            "unsupported uri leaked into rendered card: {uri} in {vcard}"
        );

        let parsed = vcard_to_card(&vcard).expect("parse rendered card");
        assert!(
            parsed.media.is_none(),
            "unsupported uri survived parse: {vcard}"
        );
    }
}

/// Drop inbound local or unsupported URI references.
#[test]
fn inbound_photo_uri_references_with_unsupported_schemes_are_dropped() {
    for line in [
        "PHOTO;VALUE=uri:file:///home/user/.face",
        "PHOTO;VALUE=uri:ftp://files.example.net/avatar.jpg",
        "PHOTO;VALUE=uri:smb://server/share/avatar.png",
        "PHOTO;VALUE=uri:://empty-scheme.example/avatar.png",
        "PHOTO:file:///home/user/.face",
        "PHOTO:://empty-scheme.example/avatar.png",
    ] {
        let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n");
        let parsed = vcard_to_card(&vcard).expect("parse card with unsupported photo uri");
        assert!(
            parsed.media.is_none(),
            "unsupported URI reference must not become media: {line}"
        );
    }
}

/// Accept inbound `VALUE=uri` values that carry `data:` payloads.
#[test]
fn inbound_photo_value_uri_data_payload_stays_mapped() {
    let line = "PHOTO;VALUE=uri:data:image/png;base64,aGVsbG8=";
    let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\n{line}\r\nEND:VCARD\r\n");
    let parsed = vcard_to_card(&vcard).expect("parse card with VALUE=uri data photo");
    let media = parsed.media.expect("media map");
    let photo = media.values().next().expect("photo entry");
    assert_eq!(photo.kind.as_deref(), Some("photo"));
    assert_eq!(photo.uri, "data:image/png;base64,aGVsbG8=");
    assert_eq!(photo.media_type.as_deref(), Some("image/png"));
}

/// Keep save output from emitting local bytes from `file://` URIs.
#[test]
fn inbound_file_uri_photo_does_not_turn_into_outbound_inline_data() {
    let inbound = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nPHOTO;VALUE=uri:file:///dev/zero\r\nEMAIL:vera@example.com\r\nEND:VCARD\r\n";

    let parsed = vcard_to_card(inbound).expect("parse inbound file URI photo");
    assert!(
        parsed.media.is_none(),
        "local file URI must not become media on read"
    );

    let saved = card_to_vcard(&parsed);
    assert!(
        !saved.contains("\r\nPHOTO"),
        "save path must not emit PHOTO from a dropped file URI: {saved}"
    );
    assert!(
        !saved.contains("file:///dev/zero"),
        "save path must not preserve local file URI text: {saved}"
    );
    assert!(
        !saved.contains("data:"),
        "save path must not emit inline data from a local URI: {saved}"
    );
    assert!(
        saved.contains("EMAIL"),
        "control field must stay present after save-path roundtrip: {saved}"
    );
}

/// Reject CRLF in map keys so `EMAIL` stays one line.
#[test]
fn a_crlf_in_a_map_key_cannot_add_a_property_to_the_card() {
    let vcard = card_to_vcard(&card_with_email_key("e1\r\nFN:Mallory\r\nX-TAIL"));

    assert!(
        !vcard.contains("\r\nFN:"),
        "a server-chosen map key injected an FN line:\n{vcard}"
    );
    let names: Vec<&str> = vcard
        .split("\r\n")
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .map(|line| line.split([':', ';']).next().unwrap_or(line))
        .collect();
    assert_eq!(names, ["BEGIN", "VERSION", "UID", "EMAIL", "END"]);
}

/// Strip bare LF or CR from map keys before write.
#[test]
fn a_bare_lf_or_cr_in_a_map_key_is_stripped_too() {
    for key in ["e1\nFN:Mallory", "e1\rFN:Mallory", "e1\n\rFN:Mallory"] {
        let vcard = card_to_vcard(&card_with_email_key(key));
        assert_eq!(
            vcard.matches("\r\n").count(),
            5,
            "{key:?} produced extra content lines:\n{vcard}"
        );
        assert!(
            !vcard.contains("\r\nFN:Mallory"),
            "{key:?} injected an FN line:\n{vcard}"
        );
    }
}

/// Keep embedded quotes from creating a second parameter.
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

/// Parameter text with `"` or `^` must use RFC 6868 caret escapes.
#[test]
fn x_jmap_key_parameter_text_uses_rfc_6868_caret_escapes() {
    let key = "x\"^y";
    let vcard = card_to_vcard(&card_with_email_key(key));
    assert!(
        vcard.contains("X-JMAP-KEY=x^'^^y"),
        "parameter text must use RFC 6868 caret escapes: {vcard}"
    );
    assert!(
        !vcard.contains("x\\\"^y"),
        "parameter text must not use backslash-quote escaping: {vcard}"
    );

    let parsed = vcard_to_card(&vcard).expect("parse emitted card");
    assert!(
        parsed.emails.expect("emails").contains_key(key),
        "decoded map key must match source text"
    );
}

/// Decode inbound RFC 6868 caret escapes on read.
#[test]
fn x_jmap_key_caret_escapes_decode_on_read() {
    let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEMAIL;X-JMAP-KEY=\"x^'^^y\":vera@example.com\r\nEND:VCARD\r\n";
    let parsed = vcard_to_card(vcard).expect("parse card with escaped parameter text");
    assert!(
        parsed.emails.expect("emails").contains_key("x\"^y"),
        "RFC 6868 escapes must decode into the key text"
    );
}

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

/// Keep escaped value newlines as data.
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
    let back = vcard_to_card(&vcard).expect("the sanitised card still parses");
    assert_eq!(
        back.name.and_then(|name| name.full).as_deref(),
        Some("Vera\r\nFN:Mallory")
    );
}

use jmap_vcard::VCardError;
use std::time::{Duration, Instant};

/// Return typed errors for truncated input. Do not panic or hang.
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

/// Reject unbalanced parameter quoting. Do not panic, hang, or inject properties.
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
        if let Ok(card) = res {
            assert!(card.id.is_none() || card.id.is_some());
        }
    }
}

/// Parse extreme folding patterns without panic or root-field drift.
#[test]
fn absurd_folding_every_second_octet_matrix() {
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

/// Parse a 10k-property card within the runtime bound.
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

/// Deeply nested AGENT vCards must stay within the runtime bound.
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
        if let Ok(card) = res {
            assert_eq!(
                card.name.and_then(|n| n.full).as_deref(),
                Some("Root Contact")
            );
        }
    }
}

/// Accept LF and CRLF endings. Reject bare CR endings.
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

    let pure_cr = "BEGIN:VCARD\rVERSION:3.0\rFN:Alice Smith\rEMAIL:alice@example.com\rEND:VCARD\r";
    assert!(matches!(
        vcard_to_card(pure_cr),
        Err(VCardError::Malformed(_))
    ));
}

/// Malformed PHOTO payloads must not panic or corrupt parsed fields.
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

/// Parse multibyte UTF-8 fields without panic at byte boundaries.
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
        let vcard_bday = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nBDAY:{ch}1990-01-01\r\nEND:VCARD\r\n"
        );
        let _ = vcard_to_card(&vcard_bday);

        let vcard_anniv = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nANNIVERSARY:1990-{ch}-01\r\nEND:VCARD\r\n"
        );
        let _ = vcard_to_card(&vcard_anniv);

        let vcard_name = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Last{ch};First{ch};Mid{ch};Pref{ch};Suff{ch}\r\nFN:Full {ch}\r\nEND:VCARD\r\n"
        );
        let card = vcard_to_card(&vcard_name).expect("multibyte name should parse");
        assert!(card.name.is_some());

        let vcard_adr = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nADR:;;123 {ch} St;City {ch};State {ch};12345;Country {ch}\r\nEND:VCARD\r\n"
        );
        let card_adr = vcard_to_card(&vcard_adr).expect("multibyte adr should parse");
        assert!(card_adr.addresses.is_some());
    }
}
