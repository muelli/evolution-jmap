// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exotic input tests covering Unicode, bidi controls, zero-width characters,
//! NUL, CR, LF, NFC/NFD normalization, line folding at 75 octets, and very long lines.

use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::CalendarEvent;
use proptest::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;

fn base_event() -> CalendarEvent {
    let mut event = CalendarEvent::default();
    event.id = Some("test-exotic-1".into());
    event.title = Some("Base Event".to_owned());
    event.start = Some("2026-11-01T10:00:00".to_owned());
    event.duration = Some("PT1H".to_owned());
    event
}

#[test]
fn probe_bidi_controls_roundtrip() {
    let mut event = base_event();
    // RLO (U+202E), LRO (U+202D), PDI (U+2069), plus LRI (U+2066), RLI (U+2067), PDF (U+202C)
    let bidi_summary = "Project \u{202e}exe.test\u{2069} review \u{202d}LTR\u{202c}";
    let bidi_location = "Building \u{2067}حاسوب\u{2069} Room 404";
    let bidi_organizer_cn = "Alice \u{202e}ecilA\u{2069}";
    let bidi_attendee_cn = "Bob \u{202d}boB\u{2069}";

    event.title = Some(bidi_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": bidi_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": bidi_organizer_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": bidi_attendee_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);
    println!("ICS with bidi:\n{ics}");

    let parsed = ical_to_event(&ics).expect("parse ics with bidi");
    assert_eq!(parsed.title.as_deref(), Some(bidi_summary));

    let parsed_loc = parsed.locations.as_ref().unwrap();
    let loc_val = parsed_loc.values().next().unwrap();
    assert_eq!(
        loc_val.get("name").unwrap().as_str().unwrap(),
        bidi_location
    );

    let parsed_parts = parsed.participants.as_ref().unwrap();
    let alice = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:alice@example.com")
        .unwrap();
    assert_eq!(
        alice.get("name").unwrap().as_str().unwrap(),
        bidi_organizer_cn
    );

    let bob = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:bob@example.com")
        .unwrap();
    assert_eq!(bob.get("name").unwrap().as_str().unwrap(), bidi_attendee_cn);
}

#[test]
fn probe_zero_width_characters_roundtrip() {
    let mut event = base_event();
    // ZWSP (U+200B), ZWNJ (U+200C), ZWJ (U+200D), BOM / ZWNBSP (U+FEFF)
    let zw_summary = "Hidden\u{200b}Space\u{200c}NonJoiner\u{200d}Joiner\u{feff}BOM";
    let zw_location = "\u{feff}Room\u{200b}101";
    let zw_organizer = "Admin\u{200c}\u{200d}";
    let zw_attendee = "\u{200b}Guest\u{feff}";

    event.title = Some(zw_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": zw_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": zw_organizer,
            "calendarAddress": "mailto:admin@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": zw_attendee,
            "calendarAddress": "mailto:guest@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);
    let parsed = ical_to_event(&ics).expect("parse ics with zero-width");
    assert_eq!(parsed.title.as_deref(), Some(zw_summary));

    let parsed_loc = parsed.locations.as_ref().unwrap();
    let loc_val = parsed_loc.values().next().unwrap();
    assert_eq!(loc_val.get("name").unwrap().as_str().unwrap(), zw_location);

    let parsed_parts = parsed.participants.as_ref().unwrap();
    let admin = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:admin@example.com")
        .unwrap();
    assert_eq!(admin.get("name").unwrap().as_str().unwrap(), zw_organizer);

    let guest = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:guest@example.com")
        .unwrap();
    assert_eq!(guest.get("name").unwrap().as_str().unwrap(), zw_attendee);
}

#[test]
fn probe_crlf_injection_in_summary_and_location() {
    let mut event = base_event();
    let injected_summary = "Meeting\r\nLOCATION:Injected Location\r\nDESCRIPTION:Injected Desc";
    let injected_location = "Room 101\r\nSUMMARY:Injected Summary";

    event.title = Some(injected_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": injected_location,
        }),
    );
    event.locations = Some(locations);

    let ics = event_to_ical(&event);
    println!("ICS with CRLF in summary and location RAW:\n{ics:?}");

    // Count physical SUMMARY and LOCATION lines (excluding folded continuation lines starting with space)
    let physical_lines: Vec<&str> = ics
        .split("\r\n")
        .filter(|l| !l.starts_with(' ') && !l.is_empty())
        .collect();
    let summary_count = physical_lines
        .iter()
        .filter(|l| l.starts_with("SUMMARY"))
        .count();
    let location_count = physical_lines
        .iter()
        .filter(|l| l.starts_with("LOCATION"))
        .count();

    assert_eq!(
        summary_count, 1,
        "CRLF in title must not inject a second SUMMARY line"
    );
    assert_eq!(
        location_count, 1,
        "CRLF in location must not inject a second LOCATION line"
    );

    let parsed = ical_to_event(&ics).expect("parse roundtrip");
    // Verify roundtrip value
    println!("Parsed title: {:?}", parsed.title);
    let parsed_loc = parsed.locations.as_ref().unwrap();
    let loc_val = parsed_loc.values().next().unwrap();
    println!("Parsed location: {:?}", loc_val.get("name"));
}

