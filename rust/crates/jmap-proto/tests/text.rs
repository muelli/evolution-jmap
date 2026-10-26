// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::text::{
    MAX_LINE_OCTETS, caret_decoded, caret_encoded, fold_overlong_lines, has_uri_scheme,
    is_valid_scheme, split_uri, uri_scheme,
};
use proptest::prelude::*;

#[test]
fn fold_overlong_lines_preserves_short_content() {
    let short = "SUMMARY:Team Standup Meeting\r\nDESCRIPTION:Quick check-in";
    assert_eq!(fold_overlong_lines(short.to_owned()), short);
}

#[test]
fn fold_overlong_lines_wraps_at_octets_budget() {
    let long_val = "a".repeat(100);
    let line = format!("DESCRIPTION:{long_val}");
    let folded = fold_overlong_lines(line.clone());
    for segment in folded.split("\r\n") {
        assert!(
            segment.len() <= MAX_LINE_OCTETS,
            "segment len {} exceeds MAX_LINE_OCTETS {}",
            segment.len(),
            MAX_LINE_OCTETS
        );
    }
    // Unfolding (replacing "\r\n " with empty) recovers original content.
    let unfolded = folded.replace("\r\n ", "");
    assert_eq!(unfolded, line);
}

#[test]
fn fold_overlong_lines_preserves_multibyte_utf8_boundaries() {
    // Euro symbol is 3 octets in UTF-8.
    let prefix = "X-TEST:".to_string() + &"a".repeat(67); // 7 + 67 = 74 octets
    let euro = "\u{20AC}"; // 3 octets, crosses 75
    let input = format!("{prefix}{euro}remainder");
    let folded = fold_overlong_lines(input.clone());
    // All lines must be valid UTF-8.
    for segment in folded.split("\r\n") {
        assert!(segment.len() <= MAX_LINE_OCTETS);
    }
    let unfolded = folded.replace("\r\n ", "");
    assert_eq!(unfolded, input);
}

#[test]
fn fold_overlong_lines_does_not_break_escaped_pairs() {
    // 73 'a's + '\' + ',' (escape pair) -> cut would land on '\' if naive.
    let prefix = "a".repeat(73);
    let input = format!("{prefix}\\,rest of line");
    let folded = fold_overlong_lines(input.clone());
    let mut lines = folded.split("\r\n");
    let first = lines.next().expect("first line");
    assert!(
        !first.ends_with('\\'),
        "cut must not leave a dangling backslash"
    );
    let unfolded = folded.replace("\r\n ", "");
    assert_eq!(unfolded, input);
}

#[test]
fn caret_encoding_and_decoding_round_trip() {
    let raw = "Hello ^ world, \"quotes\" and\nnewlines\r\ncombined";
    let encoded = caret_encoded(raw);
    assert_eq!(
        encoded,
        "Hello ^^ world, ^'quotes^' and^nnewlines^ncombined"
    );
    let decoded = caret_decoded(&encoded);
    assert_eq!(decoded, "Hello ^ world, \"quotes\" and\nnewlines\ncombined");
}

#[test]
fn caret_decoding_handles_lone_and_escaped_carets() {
    assert_eq!(caret_decoded("plain text"), "plain text");
    assert_eq!(caret_decoded("lone^caret"), "lone^caret");
    assert_eq!(caret_decoded("caret^^double"), "caret^double");
    assert_eq!(caret_decoded("quote^'val"), "quote\"val");
    assert_eq!(caret_decoded("new^nline"), "new\nline");
    assert_eq!(caret_decoded("new^Nline"), "new\nline");
    assert_eq!(caret_decoded("unknown^xstay"), "unknown^xstay");
}

#[test]
fn uri_scheme_validity() {
    assert!(is_valid_scheme("http"));
    assert!(is_valid_scheme("https"));
    assert!(is_valid_scheme("mailto"));
    assert!(is_valid_scheme("urn"));
    assert!(is_valid_scheme("my-custom+scheme.1"));
    assert!(is_valid_scheme("A"));

    assert!(!is_valid_scheme(""));
    assert!(!is_valid_scheme("1http"));
    assert!(!is_valid_scheme("+http"));
    assert!(!is_valid_scheme("-http"));
    assert!(!is_valid_scheme(".http"));
    assert!(!is_valid_scheme("http:"));
    assert!(!is_valid_scheme("http bad"));
}

#[test]
fn uri_scheme_extraction_and_splitting() {
    assert_eq!(uri_scheme("mailto:user@example.com"), Some("mailto"));
    assert_eq!(uri_scheme("HTTPS://example.com/cal"), Some("HTTPS"));
    assert_eq!(uri_scheme("urn:uuid:1234"), Some("urn"));
    assert_eq!(uri_scheme("bare-string"), None);
    assert_eq!(uri_scheme(":missing-scheme"), None);
    assert_eq!(uri_scheme("123bad:scheme"), None);

    assert_eq!(
        split_uri("mailto:user@example.com"),
        Some(("mailto", "user@example.com"))
    );
    assert_eq!(
        split_uri("https://example.com/events?id=1"),
        Some(("https", "//example.com/events?id=1"))
    );
    assert_eq!(split_uri("no-colon"), None);
    assert_eq!(split_uri(":no-scheme"), None);

    assert!(has_uri_scheme("mailto:user@example.com"));
    assert!(has_uri_scheme("http://example.com"));
    assert!(!has_uri_scheme("not-a-uri"));
    assert!(!has_uri_scheme(":empty"));
}

proptest! {
    #[test]
    fn prop_caret_round_trip(s in "\\PC*") {
        // Exclude carriage returns for round trip because encoding normalizes \r and \r\n to \n.
        let s_lf = s.replace('\r', "");
        let encoded = caret_encoded(&s_lf);
        let decoded = caret_decoded(&encoded);
        prop_assert_eq!(decoded, s_lf);
    }

    #[test]
    fn prop_fold_overlong_lines_bounds(lines in prop::collection::vec("[A-Za-z0-9][^\r\n]{0,200}", 1..10)) {
        let text = lines.join("\r\n");
        let folded = fold_overlong_lines(text.clone());
        for segment in folded.split("\r\n") {
            prop_assert!(segment.len() <= MAX_LINE_OCTETS, "segment len: {}", segment.len());
        }
        let unfolded = folded.replace("\r\n ", "");
        prop_assert_eq!(unfolded, text);
    }
}
