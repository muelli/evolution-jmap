// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The guest list on the way back. A meeting edited in Evolution has to show
//! the same guests on every other client of the account, so `participants` is
//! patched; and since a guest is told apart by address while the server files
//! it under a key of its own choosing, every change lands *in* the server's own
//! entry, leaving what the `ATTENDEE` line has no room for (`sendTo`,
//! `invitedBy`, a scheduling sequence) where it was.

mod common;

use common::Fixture;
use jmap_cal_sync::patch::diff;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Map, Value, json};

/// A meeting as the server holds it, with the guest list given.
fn meeting(participants: Value) -> CalendarEvent {
    serde_json::from_value(json!({
        "@type": "Event",
        "uid": "meeting-1",
        "title": "Planning",
        "start": "2026-01-15T13:00:00",
        "timeZone": "Europe/Berlin",
        "duration": "PT1H",
        "organizerCalendarAddress": "mailto:olga@example.com",
        "participants": participants,
    }))
    .expect("a CalendarEvent")
}

/// The component Evolution was shown, unfolded so a test can edit one line.
fn drawn(event: &CalendarEvent) -> String {
    event_to_ical(event).replace("\r\n ", "")
}

/// What EDS hands back after the user edited the drawing.
fn saved(icalendar: &str) -> CalendarEvent {
    ical_to_event(icalendar).expect("an edited component parses")
}

fn patch_of(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(path, value)| ((*path).to_owned(), value.clone()))
        .collect()
}

/// The shape both Stalwart and Fastmail store a meeting in once they have
/// normalised it: the organizer's `{owner, attendee}` comes back as `{owner}`,
/// and a plain guest's `{attendee}` comes back with no `roles` at all.
fn normalised_meeting() -> CalendarEvent {
    meeting(json!({
        "o": {
            "@type": "Participant",
            "name": "Olga",
            "calendarAddress": "mailto:olga@example.com",
            "roles": {"owner": true},
            "participationStatus": "accepted",
        },
        "v": {
            "@type": "Participant",
            "name": "Vera",
            "calendarAddress": "mailto:vera@example.com",
            "participationStatus": "needs-action",
            "sendTo": {"imip": "mailto:vera@example.com"},
            "invitedBy": "o",
        },
    }))
}

#[test]
fn an_edit_that_leaves_the_guests_alone_sends_none_against_normalised_roles() {
    let current = normalised_meeting();
    // What Evolution's editor writes back for the same meeting: the organizer
    // on its own guest list too, and every role spelled out.
    let icalendar = drawn(&current)
        .replace("SUMMARY:Planning", "SUMMARY:Planning (moved)")
        .replace(
            "ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:",
            "ATTENDEE;CN=Olga;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:olga@example.com\r\n\
             ATTENDEE;CN=Vera;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION:",
        );
    assert!(icalendar.contains("ROLE=REQ-PARTICIPANT"), "{icalendar}");

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(
        patch,
        patch_of(&[("title", json!("Planning (moved)"))]),
        "an unchanged guest list must not be patched back on every save"
    );
}

#[test]
fn a_guests_reply_is_patched_into_the_servers_own_entry() {
    let current = normalised_meeting();
    let icalendar = drawn(&current).replace(
        "ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:",
        "ATTENDEE;CN=Vera;PARTSTAT=ACCEPTED:",
    );

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(
        patch,
        patch_of(&[("participants/v/participationStatus", json!("accepted"))])
    );
}

#[test]
fn a_guest_made_optional_keeps_the_roles_the_line_cannot_show() {
    let current = meeting(json!({
        "o": {
            "@type": "Participant",
            "calendarAddress": "mailto:olga@example.com",
            "roles": {"owner": true},
        },
        "v": {
            "@type": "Participant",
            "calendarAddress": "mailto:vera@example.com",
            "roles": {"attendee": true, "contact": true},
        },
    }));
    let icalendar = drawn(&current).replace(
        "ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:vera",
        "ATTENDEE;ROLE=OPT-PARTICIPANT:mailto:vera",
    );
    assert!(icalendar.contains("OPT-PARTICIPANT"), "{icalendar}");

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(
        patch,
        patch_of(&[(
            "participants/v/roles",
            json!({"attendee": true, "optional": true, "contact": true})
        )])
    );
}

/// A role that the `ROLE` parameter can state goes when the line stops stating it.
/// A role that the line cannot state stays.
#[test]
fn a_chair_made_a_plain_guest_loses_the_chair_role_and_keeps_the_rest() {
    let current = meeting(json!({
        "o": {
            "@type": "Participant",
            "calendarAddress": "mailto:olga@example.com",
            "roles": {"owner": true},
        },
        "v": {
            "@type": "Participant",
            "calendarAddress": "mailto:vera@example.com",
            "roles": {"attendee": true, "chair": true, "contact": true},
        },
    }));
    let icalendar = drawn(&current).replace(
        "ATTENDEE;ROLE=CHAIR:mailto:vera",
        "ATTENDEE;ROLE=REQ-PARTICIPANT:mailto:vera",
    );
    assert!(icalendar.contains("REQ-PARTICIPANT"), "{icalendar}");

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(
        patch,
        patch_of(&[(
            "participants/v/roles",
            json!({"attendee": true, "contact": true})
        )])
    );
}

