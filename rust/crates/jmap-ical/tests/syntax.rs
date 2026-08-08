// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The iCalendar lexer/emitter: components, folding, escaping, parameters.

use jmap_ical::syntax::{Component, Property, parse};

const MINIMAL: &str = concat!(
    "BEGIN:VCALENDAR\r\n",
    "VERSION:2.0\r\n",
    "PRODID:-//example//EN\r\n",
    "BEGIN:VEVENT\r\n",
    "UID:1234\r\n",
    "SUMMARY:Standup\r\n",
    "END:VEVENT\r\n",
    "END:VCALENDAR\r\n",
);

#[test]
fn parses_a_calendar_and_its_nested_event() {
    let calendar = parse(MINIMAL).expect("parse");

    assert_eq!(calendar.name, "VCALENDAR");
    // BEGIN/END are structure, not data, and are not handed out as properties.
    let names: Vec<&str> = calendar
        .properties
        .iter()
        .map(|property| property.name.as_str())
        .collect();
    assert_eq!(names, ["VERSION", "PRODID"]);

    let event = calendar.child("VEVENT").expect("a VEVENT");
    assert_eq!(event.text("UID").as_deref(), Some("1234"));
    assert_eq!(event.text("SUMMARY").as_deref(), Some("Standup"));
    assert_eq!(event.text("DESCRIPTION"), None);
}

#[test]
fn rejects_input_that_is_not_a_well_formed_calendar() {
    // No BEGIN:VCALENDAR at all.
    assert!(parse("UID:1234\r\n").is_err());
    // A component that is never closed.
    assert!(parse("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n").is_err());
    // An END that closes a component nobody opened.
    assert!(
        parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VTODO\r\nEND:VCALENDAR\r\n").is_err(),
        "END:VTODO must not close a VEVENT"
    );
    // Content after the calendar is closed: a second calendar in one stream
    // is not something this layer is allowed to silently drop.
    assert!(parse(&format!("{MINIMAL}{MINIMAL}")).is_err());
}