#[test]
fn probe_crlf_injection_in_cn_parameters() {
    let mut event = base_event();
    let injected_org_cn = "Alice\r\nATTENDEE;CN=Mallory:mailto:mallory@evil.com\r\n";
    let injected_att_cn = "Bob\r\nROLE=CHAIR\r\n";

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": injected_org_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": injected_att_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);
    println!("ICS with CRLF in CN parameters:\n{ics}");

    let physical_lines: Vec<&str> = ics
        .split("\r\n")
        .filter(|l| !l.starts_with(' ') && !l.is_empty())
        .collect();
    let attendee_count = physical_lines
        .iter()
        .filter(|l| l.starts_with("ATTENDEE"))
        .count();
    let organizer_count = physical_lines
        .iter()
        .filter(|l| l.starts_with("ORGANIZER"))
        .count();

    assert_eq!(
        organizer_count, 1,
        "CRLF in organizer CN must not inject lines"
    );
    assert_eq!(
        attendee_count, 1,
        "CRLF in attendee CN must not inject lines"
    );

    let parsed = ical_to_event(&ics).expect("parse roundtrip");
    let parsed_parts = parsed.participants.as_ref().unwrap();
    let alice = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:alice@example.com")
        .unwrap();
    println!("Parsed alice name: {:?}", alice.get("name"));
    let bob = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:bob@example.com")
        .unwrap();
    println!("Parsed bob name: {:?}", bob.get("name"));
}

#[test]
fn probe_nul_in_fields() {
    let mut event = base_event();
    let nul_summary = "Meeting\0With\0NUL";
    let nul_location = "Room\0Hidden";
    let nul_org_cn = "Alice\0Admin";
    let nul_att_cn = "Bob\0Guest";

    event.title = Some(nul_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": nul_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": nul_org_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": nul_att_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);
    println!("ICS with NUL:\n{ics:?}");

    let parsed = ical_to_event(&ics).expect("parse roundtrip with NUL");
    println!("Parsed title with NUL: {:?}", parsed.title);
    let parsed_loc = parsed.locations.as_ref().unwrap();
    let loc_val = parsed_loc.values().next().unwrap();
    println!("Parsed loc with NUL: {:?}", loc_val.get("name"));
    let parsed_parts = parsed.participants.as_ref().unwrap();
    let alice = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:alice@example.com")
        .unwrap();
    println!("Parsed alice with NUL: {:?}", alice.get("name"));
    let bob = parsed_parts
        .values()
        .find(|p| p.get("calendarAddress").unwrap() == "mailto:bob@example.com")
        .unwrap();
    println!("Parsed bob with NUL: {:?}", bob.get("name"));
}

#[test]
fn probe_nfc_vs_nfd() {
    let mut event = base_event();
    // NFC: composed forms
    // "é" = U+00E9 (2 bytes: C3 A9)
    // "ö" = U+00F6 (2 bytes: C3 B6)
    // "ñ" = U+00F1 (2 bytes: C3 B1)
    let nfc_text = "Caf\u{00e9} in K\u{00f6}ln with Se\u{00f1}or";

    // NFD: decomposed forms
    // "é" = e + U+0301 (combining acute accent)
    // "ö" = o + U+0308 (combining diaeresis)
    // "ñ" = n + U+0303 (combining tilde)
    let nfd_text = "Cafe\u{0301} in Ko\u{0308}ln with Sen\u{0303}or";

    assert_ne!(nfc_text.as_bytes(), nfd_text.as_bytes());

    event.title = Some(nfc_text.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": nfd_text,
        }),
    );
    event.locations = Some(locations);

    let ics = event_to_ical(&event);
    let parsed = ical_to_event(&ics).expect("parse nfc and nfd");

    // Check if original normalization is preserved verbatim
    assert_eq!(
        parsed.title.as_deref(),
        Some(nfc_text),
        "NFC title preserved verbatim"
    );
    let loc_val = parsed.locations.unwrap().into_values().next().unwrap();
    assert_eq!(
        loc_val.get("name").unwrap().as_str().unwrap(),
        nfd_text,
        "NFD location preserved verbatim"
    );
}