#[test]
fn a_guest_added_in_evolution_is_filed_under_a_key_the_server_does_not_use() {
    // The server's own keys here are the very ones this crate invents when it
    // reads a guest list off a component, so taking the edited event's key as
    // it came would overwrite the organizer.
    let current = meeting(json!({
        "p1": {
            "@type": "Participant",
            "calendarAddress": "mailto:olga@example.com",
            "roles": {"owner": true},
        },
        "p2": {
            "@type": "Participant",
            "calendarAddress": "mailto:vera@example.com",
        },
    }));
    let icalendar = drawn(&current).replace(
        "END:VEVENT",
        "ATTENDEE;CN=Nils;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:nils@example.com\r\nEND:VEVENT",
    );

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(patch.len(), 1, "{patch:?}");
    let (path, guest) = patch.iter().next().unwrap();
    let key = path
        .strip_prefix("participants/")
        .unwrap_or_else(|| panic!("an entry added into the map, not {path}"));
    assert!(!["p1", "p2"].contains(&key), "{path} overwrites a guest");
    assert!(!key.contains('/'), "{path}");
    assert_eq!(
        guest,
        &json!({
            "@type": "Participant",
            "name": "Nils",
            "calendarAddress": "mailto:nils@example.com",
            "roles": {"attendee": true},
            "participationStatus": "needs-action",
            "expectReply": true,
        })
    );
}

#[test]
fn a_guest_removed_in_evolution_is_removed_from_the_server() {
    let current = normalised_meeting();
    let icalendar = drawn(&current).replace(
        "ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:mailto:vera@example.com\r\n",
        "",
    );
    assert!(!icalendar.contains("vera"), "{icalendar}");

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(patch, patch_of(&[("participants/v", Value::Null)]));
}

#[test]
fn the_first_guests_write_the_guest_list_and_its_organizer_whole() {
    let mut current = meeting(Value::Null);
    current.organizer_calendar_address = None;
    let icalendar = drawn(&current).replace(
        "END:VEVENT",
        "ORGANIZER;CN=Olga:mailto:olga@example.com\r\n\
         ATTENDEE;CN=Olga;ROLE=CHAIR;PARTSTAT=ACCEPTED:mailto:olga@example.com\r\n\
         ATTENDEE;CN=Vera;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION:mailto:vera@example.com\r\n\
         END:VEVENT",
    );
    let edited = saved(&icalendar);

    let patch = diff(&current, &edited);

    assert_eq!(
        patch,
        patch_of(&[
            (
                "participants",
                serde_json::to_value(edited.participants.as_ref().unwrap()).unwrap()
            ),
            ("organizerCalendarAddress", json!("mailto:olga@example.com")),
        ])
    );
}

#[test]
fn removing_every_guest_turns_the_meeting_back_into_an_appointment() {
    let current = normalised_meeting();
    let icalendar: String = drawn(&current)
        .split_inclusive("\r\n")
        .filter(|line| !line.starts_with("ORGANIZER") && !line.starts_with("ATTENDEE"))
        .collect();

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(
        patch,
        patch_of(&[
            ("participants", Value::Null),
            ("organizerCalendarAddress", Value::Null),
        ])
    );
}

#[test]
fn a_participant_no_line_could_show_outlives_the_guests_removed_beside_it() {
    // Reachable only through a web form: no CAL-ADDRESS names it, so it was
    // never on the component and its absence there is not a deletion.
    let mut current = normalised_meeting();
    current.participants.as_mut().unwrap().insert(
        "w".to_owned(),
        json!({
            "@type": "Participant",
            "name": "Room booking",
            "sendTo": {"web": "https://rooms.example.com/book"},
        }),
    );
    let icalendar = drawn(&current).replace(
        "ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:mailto:vera@example.com\r\n",
        "",
    );
    assert!(!icalendar.contains("rooms.example.com"), "{icalendar}");

    let patch = diff(&current, &saved(&icalendar));

    assert_eq!(patch, patch_of(&[("participants/v", Value::Null)]));
}

#[test]
fn a_reply_saved_through_the_backend_keeps_what_the_line_has_no_room_for() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Planning", "2026-01-15T13:00:00");
    let guests = normalised_meeting().participants.unwrap();
    fixture.patch(
        &id,
        json!({
            "participants": guests,
            "organizerCalendarAddress": "mailto:olga@example.com",
        }),
    );
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replace("\r\n ", "");
    assert!(
        icalendar.contains("ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:mailto:vera@example.com"),
        "{icalendar}"
    );
    let edited = icalendar.replace(
        "ATTENDEE;CN=Vera;PARTSTAT=NEEDS-ACTION:",
        "ATTENDEE;CN=Vera;PARTSTAT=DECLINED:",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id).participants.unwrap();
    let mut vera = guests["v"].clone();
    vera["participationStatus"] = json!("declined");
    assert_eq!(stored["v"], vera);
    assert_eq!(stored["o"], guests["o"]);
}
