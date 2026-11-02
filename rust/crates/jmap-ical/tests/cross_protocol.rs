// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-protocol audit (ROADMAP 169), area "JMAP strings into iCal text".
//!
//! Every string a JMAP server sends in a `CalendarEvent` goes into one
//! iCalendar content line. A server string must not add a content line, and
//! must not close a quoted parameter value to add a parameter. These tests
//! put one hostile payload into each string slot and check the output.

use jmap_ical::event_to_ical;
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Value, json};

/// A payload that tries to add a property line.
const LINE: &str = "x\r\nX-INJECTED:1\r\nY";
/// A payload that tries to add a property line with a bare LF.
const BARE_LF: &str = "x\nX-INJECTED:1\nY";
/// A payload that tries to close a quoted parameter and add a parameter.
const PARAM: &str = "a\";X-EVIL=1;Q=\"b";

fn base() -> CalendarEvent {
    let mut event = CalendarEvent::default();
    event.id = Some("E1".into());
    event.uid = Some("uid-1".to_owned());
    event.title = Some("Standup".to_owned());
    event.start = Some("2026-01-15T13:00:00".to_owned());
    event
}

/// Unfolded content-line names of an iCalendar document.
fn names(ics: &str) -> Vec<String> {
    ics.replace("\r\n ", "")
        .replace("\r\n\t", "")
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split([':', ';'])
                .next()
                .unwrap_or(line)
                .to_ascii_uppercase()
        })
        .collect()
}

/// The name and parameters of a content line, without the text of quoted
/// parameter values: all text before the first `:` outside quotes.
fn head(line: &str) -> String {
    let mut quoted = false;
    let mut out = String::new();
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => break,
            _ if !quoted => out.push(c),
            _ => {}
        }
    }
    out
}

/// Every event shape that carries `payload` in one string slot.
fn slots(payload: &str) -> Vec<(&'static str, CalendarEvent)> {
    let p = payload.to_owned();
    let mut out = Vec::new();
    let mut put = |name: &'static str, f: &dyn Fn(&mut CalendarEvent)| {
        let mut event = base();
        f(&mut event);
        out.push((name, event));
    };
    put("title", &|e| e.title = Some(p.clone()));
    put("description", &|e| e.description = Some(p.clone()));
    put("uid", &|e| e.uid = Some(p.clone()));
    put("status", &|e| e.status = Some(p.clone()));
    put("privacy", &|e| e.privacy = Some(p.clone()));
    put("freeBusyStatus", &|e| e.free_busy_status = Some(p.clone()));
    put("organizerCalendarAddress", &|e| {
        e.organizer_calendar_address = Some(p.clone())
    });
    put("keywords", &|e| {
        e.keywords = Some([(p.clone(), json!(true))].into_iter().collect())
    });
    put("locations.name", &|e| {
        e.locations = Some(
            [("L1".to_owned(), json!({"@type": "Location", "name": p}))]
                .into_iter()
                .collect(),
        )
    });
    put("virtualLocations.uri", &|e| {
        e.virtual_locations = Some(
            [(
                "V1".to_owned(),
                json!({"@type": "VirtualLocation", "uri": format!("https://x.example/{p}"), "name": p}),
            )]
            .into_iter()
            .collect(),
        )
    });
    put("links.href", &|e| {
        e.links = Some(
            [(
                "K1".to_owned(),
                json!({"@type": "Link", "href": format!("https://x.example/{p}"), "title": p, "contentType": p}),
            )]
            .into_iter()
            .collect(),
        )
    });
    put("participants", &|e| {
        let participant: Value = json!({
            "@type": "Participant",
            "name": p,
            "email": format!("{p}@x.example"),
            "calendarAddress": format!("mailto:{p}@x.example"),
            "roles": {"attendee": true},
            "participationStatus": p,
            "kind": p,
        });
        e.participants = Some([("P1".to_owned(), participant)].into_iter().collect())
    });
    put("alerts", &|e| {
        e.alerts = Some(
            [(
                "A1".to_owned(),
                json!({"@type": "Alert", "action": p, "trigger": {"@type": "OffsetTrigger", "offset": p}}),
            )]
            .into_iter()
            .collect(),
        )
    });
    out
}

#[test]
fn no_string_slot_adds_a_content_line() {
    for payload in [LINE, BARE_LF] {
        for (slot, event) in slots(payload) {
            let ics = event_to_ical(&event);
            assert!(
                !names(&ics).iter().any(|name| name == "X-INJECTED"),
                "slot {slot} added a content line:\n{ics}"
            );
        }
    }
}

#[test]
fn no_string_slot_closes_a_quoted_parameter() {
    for (slot, event) in slots(PARAM) {
        let ics = event_to_ical(&event).replace("\r\n ", "");
        for line in ics.split("\r\n") {
            assert!(
                !head(line).contains("X-EVIL"),
                "slot {slot} added a parameter:\n{ics}"
            );
        }
    }
}

/// A URI value (RFC 5545 Section 3.3.13) is not TEXT: a `;` or `,` in it is
/// written as is, never with a TEXT backslash escape. With the escape,
/// libical and Evolution keep the backslash and open a different URL.
#[test]
fn a_uri_value_is_not_text_escaped() {
    let uri = "https://x.example/j/1;p=2,3";
    let mut event = base();
    event.links = Some(
        [("K1".to_owned(), json!({"@type": "Link", "href": uri}))]
            .into_iter()
            .collect(),
    );
    event.virtual_locations = Some(
        [(
            "V1".to_owned(),
            json!({"@type": "VirtualLocation", "uri": uri}),
        )]
        .into_iter()
        .collect(),
    );
    event.participants = Some(
        [(
            "P1".to_owned(),
            json!({"@type": "Participant", "calendarAddress": "mailto:a,b;c@x.example",
                   "roles": {"attendee": true}}),
        )]
        .into_iter()
        .collect(),
    );
    let ics = event_to_ical(&event).replace("\r\n ", "");
    for name in ["ATTACH", "CONFERENCE", "ATTENDEE"] {
        let line = ics
            .lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} line:\n{ics}"));
        assert!(!line.contains('\\'), "{name} value is TEXT-escaped: {line}");
    }
    assert!(ics.contains(":https://x.example/j/1;p=2,3\r\n"), "{ics}");
    assert!(ics.contains(":mailto:a,b;c@x.example\r\n"), "{ics}");
}