#[test]
fn probe_emoji_and_multibyte_at_75_octet_fold_boundary() {
    // RFC 5545 §3.1 requires lines to be no longer than 75 octets excluding CRLF.
    // Test various lengths of prefix so that 2-byte, 3-byte, and 4-byte characters
    // cross the 75-octet boundary at every possible byte offset.

    let multibyte_chars = [
        ("2-byte", "\u{00e9}"),       // Latin small letter e with acute (2 bytes)
        ("3-byte", "\u{202e}"),       // RLO bidi override (3 bytes)
        ("4-byte-crab", "\u{1f980}"), // Crab emoji 🦀 (4 bytes)
        ("4-byte-family", "👨‍👩‍👧‍👦"),      // ZWJ sequence (25 bytes)
    ];

    for (name, mb) in multibyte_chars {
        // "SUMMARY:" is 8 octets.
        // Try prefixes of length 50 to 80 to hit before, on, and after the 75-octet fold point.
        for prefix_len in 50..=80 {
            let mut event = base_event();
            let prefix = "A".repeat(prefix_len);
            let suffix = "Z".repeat(50);
            let test_title = format!("{prefix}{mb}{suffix}");
            event.title = Some(test_title.clone());

            let ics = event_to_ical(&event);

            // 1. Verify that every physical line in the ICS output is at most 75 octets (excluding CRLF)
            for line in ics.split("\r\n") {
                assert!(
                    line.len() <= 75,
                    "Physical line exceeded 75 octets ({} octets): {:?} for case {} prefix_len {}",
                    line.len(),
                    line,
                    name,
                    prefix_len
                );
            }

            // 2. Verify that the ICS output is valid UTF-8 (String in Rust is always valid UTF-8,
            // but verify slicing didn't create invalid characters)
            assert!(std::str::from_utf8(ics.as_bytes()).is_ok());

            // 3. Verify that round-trip parsing reconstructs the exact original title without corruption
            let parsed = ical_to_event(&ics).unwrap_or_else(|e| {
                panic!(
                    "Failed to parse ICS for {} with prefix {}: {}\nICS:\n{}",
                    name, prefix_len, e, ics
                )
            });
            assert_eq!(
                parsed.title.as_deref(),
                Some(test_title.as_str()),
                "Round-trip title mismatch for {} with prefix_len {}",
                name,
                prefix_len
            );
        }
    }
}

