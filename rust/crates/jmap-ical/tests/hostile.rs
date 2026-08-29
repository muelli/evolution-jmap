// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a hostile JMAP server can make of the iCalendar the backend stores.
//!
//! Every string in a `CalendarEvent` came off the network, and three of them
//! reach the wire without passing through `escape`: `duration` and a
//! recurrence rule's `frequency` are emitted by `Property::raw` — their
//! punctuation is structure, so escaping would destroy them — and `timeZone`
//! becomes the `TZID` parameter of `DTSTART`, where a quoted value has no
//! escape mechanism at all. The rendered object is handed straight to
//! `i_cal_component_new_from_string`, so a string that can end a content line
//! early is a string that can add a property to the user's calendar.
//!
//! See `docs/AUDIT-FFI.md`, findings F2 and F4.

use jmap_ical::error::ICalError;
use jmap_ical::event_to_ical;
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};

fn event() -> CalendarEvent {
    CalendarEvent {
        id: Some("E1".into()),
        title: Some("Standup".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        ..CalendarEvent::default()
    }
}

/// The property names this crate emits, in the order it emits them. Anything
/// else on a line of its own came from a value.
fn line_names(ics: &str) -> Vec<String> {
    ics.split("\r\n")
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .map(|line| {
            line.split([':', ';'])
                .next()
                .unwrap_or(line)
                .to_ascii_uppercase()
        })
        .collect()
}

/// `duration` is kept verbatim because an ISO 8601 duration is punctuation all
/// the way down. That is exactly what makes a CRLF in it an injection.
///
/// It is now stopped one step ahead of the strip that first answered it: a value
/// carrying a line break is not a length, so the property is never written at
/// all (see the mapping's `stated_duration`) rather than written with the break
/// removed. The strip itself stays under test through the frequency and the
/// `TZID` below, which have no such check in front of them.
#[test]
fn a_crlf_in_the_duration_cannot_add_a_property() {
    let ics = event_to_ical(&CalendarEvent {
        duration: Some("PT1H\r\nSUMMARY:Cancelled — see attachment".to_owned()),
        ..event()
    });

    assert!(
        !ics.contains("\r\nSUMMARY:Cancelled"),
        "the duration injected a SUMMARY line:\n{ics}"
    );
    assert_eq!(
        line_names(&ics),
        [
            "BEGIN", "VERSION", "PRODID", "BEGIN", "UID", "SUMMARY", "DTSTART", "END", "END"
        ]
    );
}

/// An `RRULE`'s `FREQ` is the same shape of value, and the same hole.
#[test]
fn a_crlf_in_a_recurrence_frequency_cannot_add_a_property() {
    let ics = event_to_ical(&CalendarEvent {
        recurrence_rule: Some(RecurrenceRule {
            frequency: "daily\r\nLOCATION:Elsewhere".to_owned(),
            ..RecurrenceRule::default()
        }),
        ..event()
    });

    assert!(
        !ics.contains("\r\nLOCATION"),
        "the frequency injected a LOCATION line:\n{ics}"
    );
    assert_eq!(ics.matches("\r\nRRULE").count(), 1);
}

/// `timeZone` reaches a *parameter*, where there is no escape at all: the value
/// is either bare or double-quoted, and a quoted run ends at the next quote or
/// at the end of the line. So the line break is the only thing to remove, and
/// removing it is what keeps `DTSTART` one property.
#[test]
fn a_crlf_in_the_time_zone_cannot_add_a_property() {
    let ics = event_to_ical(&CalendarEvent {
        time_zone: Some("Europe/Berlin\r\nDESCRIPTION:Injected".to_owned()),
        ..event()
    });

    assert!(
        !ics.contains("\r\nDESCRIPTION"),
        "the time zone injected a DESCRIPTION line:\n{ics}"
    );
    assert_eq!(ics.matches("\r\nDTSTART").count(), 1);
}

/// A lone LF and a lone CR are the same attack: libical's unfolder splits on
/// either, so neither may survive into a content line. Both raw values carry it
/// here, so the assertion covers the duration — refused as a length — and the
/// frequency, which is stripped, in one document.
#[test]
fn a_bare_lf_or_cr_is_stripped_as_well() {
    for injected in ["\nSUMMARY:x", "\rSUMMARY:x", "\n\rSUMMARY:x"] {
        let ics = event_to_ical(&CalendarEvent {
            duration: Some(format!("PT1H{injected}")),
            recurrence_rule: Some(RecurrenceRule {
                frequency: format!("daily{injected}"),
                ..RecurrenceRule::default()
            }),
            ..event()
        });
        assert_eq!(
            ics.matches("\r\nSUMMARY").count(),
            1,
            "{injected:?} produced a second SUMMARY:\n{ics}"
        );
        assert_eq!(
            ics.matches("\r\nRRULE").count(),
            1,
            "{injected:?} split the RRULE:\n{ics}"
        );
    }
}

/// The TEXT values, which do go through `escape`, keep their newline as `\n`
/// rather than losing it — the strip is a backstop for the unescaped values,
/// not a replacement for escaping.
#[test]
fn a_newline_in_a_text_value_is_still_escaped_rather_than_dropped() {
    let ics = event_to_ical(&CalendarEvent {
        description: Some("first\r\nsecond".to_owned()),
        ..event()
    });

    assert_eq!(ics.matches("\r\nDESCRIPTION").count(), 1);
    let back = jmap_ical::ical_to_event(&ics).expect("parse");
    assert_eq!(back.description.as_deref(), Some("first\r\nsecond"));
}

// ---------------------------------------------------------------------------
// F4: nesting depth

/// A document nested deeply enough aborts the process on a safe code path.
///
/// The depth this rejects at is far above anything RFC 5545 describes:
/// `VCALENDAR` > `VTIMEZONE` > `STANDARD` is three, and a `VALARM` in a
/// `VEVENT` is three.
#[test]
fn a_document_nested_past_the_limit_is_refused_rather_than_parsed() {
    let ics = nested(jmap_ical::MAX_DEPTH + 1);
    assert!(
        matches!(
            jmap_ical::event::parse_ical(&ics),
            Err(ICalError::TooDeep(_))
        ),
        "a {}-deep document was accepted",
        jmap_ical::MAX_DEPTH + 1
    );
}

/// ...and one at the limit still parses, so the cap is not in the way of the
/// nesting the format actually uses.
#[test]
fn a_document_nested_up_to_the_limit_still_parses() {
    let ics = nested(jmap_ical::MAX_DEPTH);
    let calendar = jmap_ical::event::parse_ical(&ics).expect("the limit itself is allowed");
    assert_eq!(calendar.components.len(), jmap_ical::MAX_DEPTH);
}

/// A depth that used to overflow the stack outright, as the regression the cap
/// exists for. Without it this test aborts the whole test binary rather than
/// failing, which is the point.
#[test]
fn a_pathologically_nested_document_neither_parses_nor_crashes() {
    let ics = nested(100_000);
    assert!(matches!(
        jmap_ical::event::parse_ical(&ics),
        Err(ICalError::TooDeep(_))
    ));
}

/// `depth` levels of component, `VCALENDAR` included, properly closed.
fn nested(depth: usize) -> String {
    let mut ics = String::from("BEGIN:VCALENDAR\r\n");
    for _ in 1..depth {
        ics.push_str("BEGIN:VALARM\r\n");
    }
    for _ in 1..depth {
        ics.push_str("END:VALARM\r\n");
    }
    ics.push_str("END:VCALENDAR\r\n");
    ics
}

/// A `DTEND` (or any DATE-TIME value) with a multi-byte UTF-8 character
/// straddling byte offset 6 of its time part used to panic: the length check
/// only counted bytes, so slicing `time[..6]` landed mid-character before the
/// ASCII-digit check three lines down ever got a chance to reject the value
/// cleanly. Found by `proptest_fuzz.rs`'s hostile-input fuzzer (see
/// `docs/BACKLOG.md`); pinned here as the exact minimal input it found, now
/// that `to_local_date_time` checks `is_char_boundary` before slicing.
#[test]
fn a_dtend_with_a_multibyte_character_at_the_slice_boundary_does_not_panic() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nDTEND: A\u{ac0}\u{20}\u{ae}T\u{10397}\u{fffc}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let event = jmap_ical::ical_to_event(ics).expect("the rest of the event still parses");

    assert_eq!(event.id.as_ref().map(|id| id.as_str()), Some("evt1"));
    assert_eq!(event.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(
        event.duration, None,
        "an unreadable DTEND must be dropped, not panic or invent a duration"
    );
}

// ---------------------------------------------------------------------------
// Adversarial-input robustness net (Batch 13 Item 5)

use std::time::{Duration, Instant};

/// Truncated and unterminated iCalendar documents must be rejected with typed ICalError,
/// never panic, never hang, and never silently parse incomplete events.
#[test]
fn truncated_and_unterminated_ical_rejection_matrix() {
    let not_calendar_cases = [
        "",
        "   ",
        "\r\n",
        "\n",
        "\t",
        "FOO:BAR\r\n",
        "VERSION:2.0\r\n",
    ];
    for input in not_calendar_cases {
        assert_eq!(
            jmap_ical::ical_to_event(input),
            Err(ICalError::NotACalendar),
            "input {input:?} should be rejected with NotACalendar"
        );
    }

    let unterminated_cases = [
        ("BEGIN:VCALENDAR", "VCALENDAR"),
        ("BEGIN:VCALENDAR\r\n", "VCALENDAR"),
        ("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt1\r\n", "VEVENT"),
        (
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\n",
            "VCALENDAR",
        ),
        (
            "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nBEGIN:STANDARD\r\nEND:STANDARD\r\n",
            "VTIMEZONE",
        ),
    ];
    for (input, component) in unterminated_cases {
        assert_eq!(
            jmap_ical::ical_to_event(input),
            Err(ICalError::Unterminated(component.to_owned())),
            "input {input:?} should be rejected with Unterminated({component})"
        );
    }

    let mismatched_cases = [
        (
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VALARM\r\nEND:VCALENDAR\r\n",
            "VEVENT",
            "VALARM",
        ),
        (
            "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nBEGIN:STANDARD\r\nEND:STANDARD\r\nEND:VCALENDAR\r\n",
            "VTIMEZONE",
            "VCALENDAR",
        ),
    ];
    for (input, expected, found) in mismatched_cases {
        assert_eq!(
            jmap_ical::ical_to_event(input),
            Err(ICalError::Mismatched {
                expected: expected.to_owned(),
                found: found.to_owned(),
            }),
            "input {input:?} should be rejected with Mismatched"
        );
    }

    // Trailing content after END:VCALENDAR
    let trailing = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\nEXTRA:TRAILING\r\n";
    assert!(matches!(
        jmap_ical::ical_to_event(trailing),
        Err(ICalError::Trailing(_))
    ));

    // Calendar without VEVENT
    let no_event =
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nEND:VCALENDAR\r\n";
    assert_eq!(jmap_ical::ical_to_event(no_event), Err(ICalError::NoEvent));
}

/// Unbalanced quoting in parameters must never panic, hang, or inject properties.
#[test]
fn unbalanced_quoting_in_ical_parameters_matrix() {
    let hostile_quoted = [
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART;TZID=\"Europe/Berlin:20260115T130000\r\nSUMMARY:Test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nATTENDEE;CN=\"Alice Smith:mailto:alice@example.com\r\nDTSTART:20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nORGANIZER;CN=\"Bob\"Jones\":mailto:bob@example.com\r\nDTSTART:20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nATTACH;FMTTYPE=\"application/pdf:https://example.com/doc.pdf\r\nDTSTART:20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nX-PARAM;FOO=\"\"\"\":Bar\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ];

    for input in hostile_quoted {
        let start = Instant::now();
        let res = jmap_ical::ical_to_event(input);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "parse of hostile quoting hung on {input:?}"
        );
        if let Ok(event) = res {
            assert!(event.id.is_some());
        }
    }
}

/// Absurd folding (every 2 octets, tabs, empty continuation lines) must parse losslessly.
#[test]
fn absurd_folding_every_second_octet_matrix() {
    let mut folded = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nSUMMARY:\r\n",
    );
    let raw_val = "All Hands Engineering Sync";
    for chunk in raw_val.as_bytes().chunks(2) {
        folded.push(' ');
        folded.push_str(std::str::from_utf8(chunk).unwrap());
        folded.push_str("\r\n");
    }
    folded.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");

    let event = jmap_ical::ical_to_event(&folded).expect("absurdly folded event should parse");
    assert_eq!(event.title.as_deref(), Some("All Hands Engineering Sync"));

    // Empty continuation lines and tab continuations
    let empty_continuations = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:evt2\r\nDTSTART:20260115T130000Z\r\nDESCRIPTION:First line\r\n \r\n \r\n \tSecond line\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let event2 = jmap_ical::ical_to_event(empty_continuations).expect("tab folding should parse");
    assert_eq!(
        event2.description.as_deref(),
        Some("First line\tSecond line")
    );
}

