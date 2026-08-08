// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The vCard 3.0 lexer/emitter: folding, escaping, parameters.

use jmap_vcard::syntax::{self, Property};

fn named<'a>(properties: &'a [Property], name: &str) -> &'a Property {
    properties
        .iter()
        .find(|property| property.name == name)
        .unwrap_or_else(|| panic!("no {name} property"))
}

#[test]
fn parses_a_minimal_card() {
    let properties =
        syntax::parse("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vera\r\nEND:VCARD\r\n").expect("parse");

    // BEGIN/END are structure, not data, and are not handed out.
    let names: Vec<&str> = properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["VERSION", "FN"]);
    assert_eq!(named(&properties, "FN").text(), "Vera");
}

#[test]
fn rejects_a_card_without_begin_and_end() {
    assert!(syntax::parse("FN:Vera\r\n").is_err());
    assert!(syntax::parse("BEGIN:VCARD\r\nFN:Vera\r\n").is_err());
}

#[test]
fn unfolds_continuation_lines() {
    // RFC 2426 §2.6: CRLF followed by a single space or tab is a fold.
    // Bare LF appears in the wild (and in EDS's own caches), so accept it.
    let properties =
        syntax::parse("BEGIN:VCARD\r\nNOTE:one\r\n two\n\tthree\r\nEND:VCARD\r\n").expect("parse");
    assert_eq!(named(&properties, "NOTE").text(), "onetwothree");
}

#[test]
fn parses_parameters_including_quoted_values() {
    let properties = syntax::parse(concat!(
        "BEGIN:VCARD\r\n",
        "EMAIL;TYPE=WORK,PREF;X-JMAP-KEY=\"we;ird\":vera@example.com\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    let email = named(&properties, "EMAIL");
    assert_eq!(email.text(), "vera@example.com");
    assert_eq!(email.param("X-JMAP-KEY"), Some("we;ird"));
    assert!(email.has_type("WORK"));
    assert!(email.has_type("PREF"));
    assert!(!email.has_type("HOME"));
}

#[test]
fn parameter_and_property_names_are_case_insensitive() {
    let properties =
        syntax::parse("BEGIN:vcard\r\nemail;type=work:vera@example.com\r\nEND:vcard\r\n")
            .expect("parse");
    let email = named(&properties, "EMAIL");
    assert!(email.has_type("WORK"));
}

#[test]
fn strips_the_group_prefix() {
    let properties =
        syntax::parse("BEGIN:VCARD\r\nitem1.TEL:+49 30 123456\r\nEND:VCARD\r\n").expect("parse");
    let tel = named(&properties, "TEL");
    assert_eq!(tel.group.as_deref(), Some("item1"));
    assert_eq!(tel.text(), "+49 30 123456");
}

#[test]
fn unescapes_text_values_but_splits_components_first() {
    let properties = syntax::parse(concat!(
        "BEGIN:VCARD\r\n",
        "N:Olden\\;burg;Vera;;;\r\n",
        "NOTE:a\\,b\\nc\\\\d\r\n",
        "END:VCARD\r\n"
    ))
    .expect("parse");

    // The escaped semicolon is part of the family name, not a separator.
    assert_eq!(
        named(&properties, "N").components(),
        ["Olden;burg", "Vera", "", "", ""]
    );
    assert_eq!(named(&properties, "NOTE").text(), "a,b\nc\\d");
}

#[test]
fn writes_crlf_terminated_lines_wrapped_in_begin_and_end() {
    let text = syntax::write(&[Property::new("FN", "Vera")]);
    assert_eq!(text, "BEGIN:VCARD\r\nFN:Vera\r\nEND:VCARD\r\n");
}

#[test]
fn writes_escaped_values_and_quoted_parameters() {
    let text = syntax::write(&[
        Property::new("NOTE", "a,b\nc\\d;e"),
        Property::new("EMAIL", "vera@example.com")
            .with_param("X-JMAP-KEY", "we;ird")
            .with_param("TYPE", "WORK"),
    ]);

    assert!(text.contains("\r\nNOTE:a\\,b\\nc\\\\d\\;e\r\n"), "{text}");
    assert!(
        text.contains("\r\nEMAIL;X-JMAP-KEY=\"we;ird\";TYPE=WORK:vera@example.com\r\n"),
        "{text}"
    );
}

#[test]
fn writes_structured_values_without_escaping_the_separators() {
    let text = syntax::write(&[Property::structured(
        "N",
        ["Olden;burg", "Vera", "", "", ""],
    )]);
    assert!(text.contains("\r\nN:Olden\\;burg;Vera;;;\r\n"), "{text}");
}

#[test]
fn folds_long_lines_without_splitting_characters() {
    // Two widths, because they fail differently: a one-octet value catches
    // an off-by-one in the limit (the continuation's leading space counts
    // against it), a multi-octet one catches a fold placed mid-character,
    // which would make the whole vCard undecodable.
    for value in ["x".repeat(400), "ä".repeat(200)] {
        let text = syntax::write(&[Property::new("NOTE", &value)]);

        assert!(text.contains("\r\n "), "not folded at all:\n{text}");
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "line of {} octets: {line}", line.len());
        }
        let properties = syntax::parse(&text).expect("parse");
        assert_eq!(named(&properties, "NOTE").text(), value);
    }
}