#[test]
fn probe_very_long_lines_roundtrip() {
    let mut event = base_event();
    // 10,000 character summary with ASCII, Unicode, emoji, and bidi
    let long_summary: String = (0..1000)
        .map(|i| format!("{i}:Café 🦀 \u{202e}bidi\u{2069} test; "))
        .collect();
    // 5,000 character location
    let long_location: String = "L".repeat(5000);

    event.title = Some(long_summary.clone());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": long_location.clone(),
        }),
    );
    event.locations = Some(locations);

    let ics = event_to_ical(&event);

    // Verify all physical lines <= 75 octets
    for (idx, line) in ics.split("\r\n").enumerate() {
        assert!(
            line.len() <= 75,
            "Line {idx} has length {} > 75: {:?}",
            line.len(),
            line
        );
    }

    let parsed = ical_to_event(&ics).expect("parse very long lines");
    assert_eq!(
        parsed.title.as_deref(),
        Some(long_summary.as_str()),
        "Long title roundtrip"
    );
    let loc_val = parsed.locations.unwrap().into_values().next().unwrap();
    assert_eq!(
        loc_val.get("name").unwrap().as_str().unwrap(),
        long_location.as_str(),
        "Long location roundtrip"
    );
}

#[test]
fn probe_ical_to_jscalendar_to_ical_roundtrip() {
    let original_ical = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example Corp.//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:exotic-roundtrip-1\r\n",
        "SUMMARY:Meeting \u{202e}bidi\u{2069} \u{200b}zwsp\u{feff} with Caf\u{00e9}\\nSecond line\r\n",
        "LOCATION;X-JMAP-KEY=loc1:Building \u{2067}حاسوب\u{2069}\\nRoom 101\\; Station A\r\n",
        "ORGANIZER;CN=\"Alice \u{202e}Admin\u{2069}^nSpecialist\":mailto:alice@example.com\r\n",
        "ATTENDEE;CN=\"Bob \u{202d}Guest\u{2069}^'The Great^'\";ROLE=REQ-PARTICIPANT:mailto:bob@example.com\r\n",
        "DTSTART:20261101T100000\r\n",
        "DURATION:PT1H\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );

    let event = ical_to_event(original_ical).expect("parse original ical");
    let re_emitted_ical = event_to_ical(&event);
    let event2 = ical_to_event(&re_emitted_ical).expect("parse re-emitted ical");

    assert_eq!(event.title, event2.title, "Title stable across roundtrip");
    assert_eq!(
        event.locations, event2.locations,
        "Locations stable across roundtrip"
    );
    assert_eq!(
        event.participants, event2.participants,
        "Participants stable across roundtrip"
    );

    // Verify re-emitted ical contains no split UTF-8
    assert!(std::str::from_utf8(re_emitted_ical.as_bytes()).is_ok());

    // Verify all physical lines <= 75 octets
    for line in re_emitted_ical.split("\r\n") {
        assert!(line.len() <= 75, "Physical line > 75 octets: {:?}", line);
    }
}