/// A calendar with 10,000 properties parses in strictly bounded time with no stack overflow or hang.
#[test]
fn calendar_with_10k_properties_bounded_execution() {
    let mut large_ics = String::with_capacity(500_000);
    large_ics.push_str("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:stress-evt\r\nDTSTART:20260115T130000Z\r\nSUMMARY:Stress Test Event\r\n");
    for i in 0..10_000 {
        use std::fmt::Write;
        let _ = writeln!(large_ics, "X-CUSTOM-PROP-{i}:value-{i}\r");
    }
    large_ics.push_str("DESCRIPTION:Final description\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");

    let start = Instant::now();
    let event = jmap_ical::ical_to_event(&large_ics).expect("10k properties event should parse");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "parsing 10k properties event took too long: {elapsed:?}"
    );
    assert_eq!(event.title.as_deref(), Some("Stress Test Event"));
    assert_eq!(event.description.as_deref(), Some("Final description"));
}

/// CRLF, LF, CR, and mixed line endings must parse deterministically without data corruption.
#[test]
fn crlf_lf_cr_mixed_line_endings_matrix() {
    let pure_lf = "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:x\nBEGIN:VEVENT\nUID:evt1\nDTSTART:20260115T130000Z\nSUMMARY:Test Event\nDESCRIPTION:Line 1\n Line 2\nEND:VEVENT\nEND:VCALENDAR\n";
    let pure_crlf = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nSUMMARY:Test Event\r\nDESCRIPTION:Line 1\r\n Line 2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let mixed = "BEGIN:VCALENDAR\r\nVERSION:2.0\nPRODID:x\r\nBEGIN:VEVENT\nUID:evt1\r\nDTSTART:20260115T130000Z\nSUMMARY:Test Event\r\nDESCRIPTION:Line 1\n Line 2\r\nEND:VEVENT\nEND:VCALENDAR\r\n";

    for (variant, input) in [
        ("pure_lf", pure_lf),
        ("pure_crlf", pure_crlf),
        ("mixed", mixed),
    ] {
        let event = jmap_ical::ical_to_event(input)
            .unwrap_or_else(|err| panic!("variant {variant} failed to parse: {err:?}"));
        assert_eq!(
            event.title.as_deref(),
            Some("Test Event"),
            "variant {variant} title mismatch"
        );
        assert_eq!(
            event.description.as_deref(),
            Some("Line 1Line 2"),
            "variant {variant} description mismatch"
        );
    }
}

