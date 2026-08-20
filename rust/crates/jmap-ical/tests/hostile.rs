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
        recurrence_rules: Some(vec![RecurrenceRule {
            frequency: "daily\r\nLOCATION:Elsewhere".to_owned(),
            ..RecurrenceRule::default()
        }]),
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
            recurrence_rules: Some(vec![RecurrenceRule {
                frequency: format!("daily{injected}"),
                ..RecurrenceRule::default()
            }]),
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