#[test]
fn probe_hostile_unfolded_crlf_and_raw_nul_in_ical_input() {
    // 1. Raw NUL byte inside iCalendar SUMMARY and LOCATION
    let ical_with_nul = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Hostile//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:nul-attack-1\r\n",
        "SUMMARY:Safe\0Injected\r\n",
        "LOCATION:Room\0Secret\r\n",
        "DTSTART:20261101T100000\r\n",
        "DURATION:PT1H\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );

    let event = ical_to_event(ical_with_nul).expect("jmap_ical handles raw NUL without panic");
    assert_eq!(event.title.as_deref(), Some("Safe\0Injected"));
    let loc_val = event.locations.as_ref().unwrap().values().next().unwrap();
    assert_eq!(
        loc_val.get("name").unwrap().as_str().unwrap(),
        "Room\0Secret"
    );

    // Re-emit and ensure no injection
    let re_emitted = event_to_ical(&event);
    assert!(!re_emitted.contains("\r\nInjected"));

    // 2. Attempted property injection via unescaped CRLF in SUMMARY
    // If an ical has an unescaped CRLF followed by an injected property,
    // RFC 5545 treats that unescaped CRLF as starting a new content line.
    let ical_injected_prop = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Hostile//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:inj-attack-1\r\n",
        "SUMMARY:Meeting\r\n",
        "X-INJECTED:Evil\r\n",
        "DTSTART:20261101T100000\r\n",
        "DURATION:PT1H\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let parsed_inj = ical_to_event(ical_injected_prop).expect("parse");
    assert_eq!(parsed_inj.title.as_deref(), Some("Meeting"));

    // 3. Attempted injection via escaped CRLF (\r and \n)
    let ical_escaped_crlf = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Hostile//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:inj-attack-2\r\n",
        "SUMMARY:Meeting\\r\\nLOCATION:Injected Location\\r\\nDESCRIPTION:Injected Desc\r\n",
        "DTSTART:20261101T100000\r\n",
        "DURATION:PT1H\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    let parsed_esc = ical_to_event(ical_escaped_crlf).expect("parse");
    assert_eq!(
        parsed_esc.title.as_deref(),
        Some("Meeting\r\nLOCATION:Injected Location\r\nDESCRIPTION:Injected Desc")
    );
    assert!(
        parsed_esc.locations.is_none(),
        "Escaped CRLF does NOT inject a LOCATION property"
    );
    assert!(
        parsed_esc.description.is_none(),
        "Escaped CRLF does NOT inject a DESCRIPTION property"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]
    #[test]
    fn prop_exotic_unicode_roundtrip(
        title in prop_oneof![
            Just("Meeting \u{202e}LTR-to-RTL\u{2069} review".to_string()),
            Just("Session \u{202d}forced-LTR\u{202c} title".to_string()),
            Just("Arabic \u{2067}مؤتمر\u{2069} event".to_string()),
            Just("Secret\u{200b}Meeting\u{200c}With\u{200d}Zero\u{feff}Width".to_string()),
            Just("Caf\u{00e9} vs Cafe\u{0301}".to_string()),
            Just("Angstr\u{00f6}m and Angstro\u{0308}m".to_string()),
            Just("🦀 Rust 🚀 Launch 👨‍👩‍👧‍👦 Family".to_string()),
            Just("Line 1\r\nLine 2\rLine 3\nLine 4".to_string()),
            "\\PC{1,60}",
        ],
        location_name in prop_oneof![
            Just("Room \u{202e}404\u{2069}".to_string()),
            Just("\u{feff}Building \u{200b}A".to_string()),
            Just("Place with \r\n newlines and \\; escaped chars".to_string()),
            "\\PC{1,40}",
        ],
        organizer_cn in prop_oneof![
            Just("Alice \u{202e}ecilA\u{2069}".to_string()),
            Just("Bob \u{200b}\u{feff}".to_string()),
            Just("Name with \"quotes\" and ^carets^ and \nnewlines".to_string()),
            "\\PC{1,30}",
        ],
    ) {
        // Filter out empty strings because empty title/location/CN are omitted by design per RFC 8984
        prop_assume!(!title.is_empty());
        prop_assume!(!location_name.is_empty());
        prop_assume!(!organizer_cn.is_empty());

        let mut event = base_event();
        event.title = Some(title.clone());

        let mut locations = BTreeMap::new();
        locations.insert("loc1".to_string(), json!({
            "@type": "Location",
            "name": location_name.clone(),
        }));
        event.locations = Some(locations);

        let mut participants = BTreeMap::new();
        participants.insert("part-org".to_string(), json!({
            "@type": "Participant",
            "name": organizer_cn.clone(),
            "calendarAddress": "mailto:organizer@example.com",
            "roles": { "owner": true }
        }));
        event.participants = Some(participants);

        let ics = event_to_ical(&event);

        // 1. Every physical line must be at most 75 octets excluding CRLF
        for line in ics.split("\r\n") {
            prop_assert!(
                line.len() <= 75,
                "Line exceeded 75 octets (len {}): {:?}",
                line.len(),
                line
            );
        }

        // 2. ICS must be valid UTF-8
        prop_assert!(std::str::from_utf8(ics.as_bytes()).is_ok());

        // 3. Round-trip parse must succeed without panic or error
        let parsed = ical_to_event(&ics).expect("parse proptest exotic ics");

        // 4. Title and location must roundtrip
        prop_assert_eq!(parsed.title.as_deref(), Some(title.as_str()));
        let parsed_loc = parsed.locations.expect("locations present");
        let parsed_loc_name = parsed_loc.values().next().unwrap().get("name").unwrap().as_str().unwrap();
        prop_assert_eq!(parsed_loc_name, location_name.as_str());
    }
}