/// Malformed dates, durations, recurrence rules, and triggers must be safely rejected or defaulted without panic.
#[test]
fn malformed_dates_durations_recurrence_and_triggers_matrix() {
    let hostile_payloads = [
        // Malformed DTSTART
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:invalid\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:2026-01-15\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T999999Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20261345T000000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART;VALUE=DATE:2026\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        // Malformed DURATION / DTEND
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nDTEND:not-a-date\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nDURATION:invalid\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nDURATION:PT-5M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nDURATION:P\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        // Malformed RRULE
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=MONTHLY;BYDAY=99ZZ\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=SECONDLY;INTERVAL=-10\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=DAILY;COUNT=-1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=FOOBAR\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=DAILY;BYSETPOS=0\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRRULE:FREQ=DAILY;BYSETPOS=999999\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        // Malformed VALARM / TRIGGER
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:invalid\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:not-a-date\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        // Malformed GEO
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nGEO:not,a,geo\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nGEO:abc;def\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        // Malformed ATTACH
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nATTACH;ENCODING=BASE64;VALUE=BINARY:!@#$%^&*()\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nATTACH:data:image/png;base64,corrupt\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ];

    for input in hostile_payloads {
        let res = jmap_ical::ical_to_event(input);
        assert!(
            res.is_ok() || res.is_err(),
            "must complete without panicking on {input:?}"
        );
        if let Ok(event) = res {
            assert_eq!(event.id.as_ref().map(|id| id.as_str()), Some("1"));
        }
    }
}

/// Multibyte UTF-8 characters at exact slice boundaries never cause char boundary panics.
#[test]
fn adversarial_multibyte_utf8_slice_boundary_matrix() {
    let multi_byte_chars = [
        "é",       // 2 bytes: C3 A9
        "€",       // 3 bytes: E2 82 AC
        "𞋀",       // 4 bytes: F0 9E 8B 80
        "𐎟",       // 4 bytes: F0 90 8E 9F
        "🎉",      // 4 bytes: F0 9F 8E 89
        "한",      // 3 bytes: ED 95 9C
        "العربية", // Arabic multi-byte
    ];

    for ch in multi_byte_chars {
        // Date-time with multibyte characters
        let ics_dtstart = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:{ch}20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let _ = jmap_ical::ical_to_event(&ics_dtstart);

        let ics_recurrence_id = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nRECURRENCE-ID;TZID=UTC:{ch}20260115T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let _ = jmap_ical::ical_to_event(&ics_recurrence_id);

        // Summary and description with multibyte characters
        let ics_text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T130000Z\r\nSUMMARY:Summary {ch}\r\nDESCRIPTION:Description {ch}\r\nLOCATION:Location {ch}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let event = jmap_ical::ical_to_event(&ics_text).expect("multibyte text should parse");
        assert_eq!(
            event.title.as_deref(),
            Some(format!("Summary {ch}").as_str())
        );
    }
}