#[test]
fn unfolds_continuation_lines() {
    // RFC 5545 §3.1: CRLF followed by a single space or tab is a fold. Bare
    // LF appears in the wild, and in files people hand-edit, so accept it.
    let calendar = parse(concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "DESCRIPTION:one\r\n two\n\tthree\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    ))
    .expect("parse");

    let event = calendar.child("VEVENT").expect("a VEVENT");
    assert_eq!(event.text("DESCRIPTION").as_deref(), Some("onetwothree"));
}

#[test]
fn parses_parameters_including_quoted_values() {
    let calendar = parse(concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "DTSTART;TZID=Europe/Berlin;X-ODD=\"we;ird\":20260115T130000\r\n",
        "ATTENDEE;ROLE=REQ-PARTICIPANT;RSVP=TRUE:mailto:vera@example.com\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    ))
    .expect("parse");
    let event = calendar.child("VEVENT").expect("a VEVENT");

    let start = event.property("DTSTART").expect("a DTSTART");
    assert_eq!(start.raw_value(), "20260115T130000");
    assert_eq!(start.param("TZID"), Some("Europe/Berlin"));
    assert_eq!(start.param("X-ODD"), Some("we;ird"));
    assert_eq!(start.param("VALUE"), None);

    // A value may contain colons; only the first one separates.
    let attendee = event.property("ATTENDEE").expect("an ATTENDEE");
    assert_eq!(attendee.raw_value(), "mailto:vera@example.com");
    assert_eq!(attendee.param("RSVP"), Some("TRUE"));
}

#[test]
fn component_property_and_parameter_names_are_case_insensitive() {
    let calendar = parse(concat!(
        "begin:vcalendar\r\n",
        "begin:vevent\r\n",
        "dtstart;tzid=Europe/Berlin:20260115T130000\r\n",
        "end:vevent\r\n",
        "end:vcalendar\r\n",
    ))
    .expect("parse");

    let event = calendar.child("VEVENT").expect("a VEVENT");
    assert_eq!(
        event.property("DTSTART").expect("a DTSTART").param("TZID"),
        Some("Europe/Berlin")
    );
}

#[test]
fn names_are_upper_cased_on_the_way_in_too() {
    // Not only when parsing: a component built with a lower-case name would
    // otherwise emit `BEGIN:vevent` and go missing from its own accessors.
    let calendar = Component::new("vcalendar")
        .with_child(Component::new("vevent").with(Property::new("summary", "Standup")));

    assert!(
        calendar.child("VEVENT").is_some_and(|event| event
            .text("SUMMARY")
            .is_some_and(|summary| summary == "Standup")),
        "{calendar:?}"
    );
    assert_eq!(
        calendar.to_ics(),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
}

#[test]
fn unescapes_text_but_leaves_raw_values_alone() {
    let calendar = parse(concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "DESCRIPTION:a\\,b\\nc\\\\d\\;e\r\n",
        "CATEGORIES:home\\,away,work\r\n",
        "RRULE:FREQ=WEEKLY;BYDAY=MO,TU\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    ))
    .expect("parse");
    let event = calendar.child("VEVENT").expect("a VEVENT");

    assert_eq!(
        event.text("DESCRIPTION").as_deref(),
        Some("a,b\nc\\d;e"),
        "TEXT escapes are resolved"
    );
    // A comma inside a TEXT list separates values unless it is escaped.
    assert_eq!(
        event.property("CATEGORIES").expect("CATEGORIES").texts(),
        ["home,away", "work"]
    );
    // RRULE is not TEXT: its commas and semicolons are structure, and
    // unescaping them would corrupt the rule.
    assert_eq!(
        event.property("RRULE").expect("RRULE").raw_value(),
        "FREQ=WEEKLY;BYDAY=MO,TU"
    );
}

#[test]
fn collects_repeated_properties_in_order() {
    let calendar = parse(concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VEVENT\r\n",
        "ATTENDEE:mailto:vera@example.com\r\n",
        "ATTENDEE:mailto:wim@example.com\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    ))
    .expect("parse");
    let event = calendar.child("VEVENT").expect("a VEVENT");

    let attendees: Vec<&str> = event
        .all("ATTENDEE")
        .iter()
        .map(|property| property.raw_value())
        .collect();
    assert_eq!(
        attendees,
        ["mailto:vera@example.com", "mailto:wim@example.com"]
    );
    // …and the first one is what the singular accessor hands back.
    assert_eq!(
        event.property("ATTENDEE").expect("an ATTENDEE").raw_value(),
        "mailto:vera@example.com"
    );
}

#[test]
fn writes_crlf_terminated_lines_with_nested_components() {
    let event = Component::new("VEVENT")
        .with(Property::new("UID", "1234"))
        .with(Property::new("SUMMARY", "Standup"))
        .with_child(Component::new("VALARM").with(Property::raw("TRIGGER", "-PT15M")));
    let calendar = Component::new("VCALENDAR")
        .with(Property::raw("VERSION", "2.0"))
        .with_child(event);

    assert_eq!(
        calendar.to_ics(),
        concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:1234\r\n",
            "SUMMARY:Standup\r\n",
            "BEGIN:VALARM\r\n",
            "TRIGGER:-PT15M\r\n",
            "END:VALARM\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        )
    );
}

#[test]
fn writes_escaped_text_quoted_parameters_and_raw_values() {
    let calendar = Component::new("VCALENDAR").with_child(
        Component::new("VEVENT")
            .with(Property::new("DESCRIPTION", "a,b\nc\\d;e"))
            .with(Property::raw("DTSTART", "20260115T130000").with_param("TZID", "Europe/Berlin"))
            .with(Property::raw("RRULE", "FREQ=WEEKLY;BYDAY=MO,TU"))
            .with(Property::new("SUMMARY", "odd").with_param("X-ODD", "we;ird")),
    );
    let text = calendar.to_ics();

    assert!(
        text.contains("\r\nDESCRIPTION:a\\,b\\nc\\\\d\\;e\r\n"),
        "{text}"
    );
    assert!(
        text.contains("\r\nDTSTART;TZID=Europe/Berlin:20260115T130000\r\n"),
        "{text}"
    );
    assert!(
        text.contains("\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU\r\n"),
        "{text}"
    );
    assert!(
        text.contains("\r\nSUMMARY;X-ODD=\"we;ird\":odd\r\n"),
        "{text}"
    );
}

#[test]
fn round_trips_what_it_writes() {
    let calendar = parse(&parse(MINIMAL).expect("parse").to_ics()).expect("reparse");
    let event = calendar.child("VEVENT").expect("a VEVENT");
    assert_eq!(event.text("SUMMARY").as_deref(), Some("Standup"));
    assert_eq!(calendar.to_ics(), MINIMAL);
}

#[test]
fn folds_long_lines_without_splitting_characters() {
    // Two widths, because they fail differently: a one-octet value catches an
    // off-by-one in the limit (the continuation's leading space counts against
    // it), a multi-octet one catches a fold placed mid-character, which would
    // make the whole calendar undecodable.
    for value in ["x".repeat(400), "ä".repeat(200)] {
        let text = Component::new("VCALENDAR")
            .with_child(Component::new("VEVENT").with(Property::new("DESCRIPTION", &value)))
            .to_ics();

        assert!(text.contains("\r\n "), "not folded at all:\n{text}");
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "line of {} octets: {line}", line.len());
        }
        let calendar = parse(&text).expect("parse");
        let event = calendar.child("VEVENT").expect("a VEVENT");
        assert_eq!(event.text("DESCRIPTION").as_deref(), Some(value.as_str()));
    }
}
