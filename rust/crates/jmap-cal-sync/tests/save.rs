// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. A save must not destroy data that the component cannot carry. The
//! mapping drops some properties of a JSCalendar event. So a save that replaces
//! properties whole deletes data that the user never touched and cannot see.

mod common;

use common::Fixture;
use jmap_cal_sync::{Base, Itip, OnConflict, SyncError, Unsendable};
use jmap_mock::MockServer;
use jmap_proto::calendars::NDay;
use serde_json::{Value, json};

/// The component that Evolution gives to `save_component_sync` for a new appointment. The
/// local cache invented the `UID`; it is not a server identifier.
const NEW_EVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT90M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn saving_a_new_event_files_it_in_this_calendar_under_a_server_identifier() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let saved = sync.save_component(NEW_EVENT, None).unwrap();

    assert_ne!(
        saved.uid, "20260808T101500Z-4711-1000-1-0@localhost",
        "the locally invented UID must not be sent as the JMAP id"
    );
    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.title.as_deref(), Some("Planning"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(stored.duration.as_deref(), Some("PT90M"));
    assert!(
        stored
            .calendar_ids
            .as_ref()
            .unwrap()
            .contains_key(&fixture.ours),
        "filed in the calendar being synced"
    );
    // The listing agrees with the result of the save.
    let (_, events) = sync.list_existing().unwrap();
    assert_eq!(events, vec![saved]);
}

#[test]
fn a_new_events_icalendar_uid_becomes_the_jscalendar_uid() {
    let fixture = Fixture::start();

    let saved = fixture.sync().save_component(NEW_EVENT, None).unwrap();

    assert_eq!(
        fixture.event(&saved.uid.as_str().into()).uid.as_deref(),
        Some("20260808T101500Z-4711-1000-1-0@localhost"),
        "the identity other iTIP clients know the event by must survive the create"
    );
}

#[test]
fn a_new_event_that_states_its_end_rather_than_its_length_still_has_one() {
    // Evolution's appointment editor writes DTEND, never DURATION. Such an event reached
    // the server with no duration (RFC 8984's P0D), so a 90-minute meeting became a
    // zero-length one.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace("DURATION:PT90M", "DTEND;TZID=Europe/Berlin:20260115T143000");

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT1H30M"));
    // EDS receives the same length, in the one form that this mapping writes.
    assert!(
        saved.icalendar.contains("\r\nDURATION:PT1H30M\r\n"),
        "{saved:?}"
    );
}

#[test]
fn a_new_all_day_event_reaches_the_server_as_one() {
    // Evolution writes an all-day appointment as DTSTART and DTEND DATE values. Without
    // showWithoutTime, the server and every client that reads it saw a midnight
    // appointment.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT
        .replace(
            "DTSTART;TZID=Europe/Berlin:20260115T130000",
            "DTSTART;VALUE=DATE:20260115",
        )
        .replace("DURATION:PT90M", "DTEND;VALUE=DATE:20260116");

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.show_without_time, Some(true));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T00:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("P1D"));
    // RFC 5545 §3.2.19 and RFC 8984 §4.1.5 agree: a day has no zone.
    assert_eq!(stored.time_zone, None);
    // EDS receives the same event, still without a time.
    assert!(
        saved
            .icalendar
            .contains("\r\nDTSTART;VALUE=DATE:20260115\r\n"),
        "{saved:?}"
    );
}

#[test]
fn giving_an_all_day_event_a_time_clears_the_flag_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Retreat", "2026-01-15T00:00:00");
    // A day has no zone (RFC 8984 §4.1.5). `CalendarEvent::simple` seeds one, so the
    // all-day patch removes it.
    fixture.patch(
        &id,
        json!({"showWithoutTime": true, "duration": "P1D", "timeZone": null}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("DTSTART;VALUE=DATE:20260115"),
        "{icalendar}"
    );
    let edited = icalendar
        .replace("DTSTART;VALUE=DATE:20260115", "DTSTART:20260115T090000")
        .replace("DURATION:P1D", "DURATION:PT2H");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.show_without_time, None,
        "the day the user turned into an appointment is still a day on the server"
    );
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H"));
}

#[test]
fn an_all_day_event_the_component_could_not_say_is_all_day_keeps_its_flag() {
    // A server can set showWithoutTime on an event that no DATE value can hold: one at
    // 09:00 or, as here, one with a zone. The component shows it as timed and loses the
    // flag. The save must not read that loss as the user clearing the flag.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Retreat", "2026-01-15T00:00:00");
    fixture.patch(
        &id,
        json!({"showWithoutTime": true, "duration": "P1D", "timeZone": "Europe/Berlin"}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("DTSTART;TZID=Europe/Berlin:20260115T000000"),
        "{icalendar}"
    );
    let edited = icalendar.replace("SUMMARY:Retreat", "SUMMARY:Retreat (offsite)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Retreat (offsite)"));
    assert_eq!(
        stored.show_without_time,
        Some(true),
        "a flag the component never showed cannot have been unset by the user"
    );
}

#[test]
fn a_save_leaves_the_servers_own_timestamps_alone() {
    // `created` and `updated` (RFC 8984 §4.1.5, §4.1.6) are the server's record of the
    // event. The component shows them as CREATED, and as DTSTAMP and LAST-MODIFIED (one
    // instant, RFC 5545 §3.8.7.2). The save never reads them back. An editor rewrites all
    // three from its own clock on each save, so a read reports the client's time as the
    // server's last change.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({"created": "2026-01-02T09:30:00Z", "updated": "2026-01-15T17:45:01Z"}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("CREATED:20260102T093000Z"),
        "{icalendar}"
    );
    assert!(
        icalendar.contains("DTSTAMP:20260115T174501Z"),
        "{icalendar}"
    );
    let edited = icalendar
        .replace("CREATED:20260102T093000Z", "CREATED:20260210T080000Z")
        .replace("DTSTAMP:20260115T174501Z", "DTSTAMP:20260210T080000Z")
        .replace(
            "LAST-MODIFIED:20260115T174501Z",
            "LAST-MODIFIED:20260210T080000Z",
        )
        .replace("SUMMARY:Planning", "SUMMARY:Planning (moved)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Planning (moved)"));
    assert_eq!(stored.created.as_deref(), Some("2026-01-02T09:30:00Z"));
    assert_eq!(stored.updated.as_deref(), Some("2026-01-15T17:45:01Z"));
}

#[test]
fn editing_an_event_leaves_unmapped_properties_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // `sequence` is a property that no component from this mapping can carry. (Earlier
    // examples were `locations`, `keywords`, `freeBusyStatus`, `priority` and
    // `useDefaultAlerts`; all are now mapped.)
    //
    // The guest list is drawn (the ATTENDEE below) and patched one member at a time. A
    // save that does not touch it must keep the `sendTo` that the line cannot hold, and
    // all other members.
    let guest = json!({
        "@type": "Participant",
        "name": "Vera Example",
        "sendTo": {"imip": "mailto:vera@example.com"},
        "roles": {"attendee": true},
        "participationStatus": "accepted",
    });
    // The online meeting is drawn as the CONFERENCE below and is editable, but only one
    // member at a time. The line cannot hold the `description`, so it survives a save
    // like an unmapped property. Nothing about it changed here, so the whole entry must
    // stay unchanged.
    let online = json!({
        "@type": "VirtualLocation",
        "name": "Team room",
        "description": "Ask Vera for the passcode",
        "uri": "https://meet.example.com/standup",
        "features": {"video": true},
    });
    // The link is the same case: the ATTACH below is editable, but only its address. The
    // line cannot hold a Link's `title` (RFC 8984 §1.4.11). The media type and size are
    // the server's own description of the resource. Nothing about it changed here, so the
    // whole entry must stay unchanged.
    let agenda = json!({
        "@type": "Link",
        "href": "https://files.example.com/standup.pdf",
        "contentType": "application/pdf",
        "size": 51_200,
        "title": "What we said we would do",
    });
    fixture.patch(
        &id,
        json!({
            "participants": {"p1": guest.clone()},
            "virtualLocations": {"v1": online.clone()},
            "links": {"l1": agenda.clone()},
            "sequence": 3,
        }),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    // Unfold first: RFC 5545 §3.1 splits a content line longer than 75 octets, and this
    // line is longer.
    assert!(
        icalendar
            .replace("\r\n ", "")
            .contains("ATTENDEE;CN=\"Vera Example\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:vera@example.com"),
        "{icalendar}"
    );
    assert!(
        icalendar.replace("\r\n ", "").contains(
            "CONFERENCE;VALUE=URI;FEATURE=VIDEO;LABEL=\"Team room\";X-JMAP-KEY=v1:\
             https://meet.example.com/standup"
        ),
        "{icalendar}"
    );
    assert!(
        icalendar.replace("\r\n ", "").contains(
            "ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:\
             https://files.example.com/standup.pdf"
        ),
        "{icalendar}"
    );
    assert!(!icalendar.contains("passcode"), "{icalendar}");
    assert!(
        !icalendar.contains("What we said we would do"),
        "{icalendar}"
    );
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.extra.get("sequence"),
        Some(&json!(3)),
        "an unmapped property was overwritten"
    );
    assert_eq!(
        stored
            .participants
            .as_ref()
            .and_then(|guests| guests.get("p1")),
        Some(&guest),
        "the guest list was rewritten by a save that only changed the title"
    );
    assert_eq!(
        stored
            .virtual_locations
            .as_ref()
            .and_then(|places| places.get("v1")),
        Some(&online),
        "the conference link was rewritten by a save that only changed the title"
    );
    assert_eq!(
        stored.links.as_ref().and_then(|links| links.get("l1")),
        Some(&agenda),
        "the agenda was rewritten by a save that only changed the title"
    );
}

/// A component with its folds removed. RFC 5545 §3.1 splits a content line longer than 75
/// octets, and a `CONFERENCE` with a label and a key is longer. So a text substitution
/// must name the full line, not the first fragment that the emitter wrote.
fn unfolded(icalendar: &str) -> String {
    icalendar.replace("\r\n ", "")
}

/// An event with an online meeting, and the sync that serves it.
fn joined_online(fixture: &Fixture, places: Value) -> (jmap_proto::Id, jmap_cal_sync::CalSync) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"virtualLocations": places}));
    let sync = fixture.sync();
    (id, sync)
}

/// The start virtual location for the tests below: a link, a name, the ways to take
/// part, and a note that the `CONFERENCE` line cannot hold.
fn team_room() -> Value {
    json!({
        "@type": "VirtualLocation",
        "name": "Team room",
        "description": "Ask Vera for the passcode",
        "uri": "https://meet.example.com/standup",
        "features": {"video": true},
    })
}

#[test]
fn moving_where_an_event_is_joined_online_patches_the_link_in_place() {
    // The second property that a save patches into and does not replace, like
    // `locations`: a VirtualLocation holds more than its line shows. A patch that names
    // `virtualLocations` replaces the entry and deletes the description. So the save
    // names only the member that the user edited.
    let fixture = Fixture::start();
    let (id, sync) = joined_online(&fixture, json!({"v1": team_room()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace(
        "https://meet.example.com/standup",
        "https://meet.example.com/standup-2",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let mut expected = team_room();
    expected["uri"] = json!("https://meet.example.com/standup-2");
    assert_eq!(
        stored.virtual_locations.as_ref().unwrap()["v1"],
        expected,
        "only the link the user edited may change"
    );
}

#[test]
fn renaming_a_conference_and_changing_how_it_is_joined_both_arrive() {
    // The other two members that a CONFERENCE shows: RFC 7986 §6.4's LABEL is the `name`,
    // and §6.3's FEATURE is the `features`. The save replaces the set whole. This is safe
    // because `maps_virtual_locations` already refuses a set that the line cannot draw in
    // full.
    let fixture = Fixture::start();
    let (id, sync) = joined_online(&fixture, json!({"v1": team_room()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar
        .replace("FEATURE=VIDEO", "FEATURE=AUDIO,PHONE")
        .replace("LABEL=\"Team room\"", "LABEL=\"Phone bridge\"");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let mut expected = team_room();
    expected["name"] = json!("Phone bridge");
    expected["features"] = json!({"audio": true, "phone": true});
    assert_eq!(stored.virtual_locations.as_ref().unwrap()["v1"], expected);
}

#[test]
fn clearing_a_conferences_name_removes_it_rather_than_naming_it_nothing() {
    // A deleted LABEL becomes `"virtualLocations/v1/name": null`. RFC 8620 §5.3 removes a
    // property to mean "the default", and RFC 8984 §4.2.6 makes the empty string the
    // default `name`. To store "" claims a place whose name is nothing, which is
    // different.
    let fixture = Fixture::start();
    let (id, sync) = joined_online(&fixture, json!({"v1": team_room()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace(";LABEL=\"Team room\"", "");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let mut expected = team_room();
    expected.as_object_mut().unwrap().remove("name");
    assert_eq!(stored.virtual_locations.as_ref().unwrap()["v1"], expected);
}

#[test]
fn a_conference_missing_from_the_component_is_left_where_it_is() {
    // The conservative half of this mapping: a missing line does not tell who removed it.
    // Evolution 3.52 has no UI for a conference. Only a real Evolution can show if its
    // editor writes back a property that it does not know. To delete the server's entry
    // can destroy a link that the user never touched. So the save ignores the conferences
    // that it does not name.
    let fixture = Fixture::start();
    let (id, sync) = joined_online(
        &fixture,
        json!({"v1": team_room(), "v2": {
            "@type": "VirtualLocation",
            "uri": "tel:+1-555-0100",
        }}),
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited: String = icalendar
        .split_inclusive("\r\n")
        .filter(|line| !line.contains("tel:+1-555-0100"))
        .collect();
    assert_ne!(edited, icalendar, "the line to drop was not found");
    sync.save_component(
        &edited.replace("SUMMARY:Standup", "SUMMARY:Standup (short)"),
        Some(id.as_str()),
    )
    .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.virtual_locations.as_ref().unwrap()["v2"],
        json!({"@type": "VirtualLocation", "uri": "tel:+1-555-0100"}),
        "a conference the component stopped naming must not be deleted"
    );
}

#[test]
fn a_conference_the_server_does_not_hold_is_not_created_by_a_save() {
    // The other half of the same caution, which RFC 8620 §5.3 also requires: a patch can
    // only go through objects that already exist. A line with a key that the server never
    // chose, or with no key (as from any other client), names no entry for this save. The
    // create path, which writes the whole property, handles it.
    let fixture = Fixture::start();
    let (id, sync) = joined_online(&fixture, json!({"v1": team_room()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace(
        "END:VEVENT",
        "CONFERENCE;VALUE=URI;X-JMAP-KEY=v9:tel:+1-555-0100\r\n\
         CONFERENCE;VALUE=URI:https://meet.example.com/other\r\n\
         END:VEVENT",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored
            .virtual_locations
            .as_ref()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["v1"],
        "a save must not file a conference the server has no entry for"
    );
}

#[test]
fn an_event_whose_conference_is_shown_in_part_is_not_patched_at_all() {
    // Each conditionally-mapped property has this rule: the user cannot have edited a
    // drawing that omitted something. Here a way to take part is outside RFC 7986 §6.3,
    // and the CONFERENCE line cannot hold it. So the save leaves the whole property
    // alone, also the entry whose URI the save changed.
    let fixture = Fixture::start();
    let mut hologram = team_room();
    hologram["features"] = json!({"hologram": true});
    let (id, sync) = joined_online(&fixture, json!({"v1": hologram.clone()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar
        .replace("meet.example.com/standup", "meet.example.com/standup-2")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.virtual_locations.as_ref().unwrap()["v1"],
        hologram,
        "a property shown in part must not be written back"
    );
}

#[test]
fn a_new_events_conference_reaches_the_server() {
    // The create path writes the whole property, because no server entry exists to patch.
    // So a component that already names an online meeting (for example, an event copied
    // from another calendar) keeps it.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "END:VEVENT",
        "CONFERENCE;VALUE=URI;FEATURE=VIDEO;LABEL=Team room:https://meet.example.com/planning\r\n\
         END:VEVENT",
    );

    let saved = fixture
        .sync()
        .save_component(&icalendar, None)
        .expect("create");

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(
        stored.virtual_locations,
        serde_json::from_value(json!({"v1": {
            "@type": "VirtualLocation",
            "uri": "https://meet.example.com/planning",
            "name": "Team room",
            "features": {"video": true},
        }}))
        .expect("a map of virtual locations")
    );
}

/// An event that points at the given external resources, and the sync that serves it.
fn points_at(fixture: &Fixture, links: Value) -> (jmap_proto::Id, jmap_cal_sync::CalSync) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"links": links}));
    (id, fixture.sync())
}

/// The start link for the tests below: an address, its server-known media type and
/// size, and two members that no `ATTACH` line can hold.
fn agenda() -> Value {
    json!({
        "@type": "Link",
        "href": "https://files.example.com/standup.pdf",
        "contentType": "application/pdf",
        "size": 51_200,
        "cid": "agenda@example.com",
        "title": "What we said we would do",
    })
}

#[test]
fn moving_an_attachment_patches_the_entry_the_server_chose() {
    // The third property that a save patches into and does not replace. A Link (RFC 8984
    // §1.4.11) holds a `cid`, a `rel` and a `title` that no ATTACH line can hold. A patch
    // that names `links` deletes half of a resource that the user never saw. So the save
    // names `links/<key>/href` under the drawn key, and all else stays.
    let fixture = Fixture::start();
    let (id, sync) = points_at(&fixture, json!({"l1": agenda()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace("standup.pdf", "standup-final.pdf");
    assert_ne!(edited, icalendar, "the line to edit was not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let mut expected = agenda();
    expected["href"] = json!("https://files.example.com/standup-final.pdf");
    assert_eq!(
        stored.links.as_ref().unwrap()["l1"],
        expected,
        "only the address the user edited may change"
    );
}

#[test]
fn the_media_type_and_size_the_server_stated_are_not_rewritten_by_a_save() {
    // A save names only the `href` of a Link. `contentType` and `size` are the server's
    // description of the resource (§1.4.11 calls the size an estimate), not fields that
    // the user saw. An editor usually rewrites the line without these parameters. To read
    // that as a cleared media type deletes server data on the first unrelated edit.
    let fixture = Fixture::start();
    let (id, sync) = points_at(&fixture, json!({"l1": agenda()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar
        .replace(";FMTTYPE=application/pdf", "")
        .replace(";SIZE=51200", "")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.links.as_ref().unwrap()["l1"],
        agenda(),
        "a save must not rewrite what the server knows about a resource"
    );
}

#[test]
fn an_attachment_missing_from_the_component_is_left_where_it_is() {
    // A missing ATTACH gets the same caution as a missing CONFERENCE: a missing line does
    // not tell who removed it. Evolution's editor keeps attachments in its own store and
    // writes what it kept. So a missing line can come from an editor that had no URI, or
    // from a user who removed the file. To read it as a deletion can destroy a document
    // that nobody touched. The cost: a removal made elsewhere returns on the next sync.
    let fixture = Fixture::start();
    let minutes = json!({
        "@type": "Link",
        "href": "https://files.example.com/minutes.txt",
    });
    let (id, sync) = points_at(&fixture, json!({"l1": agenda(), "l2": minutes.clone()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited: String = icalendar
        .split_inclusive("\r\n")
        .filter(|line| !line.contains("minutes.txt"))
        .collect();
    assert_ne!(edited, icalendar, "the line to drop was not found");
    sync.save_component(
        &edited.replace("SUMMARY:Standup", "SUMMARY:Standup (short)"),
        Some(id.as_str()),
    )
    .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.links.as_ref().unwrap()["l2"],
        minutes,
        "a resource the component stopped naming must not be deleted"
    );
}

#[test]
fn a_resource_the_server_does_not_hold_is_not_created_by_a_save() {
    // RFC 8620 §5.3 lets a patch go only through objects that already exist. A line with
    // a key that the server never chose, or with no key (as from another client), names
    // no entry to patch. A create is not possible either: a new resource is a file to
    // upload as a blob, not an address for the server to fetch.
    let fixture = Fixture::start();
    let (id, sync) = points_at(&fixture, json!({"l1": agenda()}));

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace(
        "END:VEVENT",
        "ATTACH;X-JMAP-KEY=l9:https://files.example.com/minutes.txt\r\n\
         ATTACH:https://files.example.com/slides.pdf\r\n\
         END:VEVENT",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.links.as_ref().unwrap().keys().collect::<Vec<_>>(),
        ["l1"],
        "a save must not file a resource the server has no entry for"
    );
}

#[test]
fn an_attachment_under_a_key_this_side_invented_is_not_patched_onto_another() {
    // A key outside RFC 8984 §1.4.4's `Id` grammar cannot go on the line, so that entry
    // reads back under an invented key. An invented key avoids only the keys that the
    // document names. So it can collide with a server key for an entry that the drawing
    // omitted. Here `k1` is a resource with no address to draw. To patch `links/k1/href`
    // gives that entry the agenda's address and loses the edit. So the save compares the
    // server's address with the drawn address, and drops an edit under an invented key.
    let fixture = Fixture::start();
    let addressless = json!({
        "@type": "Link",
        "title": "Whatever it was we could not link to",
    });
    let drawn = json!({
        "@type": "Link",
        "href": "https://files.example.com/standup.pdf",
        "title": "What we said we would do",
    });
    let (id, sync) = points_at(
        &fixture,
        json!({"k1": addressless.clone(), "an/agenda": drawn.clone()}),
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar
        .replace("standup.pdf", "standup-final.pdf")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    assert_ne!(edited, icalendar, "the line to edit was not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    let links = stored.links.as_ref().unwrap();
    assert_eq!(
        links["k1"], addressless,
        "another entry was patched instead"
    );
    assert_eq!(
        links["an/agenda"], drawn,
        "an edit under a key this side invented must be dropped, not guessed at"
    );
}

#[test]
fn editing_one_of_several_attachments_preserves_other_attachments_and_images() {
    let fixture = Fixture::start();
    let image = json!({
        "@type": "Link",
        "href": "https://files.example.com/badge.png",
        "rel": "icon",
        "display": "badge",
        "title": "Badge Icon",
    });
    let slides = json!({
        "@type": "Link",
        "href": "https://files.example.com/slides.pdf",
        "contentType": "application/pdf",
        "size": 102_400,
        "title": "Presentation Slides",
    });
    let (id, sync) = points_at(
        &fixture,
        json!({
            "l1": agenda(),
            "l2": slides.clone(),
            "img1": image.clone(),
        }),
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace("standup.pdf", "standup-v2.pdf");
    assert_ne!(edited, icalendar, "line to edit not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let links = stored.links.as_ref().unwrap();
    assert_eq!(links.len(), 3, "attachment count changed: {links:?}");
    let mut expected_agenda = agenda();
    expected_agenda["href"] = json!("https://files.example.com/standup-v2.pdf");
    assert_eq!(links["l1"], expected_agenda);
    assert_eq!(links["l2"], slides);
    assert_eq!(links["img1"], image);
}

#[test]
fn editing_unrelated_field_preserves_all_multiple_attachments_and_images() {
    let fixture = Fixture::start();
    let image = json!({
        "@type": "Link",
        "href": "https://files.example.com/badge.png",
        "rel": "icon",
        "display": "badge",
    });
    let slides = json!({
        "@type": "Link",
        "href": "https://files.example.com/slides.pdf",
        "contentType": "application/pdf",
    });
    let (id, sync) = points_at(
        &fixture,
        json!({
            "l1": agenda(),
            "l2": slides.clone(),
            "img1": image.clone(),
        }),
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Sprint Planning Meeting");
    assert_ne!(edited, icalendar, "summary not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Sprint Planning Meeting"));
    let links = stored.links.as_ref().unwrap();
    assert_eq!(
        links.len(),
        3,
        "links map modified on unrelated edit: {links:?}"
    );
    assert_eq!(links["l1"], agenda());
    assert_eq!(links["l2"], slides);
    assert_eq!(links["img1"], image);
}

#[test]
fn readdressing_an_image_preserves_icon_rel_and_display() {
    let fixture = Fixture::start();
    let image = json!({
        "@type": "Link",
        "href": "https://files.example.com/badge.png",
        "rel": "icon",
        "display": "badge",
        "title": "Badge Icon",
    });
    let (id, sync) = points_at(
        &fixture,
        json!({
            "l1": agenda(),
            "img1": image.clone(),
        }),
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    let edited = icalendar.replace("badge.png", "new-badge.png");
    assert_ne!(edited, icalendar, "image line not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let links = stored.links.as_ref().unwrap();
    assert_eq!(links.len(), 2, "links count changed: {links:?}");
    assert_eq!(links["l1"], agenda());
    let mut expected_image = image;
    expected_image["href"] = json!("https://files.example.com/new-badge.png");
    assert_eq!(links["img1"], expected_image);
}

#[test]
fn a_new_events_attachment_reaches_the_server() {
    // The create path writes the whole property, because no server entry exists to patch.
    // So a component that already points at a resource (for example, an event copied from
    // another calendar) keeps it.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "END:VEVENT",
        "ATTACH;FMTTYPE=application/pdf;SIZE=51200:https://files.example.com/planning.pdf\r\n\
         END:VEVENT",
    );

    let saved = fixture
        .sync()
        .save_component(&icalendar, None)
        .expect("create");

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(
        stored.links,
        serde_json::from_value(json!({"k1": {
            "@type": "Link",
            "href": "https://files.example.com/planning.pdf",
            "contentType": "application/pdf",
            "size": 51_200,
        }}))
        .expect("a map of links")
    );
}

#[test]
fn a_new_events_local_attachment_is_not_sent_to_the_server() {
    // Evolution keeps an attached file in its own store, as a `file:` URI. Nobody else
    // can fetch that address, and the path names the user's home directory. So the save
    // does not file it as a Link, and creates the event without it. To send the file
    // needs a blob upload, which this backend does not do yet.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "END:VEVENT",
        "ATTACH:file:///home/vera/.local/share/evolution/calendar/planning.pdf\r\n\
         END:VEVENT",
    );

    let saved = fixture
        .sync()
        .save_component(&icalendar, None)
        .expect("create");

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.links, None);
}

#[test]
fn a_server_file_link_is_not_loaded_and_an_edit_leaves_it_on_the_server() {
    // Evolution and EDS read a `file:` path on the local machine, so the server's link
    // never reaches the component. The server keeps its entry through an edit.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let links = json!({
        "zero": {"@type": "Link", "href": "file:///dev/zero"},
        "doc": {"@type": "Link", "href": "https://files.example.com/agenda.pdf"},
    });
    fixture.patch(&id, json!({"links": links}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        !icalendar.to_ascii_lowercase().contains("file:"),
        "{icalendar}"
    );
    assert!(
        icalendar.contains("https://files.example.com/agenda.pdf"),
        "{icalendar}"
    );
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Planning");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Planning"));
    assert_eq!(
        stored.links,
        serde_json::from_value(links).expect("a map of links")
    );
}

#[test]
fn rescheduling_an_event_moves_the_start_and_keeps_its_zone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("TZID=Europe/Berlin"), "{icalendar}");
    let edited = icalendar.replace("20260115T090000", "20260115T093000");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:30:00"));
    assert_eq!(
        stored.time_zone.as_deref(),
        Some("Europe/Berlin"),
        "moving the start within its zone must not restate the zone"
    );
}

#[test]
fn moving_an_event_to_another_zone_lengthening_it_and_unconfirming_it_all_arrive() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("TZID=Europe/Berlin", "TZID=America/New_York")
        .replace("DURATION:PT1H", "DURATION:PT2H")
        .replace("STATUS:CONFIRMED", "STATUS:TENTATIVE");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H"));
    assert_eq!(stored.status.as_deref(), Some("tentative"));
    assert_eq!(
        stored.start.as_deref(),
        Some("2026-01-15T09:00:00"),
        "the wall-clock time was not what the user changed"
    );
}

#[test]
fn a_length_the_component_could_not_state_survives_a_save() {
    // The server's `duration` can be a value that iCalendar cannot hold. RFC 5545 §3.3.6
    // spells a negative length and RFC 8984 §1.4.6 does not, so the mapping refuses to
    // write this value. If libical refused the line, the whole component and the
    // appointment would be lost. The save must not clear the value. The baseline is the
    // server's event through the same rendering, so both sides omit the length and the
    // server keeps it.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"duration": "-PT1H"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("DURATION"), "{icalendar}");
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Daily standup"));
    assert_eq!(
        stored.duration.as_deref(),
        Some("-PT1H"),
        "a length the component could not state is not a length the user cleared"
    );
}

#[test]
fn an_occurrence_whose_length_the_component_could_not_state_is_left_alone() {
    // The same rule one level down, where the save replaces the property whole and does
    // not patch it key by key. An override that the component cannot describe reads back
    // as the empty patch. So to write the map deletes the server's length. For the same
    // reason, a rule with a `byDay` leaves `recurrenceRule` unchanged.
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-20T09:00:00": {"duration": "-PT1H"}}}),
    );
    let sync = fixture.sync();

    // An RDATE places the occurrence at the series' length: the occurrence shows, but its
    // real length does not.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("RDATE:20260120T090000Z"), "{icalendar}");
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-20T09:00:00".to_owned(),
                json!({"duration": "-PT1H"})
            )]
            .into()
        ),
        "the occurrence the mapping saw in part must not be rewritten from that view",
    );
}

#[test]
fn a_recurrence_the_mapping_can_carry_is_patched() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "daily", "count": 10}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=DAILY;COUNT=10"),
        "{icalendar}"
    );
    let edited = icalendar.replace("COUNT=10", "COUNT=5");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.frequency, "daily");
    assert_eq!(rules.count, Some(5));
}

#[test]
fn a_recurrence_the_mapping_cannot_carry_is_left_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // The RecurrenceRule that this crate models has no `rscale`, and libical reads no
    // `RRULE` for a Chinese-calendar rule. So the RRULE that the user edited is narrower
    // than the server's rule.
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "rscale": "chinese",
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;COUNT=4");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert!(
        rules.rscale.as_deref() == Some("chinese")
            || rules.extra.get("rscale") == Some(&json!("chinese")),
        "a rule part the RRULE could not carry was dropped"
    );
    assert_eq!(
        rules.count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
}

#[test]
fn a_series_end_restated_as_a_utc_instant_does_not_move_the_recurrence() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-08-10T09:00:00");
    fixture.patch(
        &id,
        json!({
            "timeZone": "Europe/Zurich",
            "recurrenceRule":{
                "@type": "RecurrenceRule",
                "frequency": "daily",
                "until": "2026-09-01T09:00:00",
            },
        }),
    );
    let sync = fixture.sync();

    // RFC 8984 §4.3.3's `until` is a local time in the event's zone, so the mapping draws
    // it like DTSTART.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("UNTIL=20260901T090000\r\n")
            || icalendar.contains("UNTIL=20260901T090000;"),
        "{icalendar}"
    );

    // RFC 5545 §3.3.10 requires a UTC instant there when DTSTART names a zone. A
    // conformant editor writes that, and so does an imported event: the same moment, two
    // hours earlier on the clock. As a local time, it ends the series at 07:00 Zurich
    // time. That is before the last occurrence starts, so the server loses a day that the
    // user can still see.
    let edited = icalendar.replace("UNTIL=20260901T090000", "UNTIL=20260901T070000Z");
    assert_ne!(edited, icalendar);
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(
        rules.until.as_deref(),
        Some("2026-09-01T09:00:00"),
        "the end of the series moved on a save that never edited it"
    );
}

#[test]
fn a_new_events_recurrence_reaches_the_server() {
    // An ordinary series. It shows that the refusal below is about the rule, not about
    // recurrence on a create.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "DURATION:PT90M",
        "DURATION:PT90M\r\nRRULE:FREQ=WEEKLY;COUNT=10;BYDAY=TH",
    );

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let rules = fixture
        .event(&saved.uid.as_str().into())
        .recurrence_rule
        .unwrap();
    assert_eq!(rules.frequency, "weekly");
    assert_eq!(rules.count, Some(10));
    assert_eq!(rules.by_day.as_deref(), Some([NDay::new("th")].as_slice()));
}

#[test]
fn a_new_event_whose_series_end_cannot_be_stated_is_not_created_at_all() {
    // RFC 5545 §3.3.10 requires `UNTIL` as a UTC instant when `DTSTART` names a zone, and
    // RFC 8984 §4.3.3 wants a local time. Without a `VTIMEZONE` in the document, the
    // conversion needs a zone database, and `jmap-ical` deliberately carries none. So the
    // value stays as stated, and `maps_recurrence_rule` reports that the rule cannot be
    // sent. This is the most common such rule: every "repeat until <date>" that a
    // conformant editor writes in a zoned calendar.
    //
    // An edit leaves `recurrenceRule` alone, and the server's rule stays (see
    // `a_series_end_restated_as_a_utc_instant_does_not_move_the_recurrence`). A create
    // has no rule to keep, so the sync refuses the save. A strict server can reject the
    // whole set. A lenient one can store an `until` that is no LocalDateTime. This
    // mapping cannot draw that, so Evolution shows one appointment while the server runs
    // a series.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "DURATION:PT90M",
        "DURATION:PT90M\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z",
    );

    let failure = fixture.sync().save_component(&icalendar, None).unwrap_err();

    assert!(
        matches!(failure, SyncError::Unsendable(_)),
        "{failure:?} — the user has to be told, not given a lesser event"
    );
    let (_, events) = fixture.sync().list_existing().unwrap();
    assert!(
        events.is_empty(),
        "a refused create must leave nothing behind: {events:?}"
    );
}

#[test]
fn a_series_end_that_cannot_be_stated_is_refused_by_naming_the_zone_and_the_date() {
    // The refusal above must tell the user what happened. The document's own `VTIMEZONE`
    // now does the conversion (see below), so few cases remain. These are a zone that the
    // entry names but does not define, or defines in a shape that `jmap-ical`'s evaluator
    // does not guess at. A message about "an end date stated as a UTC instant" now
    // describes the case that works. So the refusal names the instant and the zone. These
    // two facts tell the user which appointment to change, and tell a bug reader which
    // zone definition to examine.
    //
    // The refusal carries two facts, not a sentence, because `jmap_backend_cal::ops`
    // writes the translatable sentence from this reason. The instant is the kept value,
    // normalised from the component's `20260331T120000Z`. The user sees this form.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "DURATION:PT90M",
        "DURATION:PT90M\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z",
    );

    let failure = fixture.sync().save_component(&icalendar, None).unwrap_err();

    assert_eq!(
        failure_reason(&failure),
        &Unsendable::RecurrenceEnd {
            until: "2026-03-31T12:00:00Z".to_owned(),
            zone: "Europe/Berlin".to_owned(),
        }
    );
}

#[test]
fn a_zone_defined_in_a_shape_the_evaluator_refuses_is_named_in_the_refusal_too() {
    // The other case, which remains when a document does everything correctly. Berlin is
    // defined, but with a transition rule that `jmap-ical`'s zone evaluator refuses whole
    // (here an `INTERVAL` other than 1). The conversion is not available, and the user
    // gets the same answer. For the user, "not defined" and "defined unreadably" are one
    // problem with the entry's time zone.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT
        .replace(
            "BEGIN:VEVENT",
            &format!(
                "{}BEGIN:VEVENT",
                BERLIN.replace("FREQ=YEARLY", "FREQ=YEARLY;INTERVAL=2")
            ),
        )
        .replace(
            "DURATION:PT90M",
            "DURATION:PT90M\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z",
        );

    let failure = fixture.sync().save_component(&icalendar, None).unwrap_err();

    assert_eq!(
        failure_reason(&failure),
        &Unsendable::RecurrenceEnd {
            until: "2026-03-31T12:00:00Z".to_owned(),
            zone: "Europe/Berlin".to_owned(),
        },
        "a zone the entry defines unreadably is still the zone to name"
    );
}

/// Berlin as libical writes it, and as every component that Evolution saves contains it:
/// the `VTIMEZONE` that defines the `TZID` beside it (RFC 5545 §3.6.5).
const BERLIN: &str = "BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
BEGIN:DAYLIGHT\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
DTSTART:19700329T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
DTSTART:19701025T030000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n";

#[test]
fn a_new_events_series_end_converts_through_the_zone_the_document_defines() {
    // The refusal above applies when a document names a zone but does not define it. A
    // real document defines it. Then the UTC instant that §3.3.10 requires converts to
    // the local time that §4.3.3 wants, with no zone database: the rules are in the file.
    // So the most common recurring save ("repeat weekly until <date>" in a zoned
    // calendar) reaches the server as a series, not as an error.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT
        .replace("BEGIN:VEVENT", &format!("{BERLIN}BEGIN:VEVENT"))
        .replace(
            "DURATION:PT90M",
            "DURATION:PT90M\r\nRRULE:FREQ=WEEKLY;UNTIL=20260331T120000Z",
        );

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let rules = fixture
        .event(&saved.uid.as_str().into())
        .recurrence_rule
        .unwrap();
    // Two hours later, because the last Sunday of March was two days earlier.
    assert_eq!(rules.until.as_deref(), Some("2026-03-31T14:00:00"));
}

#[test]
fn a_new_event_whose_rule_the_rrule_narrowed_is_not_created_at_all() {
    // The other cause of the same refusal, which is not about time zones. JSCalendar
    // states RFC 7529's leap month only beside the `rscale` that this mapping drops. So
    // the outgoing rule names a month that the server can reject.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "DURATION:PT90M",
        "DURATION:PT90M\r\nRRULE:FREQ=YEARLY;BYMONTH=5L",
    );

    let failure = fixture.sync().save_component(&icalendar, None).unwrap_err();

    // The refusal must not name a time zone: this rule has no end. A zone in the message
    // makes the user change something that did not stop the save.
    assert_eq!(
        failure_reason(&failure),
        &Unsendable::Recurrence,
        "a refusal that is not about the zone must not name one"
    );
}

/// The reason that a refused save carries, or a panic that names the other result.
fn failure_reason(failure: &SyncError) -> &Unsendable {
    match failure {
        SyncError::Unsendable(reason) => reason,
        other => panic!("{other:?} — expected a refusal"),
    }
}

#[test]
fn the_days_a_weekly_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "byDay": [{"@type": "NDay", "day": "mo"}],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=WEEKLY;BYDAY=MO"),
        "{icalendar}"
    );
    // Add Thursday, as the recurrence page of the appointment editor does.
    let edited = icalendar.replace("BYDAY=MO", "BYDAY=MO,TH");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(
        rules.by_day.as_deref(),
        Some(&[NDay::new("mo"), NDay::new("th")][..])
    );
}

#[test]
fn the_days_of_the_month_a_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "byMonthDay": [15],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=MONTHLY;BYMONTHDAY=15"),
        "{icalendar}"
    );
    // Move it to the last day of the month, as the editor's recurrence page writes for
    // "on the last day".
    let edited = icalendar.replace("BYMONTHDAY=15", "BYMONTHDAY=-1");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_month_day.as_deref(), Some(&[-1][..]));
}

#[test]
fn the_months_a_yearly_rule_repeats_in_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Tax return", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byMonth": ["3"],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=YEARLY;BYMONTH=3"),
        "{icalendar}"
    );
    // Add the second half-year, as the editor's recurrence page writes for a yearly
    // series in two months.
    let edited = icalendar.replace("BYMONTH=3", "BYMONTH=3,9");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(
        rules.by_month.as_deref(),
        Some(&["3".to_owned(), "9".to_owned()][..])
    );
}

#[test]
fn the_days_of_the_year_a_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "New Year", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byYearDay": [1],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=YEARLY;BYYEARDAY=1"),
        "{icalendar}"
    );
    // Add the last day of the year: the negative form, which RFC 8984 §4.3.3 counts back
    // from 31 December.
    let edited = icalendar.replace("BYYEARDAY=1", "BYYEARDAY=1,-1");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_year_day.as_deref(), Some(&[1, -1][..]));
}

#[test]
fn a_day_of_the_year_the_rrule_should_not_carry_is_not_sent() {
    // RFC 5545 §3.3.10 does not admit `FREQ=MONTHLY;BYYEARDAY=100`: a day of the year is
    // not inside a month. Neither calcard nor libical checks it, so the check occurs on
    // output. The save replaces `recurrenceRule` whole, so one part that the server can
    // reject loses every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "monthly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYYEARDAY=100")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_year_day, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Rent, due"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn a_leap_month_is_not_sent() {
    // RFC 7529's `RSCALE` is the only iCalendar way to name this month (RFC 8984 §4.3.3's
    // `5L`). calcard carries the token without a check, so the check occurs on output.
    // The save replaces `recurrenceRule` whole, so one part that the server can reject
    // loses every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Festival", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "yearly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=YEARLY", "RRULE:FREQ=YEARLY;BYMONTH=5L")
        .replace("SUMMARY:Festival", "SUMMARY:Spring festival");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_month, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Spring festival"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn the_day_a_rules_weeks_start_on_reaches_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint review", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "interval": 2,
            "byDay": [{"@type": "NDay", "day": "tu"}],
            "firstDayOfWeek": "su",
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU"),
        "{icalendar}"
    );
    // Count the weeks from Saturday, as the editor's recurrence page writes when the
    // calendar's week start changes.
    let edited = icalendar.replace("WKST=SU", "WKST=SA");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.first_day_of_week.as_deref(), Some("sa"));
}

#[test]
fn a_day_no_week_starts_on_is_not_sent() {
    // A `firstDayOfWeek` outside RFC 8984 §4.3.3's closed vocabulary has no `WKST` form,
    // and libical refuses a component with `WKST=XX`. So the rule shows without it. The
    // save replaces `recurrenceRule` whole, so a save that did not touch the recurrence
    // must not write it. That write drops the day from the server's rule.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "firstDayOfWeek": "xx",
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=WEEKLY\r\n"),
        "the day is left off the rule the user is shown: {icalendar}"
    );
    // So an edit to the recurrence edits a rule that the user saw only in part. The save
    // drops the edit whole, and does not send the narrower rule.
    let edited = icalendar
        .replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;COUNT=4")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let rules = stored.recurrence_rule.unwrap();
    assert_eq!(
        rules.first_day_of_week.as_deref(),
        Some("xx"),
        "the day the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules.count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Daily standup"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn the_weeks_of_the_year_a_rule_repeats_in_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Payroll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byWeekNo": [1],
            "firstDayOfWeek": "su",
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=YEARLY;BYWEEKNO=1;WKST=SU"),
        "{icalendar}"
    );
    // Add the last week of the year: the negative form, which RFC 8984 §4.3.3 counts back
    // from the end of the year.
    let edited = icalendar.replace("BYWEEKNO=1;", "BYWEEKNO=1,-1;");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_week_no.as_deref(), Some(&[1, -1][..]));
    assert_eq!(
        rules.first_day_of_week.as_deref(),
        Some("su"),
        "the day the weeks are counted from goes with them"
    );
}

#[test]
fn a_week_of_the_year_the_rrule_should_not_carry_is_not_sent() {
    // RFC 5545 §3.3.10 does not admit `FREQ=MONTHLY;BYWEEKNO=20`: it admits `BYWEEKNO`
    // only with `YEARLY`. Neither calcard nor libical checks it, so the check occurs on
    // output. The save replaces `recurrenceRule` whole, so one part that the server can
    // reject loses every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "monthly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYWEEKNO=20")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_week_no, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Rent, due"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn a_week_no_year_has_is_not_cleared_from_the_servers_rule() {
    // A `byWeekNo` outside RFC 5545's `ordwk` fits no `BYWEEKNO` that this mapping writes
    // (no year has week 54). So the rule shows without it. The save replaces
    // `recurrenceRule` whole, so a save that only narrowed the rule must not write it.
    // That write drops the week from the server's rule.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Stocktake", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byWeekNo": [54],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=YEARLY\r\n"),
        "the week is left off the rule the user is shown: {icalendar}"
    );
    // So an edit to the recurrence edits a rule that the user saw only in part. The save
    // drops the edit whole, and does not send the narrower rule.
    let edited = icalendar
        .replace("RRULE:FREQ=YEARLY", "RRULE:FREQ=YEARLY;COUNT=4")
        .replace("SUMMARY:Stocktake", "SUMMARY:Annual stocktake");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let rules = stored.recurrence_rule.unwrap();
    assert_eq!(
        rules.by_week_no.as_deref(),
        Some(&[54][..]),
        "the week the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules.count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Annual stocktake"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn the_occurrence_of_the_set_a_rule_takes_reaches_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Retro", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "byDay": [{"@type": "NDay", "day": "fr"}],
            "bySetPosition": [-1],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=MONTHLY;BYDAY=FR;BYSETPOS=-1"),
        "{icalendar}"
    );
    // Add the first Friday of the month: the positive form, which counts from the start
    // of the set that `BYDAY` expands to.
    let edited = icalendar.replace("BYSETPOS=-1", "BYSETPOS=1,-1");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_set_position.as_deref(), Some(&[1, -1][..]));
    assert_eq!(
        rules.by_day.as_ref().unwrap()[0].day,
        "fr",
        "the days it selects from go with it"
    );
}

#[test]
fn a_position_with_nothing_to_select_from_is_not_sent() {
    // RFC 5545 §3.3.10 does not admit `FREQ=MONTHLY;BYSETPOS=2`: `BYSETPOS` MUST only
    // occur with another `BYxxx` part. libical keeps it without a check, so the check
    // occurs on output. If sent, it names a series whose second (and only) monthly
    // occurrence does not exist.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "monthly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYSETPOS=2")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_set_position, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Rent, due"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn a_position_the_server_holds_alone_is_not_cleared_from_its_rule() {
    // The opposite direction, which loses data. No `RRULE` that this mapping writes can
    // hold a `bySetPosition` with no other `by*` part, so the rule shows without it. The
    // save replaces `recurrenceRule` whole, so a save that only narrowed the rule deletes
    // the position from the server's copy.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Stocktake", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "bySetPosition": [-1],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=YEARLY\r\n"),
        "the position is left off the rule the user is shown: {icalendar}"
    );
    let edited = icalendar
        .replace("RRULE:FREQ=YEARLY", "RRULE:FREQ=YEARLY;COUNT=4")
        .replace("SUMMARY:Stocktake", "SUMMARY:Annual stocktake");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let rules = stored.recurrence_rule.unwrap();
    assert_eq!(
        rules.by_set_position.as_deref(),
        Some(&[-1][..]),
        "the position the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules.count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Annual stocktake"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn the_hours_of_the_day_a_rule_repeats_at_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "daily",
            "byHour": [9],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=DAILY;BYHOUR=9"),
        "{icalendar}"
    );
    // A second standup after lunch. The hours are a set, so the save sends the new hour
    // together with the old one.
    let edited = icalendar.replace("BYHOUR=9", "BYHOUR=9,14");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_hour.as_deref(), Some(&[9, 14][..]));
}

#[test]
fn an_hour_no_day_has_is_not_sent() {
    // RFC 5545 §3.3.10's `hour` excludes 24, and libical drops the whole `RRULE` for such
    // a rule. So the check occurs on output, before the server can keep a rule that no
    // reader can expand.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "daily"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=DAILY", "RRULE:FREQ=DAILY;BYHOUR=24")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_hour, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Daily standup"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn hours_the_server_holds_are_not_cleared_by_a_save_that_narrowed_the_rule() {
    // The direction that loses data. This mapping can write the server's `byHour`. The
    // save replaces `recurrenceRule` whole, so a save that changed the rule for another
    // reason must send `byHour` again.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "daily",
            "byHour": [9, 14],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace(
        "RRULE:FREQ=DAILY;BYHOUR=9,14",
        "RRULE:FREQ=DAILY;BYHOUR=9,14;COUNT=4",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_hour.as_deref(), Some(&[9, 14][..]));
    assert_eq!(rules.count, Some(4), "the edit itself still has to land");
}

#[test]
fn the_minutes_and_seconds_a_rule_repeats_at_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sensor poll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "hourly",
            "byMinute": [0],
            "bySecond": [0],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=HOURLY;BYSECOND=0;BYMINUTE=0"),
        "{icalendar}"
    );
    // A second poll on the half hour. Both parts are sets, so the save sends the new
    // value together with the old one.
    let edited = icalendar.replace("BYMINUTE=0", "BYMINUTE=0,30");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[0][..]));
}

#[test]
fn the_sixtieth_second_is_sent_and_the_sixtieth_minute_is_not() {
    // RFC 5545 §3.3.10's `seconds` goes to 60, but its `minutes` only to 59. libical
    // drops the whole `RRULE` for a rule with minute 60. So the check occurs on output,
    // before the server can keep a rule that no reader can expand. That check must not
    // catch the leap second in the same save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sensor poll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "minutely"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MINUTELY", "RRULE:FREQ=MINUTELY;BYSECOND=60")
        .replace("SUMMARY:Sensor poll", "SUMMARY:Poll on the second");
    sync.save_component(&edited, Some(id.as_str())).unwrap();
    assert_eq!(
        fixture
            .event(&id)
            .recurrence_rule
            .unwrap()
            .by_second
            .as_deref(),
        Some(&[60][..]),
        "the leap second is a value RFC 5545 admits"
    );

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("BYSECOND=60", "BYSECOND=60;BYMINUTE=60")
        .replace("SUMMARY:Poll on the second", "SUMMARY:Sensor poll");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_minute, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Sensor poll"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn minutes_the_server_holds_are_not_cleared_by_a_save_that_narrowed_the_rule() {
    // The direction that loses data. The save replaces `recurrenceRule` whole, so a save
    // that changed the rule for another reason must send the server's minutes and seconds
    // again.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sensor poll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":{
            "@type": "RecurrenceRule",
            "frequency": "hourly",
            "byMinute": [0, 30],
            "bySecond": [15],
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("BYMINUTE=0,30", "BYMINUTE=0,30;COUNT=4");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[15][..]));
    assert_eq!(rules.count, Some(4), "the edit itself still has to land");
}

#[test]
fn a_day_of_the_month_the_rrule_should_not_carry_is_not_sent() {
    // RFC 5545 §3.3.10 does not admit `FREQ=WEEKLY;BYMONTHDAY=15`, and calcard returns it
    // without a check. So, as for an ordinal weekday, the check occurs on output. The
    // save replaces `recurrenceRule` whole, so one part that the server can reject loses
    // every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "weekly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;BYMONTHDAY=15")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_month_day, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Daily standup"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn a_days_ordinal_the_rrule_should_not_carry_is_not_sent() {
    // RFC 5545 §3.3.10 does not admit `FREQ=WEEKLY;BYDAY=2MO`: an ordinal needs a month
    // or a year to count in. calcard returns it without a check, so the check occurs on
    // output, as for the series' `timeZone`. The save replaces `recurrenceRule` whole, so
    // one part that the server can reject loses every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "weekly"}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;BYDAY=2MO")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rule.unwrap().by_day, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Daily standup"),
        "the edit the save could carry still has to land"
    );
}

/// A daily event on the server, so the tests below have a recurrence with single
/// instances to name.
fn seed_daily(fixture: &Fixture) -> jmap_proto::Id {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "daily"}}),
    );
    id
}

/// The component with `line` before its `END:VEVENT`. Evolution does this when the user
/// deletes one occurrence.
fn with_line(icalendar: &str, line: &str) -> String {
    icalendar.replace("END:VEVENT\r\n", &format!("{line}\r\nEND:VEVENT\r\n"))
}

#[test]
fn deleting_one_occurrence_reaches_the_server_as_an_excluded_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some([("2026-01-16T09:00:00".to_owned(), json!({"excluded": true}))].into()),
        "the instance the user deleted is off, and only that one"
    );
    // The series rule stays unchanged.
    assert_eq!(stored.recurrence_rule.unwrap().frequency, "daily");
}

#[test]
fn restoring_a_deleted_occurrence_removes_the_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-16T09:00:00": {"excluded": true}}}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(icalendar.contains("EXDATE:20260116T090000Z"), "{icalendar}");
    let edited: String = icalendar
        .lines()
        .filter(|line| !line.starts_with("EXDATE"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    // A PatchObject removes a property to mean "the default", which for
    // recurrenceOverrides is no named instances.
    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_edited_on_its_own_survives_an_edit_to_another_instance() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    // An override that changes the instance is a separate VEVENT in the component. A
    // deletion of a different occurrence must keep it.
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {
            "2026-01-20T09:00:00": {"title": "Standup with the board"},
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = with_line(&icalendar, "EXDATE:20260116T090000Z");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [
                ("2026-01-16T09:00:00".to_owned(), json!({"excluded": true})),
                (
                    "2026-01-20T09:00:00".to_owned(),
                    json!({"title": "Standup with the board"}),
                )
            ]
            .into()
        ),
    );
}

/// The component with `vevent` added inside its envelope. Evolution does this when the
/// user edits one occurrence: a second instance of the same uid, with the `RECURRENCE-ID`
/// of the day it replaces.
fn with_instance(icalendar: &str, vevent: &str) -> String {
    icalendar.replace("END:VCALENDAR\r\n", &format!("{vevent}END:VCALENDAR\r\n"))
}

#[test]
fn editing_one_occurrence_reaches_the_server_as_a_patched_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    // A detached instance is a full component, not a patch: Evolution copies the series
    // and edits the copy. What it repeats unchanged (here the status, and the length as a
    // DTEND) is not an edit. Only the new start and title reach the server.
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART:20260116T100000Z\r\n\
             DTEND:20260116T110000Z\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup with the board\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({
                    "start": "2026-01-16T10:00:00",
                    "title": "Standup with the board",
                }),
            )]
            .into()
        ),
    );
    // The series is unchanged: one daily rule, and the title of the other occurrences.
    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup"));
    assert_eq!(stored.recurrence_rule.unwrap().frequency, "daily");
}

#[test]
fn undoing_an_edit_to_one_occurrence_removes_the_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {
            "2026-01-20T09:00:00": {"title": "Standup with the board"},
        }}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RECURRENCE-ID:20260120T090000Z"),
        "{icalendar}"
    );
    // Evolution deletes the detached instance to make the occurrence like the others
    // again.
    let detached = icalendar.rfind("BEGIN:VEVENT").expect("two instances");
    let series = format!("{}END:VCALENDAR\r\n", &icalendar[..detached]);
    sync.save_component(&series, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_the_server_says_is_not_excluded_stays_spelled_that_way() {
    // RFC 8984 §4.3.4 defaults `excluded` to false. So an override that states false and
    // one that states nothing are the same instance, with the same single RDATE. A save
    // must not rewrite the property only because its form is shorter: the baseline is the
    // round trip, not the server's event.
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-20T09:00:00": {"excluded": false}}}),
    );
    let sync = fixture.sync();

    let before = sync.load_component(id.as_str()).unwrap();
    assert!(
        before.icalendar.contains("RDATE:20260120T090000Z"),
        "{before:?}"
    );
    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_component(&before.icalendar, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some([("2026-01-20T09:00:00".to_owned(), json!({"excluded": false}))].into()),
    );
    assert_eq!(
        sync.list_existing().unwrap().0,
        state_before,
        "a no-op save must not bump the server state and wake every other client"
    );
}

#[test]
fn clearing_the_description_clears_it_on_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"description": "bring the numbers"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited: String = icalendar
        .lines()
        .filter(|line| !line.starts_with("DESCRIPTION"))
        .map(|line| format!("{line}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).description, None);
}

#[test]
fn a_save_whose_start_cannot_be_read_leaves_the_servers_start_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();

    // JSCalendar cannot say "no start". So a component with an unreadable DTSTART must
    // not become an event that says it.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("DTSTART:20260115T090000Z", "DTSTART:whenever")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
}

#[test]
fn a_save_that_changes_nothing_sends_no_patch() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // The component cannot state three things exactly. The first is a time zone in its
    // other legal spelling. The second is a recurrence part that the RRULE must drop:
    // here a `bySetPosition` with no other `by*` part. The third is an instance edited
    // alone, which an RDATE can place but not describe. None of them is an edit, and none
    // must look like one.
    fixture.patch(
        &id,
        json!({
            "timeZone": "UTC",
            "recurrenceRule":{
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "bySetPosition": [-1],
            },
            "recurrenceOverrides": {
                "2026-01-22T09:00:00": {"title": "Standup (long)"},
            },
        }),
    );
    let sync = fixture.sync();

    let before = sync.load_component(id.as_str()).unwrap();
    let (state_before, _) = sync.list_existing().unwrap();
    let after = sync
        .save_component(&before.icalendar, Some(id.as_str()))
        .unwrap();

    assert_eq!(after, before);
    let (state_after, _) = sync.list_existing().unwrap();
    assert_eq!(
        state_after, state_before,
        "a no-op save must not bump the server state and wake every other client"
    );
}

/// The `TZID` that Evolution puts on every zoned component it saves. libical names its
/// built-in zones with its own solidus-prefixed identifier, and the editor sets the start
/// with the zone object. So this identifier returns even for a component that had the
/// plain zone name.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

/// The `VTIMEZONE` that libical writes beside it, cut to the two lines that matter here.
/// `X-LIC-LOCATION` records the IANA zone.
///
/// The backend's envelope carries it: `marshal::icalendar_from_instances` copies in a
/// definition for every zone that the instances use. These tests cover the mapping's
/// half. `jmap-functional`'s calendar leg shows that the two halves meet through real
/// EDS; this crate can only supply the identifier by hand.
const LIBICAL_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
TZID:/freeassociation.sourceforge.net/Europe/Berlin\r\n\
X-LIC-LOCATION:Europe/Berlin\r\n\
BEGIN:STANDARD\r\nTZNAME:CET\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
DTSTART:19701025T030000\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n";

/// A new spelling of a zone is not a zone change. Each save of a zoned appointment
/// returns libical's identifier instead of the name that the mapping wrote. As an edit,
/// it puts into `timeZone` a string that RFC 8984 §1.4.8 admits only beside a `timeZones`
/// definition. A server can reject that value, and the user's real edits with it.
#[test]
fn the_zone_evolution_respells_is_not_read_as_a_zone_change() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("TZID=Europe/Berlin", &format!("TZID={LIBICAL_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"))
        .replace("SUMMARY:Standup", "SUMMARY:Standup (daily)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (daily)"));
    assert_eq!(
        stored.time_zone.as_deref(),
        Some("Europe/Berlin"),
        "the zone was respelled, not changed"
    );
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T09:00:00"));
}

/// The same respelling with a real move: the chosen zone must reach the server under its
/// JSCalendar name.
#[test]
fn a_move_to_another_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace(
            "TZID=Europe/Berlin",
            "TZID=/freeassociation.sourceforge.net/America/New_York",
        )
        .replace(
            "BEGIN:VEVENT",
            "BEGIN:VTIMEZONE\r\n\
             TZID:/freeassociation.sourceforge.net/America/New_York\r\n\
             X-LIC-LOCATION:America/New_York\r\n\
             BEGIN:STANDARD\r\nTZNAME:EST\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
             DTSTART:19701101T020000\r\nEND:STANDARD\r\n\
             END:VTIMEZONE\r\nBEGIN:VEVENT",
        );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.time_zone.as_deref(), Some("America/New_York"));
}

/// A zone that only the document knows, on a *new* appointment.
///
/// Here the save cannot leave the zone alone. On an edit, the server already holds a
/// zone, and to keep it is the conservative answer. On a create there is nothing to keep.
/// A zone that cannot be stated makes the appointment floating: a wall-clock time that
/// each reader resolves in its own zone.
///
/// RFC 8984 §1.4.8 gives such a zone a form: the custom identifier, sent beside the
/// §4.7.2 `timeZones` entry that defines it. The definition comes from the document's own
/// `VTIMEZONE`, as in an Exchange invitation or another client's `.ics`. So the event
/// states its zone and the rules of that zone.
#[test]
fn a_new_event_in_a_zone_only_the_document_defines_carries_its_definition() {
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT
        .replace("TZID=Europe/Berlin", &format!("TZID={CUSTOM_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{CUSTOM_VTIMEZONE}BEGIN:VEVENT"));

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(
        stored.time_zone.as_deref(),
        Some(CUSTOM_TZID),
        "the appointment was filed floating, hours from where the user put it"
    );
    let definitions = stored.time_zones.expect("the zone the event names");
    let zone = definitions
        .get(CUSTOM_TZID)
        .unwrap_or_else(|| panic!("no definition for {CUSTOM_TZID}: {definitions:?}"));
    assert_eq!(zone["standard"][0]["offsetTo"], json!("+0100"));
    assert_eq!(zone["daylight"][0]["offsetTo"], json!("+0200"));
    // EDS receives the event with the definition beside it, because only that makes the
    // `TZID` resolvable.
    assert!(
        saved.icalendar.contains(&format!("TZID:{CUSTOM_TZID}\r\n")),
        "{saved:?}"
    );
}

/// A zone that the document does not define still makes the appointment floating. A
/// solidus-prefixed identifier with no `VTIMEZONE` refers to nothing. A server can refuse
/// the whole `CalendarEvent/set` for it, and the user then loses the appointment, not
/// only its zone.
#[test]
fn a_new_event_naming_a_zone_nothing_defines_is_filed_floating() {
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace("TZID=Europe/Berlin", &format!("TZID={CUSTOM_TZID}"));

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.time_zone, None);
    assert_eq!(stored.time_zones, None);
}

/// A series in a zone that cannot be sent, with an occurrence in a zone that can.
///
/// The series' `TZID` is a Windows name, so the create makes the series floating: no
/// `TimeZoneId` can state it. The moved occurrence is in a different zone that the
/// document *defines*, which RFC 8984 §1.4.8 admits. Earlier, the save dropped all of
/// `timeZones` with the series' zone. That removed the definition and left the override's
/// identifier dangling, so a server could refuse the whole `CalendarEvent/set`.
///
/// So the save prunes the map to the zones still in use, and does not empty it. The
/// series becomes floating, and the occurrence keeps its zone.
#[test]
fn a_new_events_unsendable_zone_keeps_the_definition_an_occurrence_still_names() {
    let fixture = Fixture::start();
    let icalendar = with_instance(
        &NEW_EVENT
            .replace("TZID=Europe/Berlin", "TZID=Unknown Vendor Standard Time")
            .replace(
                "DURATION:PT90M",
                "DURATION:PT90M\r\nRRULE:FREQ=DAILY;COUNT=3",
            )
            .replace("BEGIN:VEVENT", &format!("{CUSTOM_VTIMEZONE}BEGIN:VEVENT")),
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
             RECURRENCE-ID;TZID=Unknown Vendor Standard Time:20260116T130000\r\n\
             DTSTART;TZID={CUSTOM_TZID}:20260116T150000\r\n\
             DURATION:PT90M\r\n\
             SUMMARY:Planning\r\n\
             END:VEVENT\r\n"
        ),
    );

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.time_zone, None, "the series' zone cannot be stated");
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-16T13:00:00".to_owned(),
                json!({"start": "2026-01-16T15:00:00", "timeZone": CUSTOM_TZID}),
            )]
            .into()
        ),
    );
    let definitions = stored
        .time_zones
        .expect("the zone the moved occurrence names");
    let zone = definitions
        .get(CUSTOM_TZID)
        .unwrap_or_else(|| panic!("no definition for {CUSTOM_TZID}: {definitions:?}"));
    assert_eq!(zone["standard"][0]["offsetTo"], json!("+0100"));
}

/// The identifier that a server invents for a zone that no database names (RFC 8984
/// §1.4.8's second form), and the `VTIMEZONE` that defines it. A document from elsewhere
/// carries the pair.
const CUSTOM_TZID: &str = "/example.com/Europe-Berlin";

const CUSTOM_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
TZID:/example.com/Europe-Berlin\r\n\
BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\nTZNAME:CET\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\n\
TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\nTZNAME:CEST\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\n";

/// A zone that nothing in the document explains: a Windows name from Exchange, or
/// libical's own identifier with no `VTIMEZONE` to translate it. The backend's envelope
/// now defines the zones that its components name, so Evolution no longer sends the
/// second form. But other sources also build documents, and an identifier that no zone
/// database knows (the first form) still arrives undefined.
///
/// JSCalendar cannot carry either value, so the save sends neither. The server keeps its
/// zone, which is the zone that the component showed. The known cost for the remaining
/// case: the save also ignores a real user change to an unresolvable zone.
#[test]
fn a_zone_the_document_could_not_name_leaves_the_servers_alone() {
    for tzid in ["Unknown Vendor Standard Time", LIBICAL_TZID] {
        let fixture = Fixture::start();
        let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
        fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
        let sync = fixture.sync();

        let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
        let edited = icalendar
            .replace("TZID=Europe/Berlin", &format!("TZID={tzid}"))
            .replace("SUMMARY:Standup", "SUMMARY:Standup (daily)");
        sync.save_component(&edited, Some(id.as_str())).unwrap();

        let stored = fixture.event(&id);
        assert_eq!(
            stored.title.as_deref(),
            Some("Standup (daily)"),
            "the edit the user made must still arrive, {tzid}"
        );
        assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"), "{tzid}");
    }
}

/// The same value on a create, where there is no server zone to keep. The appointment
/// becomes floating, not lost. A wall-clock time with no zone shows correctly for the
/// user who typed it, and a refused event shows nothing.
#[test]
fn a_new_events_unnameable_zone_is_not_sent() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let icalendar = NEW_EVENT.replace("TZID=Europe/Berlin", "TZID=Unknown Vendor Standard Time");

    let saved = sync.save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.title.as_deref(), Some("Planning"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.time_zone, None);
}

/// A create with libical's spelling, which is what Evolution gives the backend for a new
/// zoned appointment.
#[test]
fn a_new_events_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let icalendar = NEW_EVENT
        .replace("TZID=Europe/Berlin", &format!("TZID={LIBICAL_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"));

    let saved = sync.save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
}

/// One occurrence moved into another zone. Evolution's editor writes this when
/// the user changes the time zone of one day of a series. The result is a
/// detached component whose `DTSTART` has a different `TZID` from the series.
/// The zone must reach the server in the override. If not, the series' zone
/// resolves the typed wall-clock time, and the occurrence moves by hours.
#[test]
fn moving_one_occurrence_to_another_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        // Put the definition for the instance's new zone once, before the series: two
        // copies make two zones under one TZID.
        .replacen(
            "BEGIN:VEVENT",
            &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"),
            1,
        );
    // The series is in UTC. The instance moves to Berlin, in libical's spelling for
    // built-in zones. The instance repeats the title and the status unchanged, which is
    // not an edit, so only the zone and the start reach the server.
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART;TZID={LIBICAL_TZID}:20260116T100000\r\n\
             DURATION:PT1H\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({"start": "2026-01-16T10:00:00", "timeZone": "Europe/Berlin"}),
            )]
            .into()
        ),
    );
}

/// The unexplained zone one level down from
/// [`a_zone_the_document_could_not_name_leaves_the_servers_alone`]. The save replaces
/// `recurrenceOverrides` whole, so it cannot send an entry with a value that RFC 8984
/// §1.4.8 does not admit. The server can reject the whole `CalendarEvent/set` and the
/// user's other edits with it. So the save leaves the property alone and sends the rest.
#[test]
fn an_occurrences_unnameable_zone_leaves_the_overrides_alone() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replace("SUMMARY:Standup", "SUMMARY:Standup (daily)");
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART;TZID=Unknown Vendor Standard Time:20260116T100000\r\n\
             DURATION:PT1H\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup (daily)\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides, None,
        "an override we cannot spell is not sent in place of the ones the server holds"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (daily)"),
        "the edit the user made must still arrive"
    );
}

/// One occurrence moved into a zone that the *document* defines: the other half of
/// [`an_occurrences_unnameable_zone_leaves_the_overrides_alone`], and the half that must
/// arrive.
///
/// The user moves one day of a series into the zone that an invitation brought. Evolution
/// files that `VTIMEZONE` in the calendar's own timezone store. The backend's envelope
/// copies it back beside the components that name it. So this crate receives a custom
/// identifier *with* its RFC 8984 §4.7.2 definition. §1.4.8 admits that pair, so the save
/// must send both halves. The identifier alone is a dangling reference, and the server
/// can reject the whole `CalendarEvent/set` for it.
///
/// The definition goes as its own entry, `timeZones/<pointer>`, and does not replace the
/// property. This keeps the module's rule, because a `VTIMEZONE` cannot show a zone's
/// `aliases`, `url` or `validUntil`. The save adds an entry that the server did not have,
/// and touches nothing that it had. See
/// [`an_occurrence_moved_into_a_zone_the_server_defines_leaves_the_definition_alone`].
#[test]
fn moving_one_occurrence_into_a_zone_only_the_document_defines_sends_the_definition() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    // Put the definition once, before the series: two copies make two zones under one
    // `TZID`.
    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replacen(
            "BEGIN:VEVENT",
            &format!("{CUSTOM_VTIMEZONE}BEGIN:VEVENT"),
            1,
        );
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART;TZID={CUSTOM_TZID}:20260116T100000\r\n\
             DURATION:PT1H\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({"start": "2026-01-16T10:00:00", "timeZone": CUSTOM_TZID}),
            )]
            .into()
        ),
        "the occurrence stayed where the series is, hours from where the user put it"
    );
    let definitions = stored
        .time_zones
        .expect("the zone the moved occurrence names");
    let zone = definitions
        .get(CUSTOM_TZID)
        .unwrap_or_else(|| panic!("no definition for {CUSTOM_TZID}: {definitions:?}"));
    assert_eq!(zone["standard"][0]["offsetTo"], json!("+0100"));
    assert_eq!(zone["daylight"][0]["offsetTo"], json!("+0200"));
}

/// The same move into an identifier that the **server** already defines, as in a second
/// edit of the same occurrence.
///
/// The save must add nothing and write nothing. The server's entry can have a `url`, a
/// `validUntil` or `aliases`. A `VTIMEZONE` cannot hold these, and the user never saw
/// them. So the override names the zone, and the definition beside it stays unchanged.
#[test]
fn an_occurrence_moved_into_a_zone_the_server_defines_leaves_the_definition_alone() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    // A definition with more data than a document can state, under the identifier that
    // the component below names.
    let held = json!({
        "@type": "TimeZone",
        "tzId": CUSTOM_TZID,
        "url": "https://example.com/zones/Europe-Berlin",
        "standard": [{
            "@type": "TimeZoneRule",
            "start": "1970-10-25T03:00:00",
            "offsetFrom": "+0200",
            "offsetTo": "+0100",
        }],
    });
    fixture.patch(&id, json!({"timeZones": {CUSTOM_TZID: held}}));
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replacen(
            "BEGIN:VEVENT",
            &format!("{CUSTOM_VTIMEZONE}BEGIN:VEVENT"),
            1,
        );
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID:20260116T090000Z\r\n\
             DTSTART;TZID={CUSTOM_TZID}:20260116T100000\r\n\
             DURATION:PT1H\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({"start": "2026-01-16T10:00:00", "timeZone": CUSTOM_TZID}),
            )]
            .into()
        ),
    );
    let definitions = stored.time_zones.expect("the zone the server defined");
    let zone = definitions
        .get(CUSTOM_TZID)
        .unwrap_or_else(|| panic!("no definition for {CUSTOM_TZID}: {definitions:?}"));
    assert_eq!(
        zone["url"],
        json!("https://example.com/zones/Europe-Berlin"),
        "the save overwrote a definition the user was never shown"
    );
    assert!(
        zone["daylight"].is_null(),
        "the server's definition was replaced by the document's drawing: {zone}"
    );
}

/// The whole series moved into a zone that only the document defines: the same pair one
/// level up. The user changes the appointment's zone to the one that an invitation
/// brought. `timeZone` and its `timeZones` entry must go together, or the identifier
/// names nothing.
#[test]
fn moving_a_series_into_a_zone_only_the_document_defines_sends_the_definition() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replace("TZID=Europe/Berlin", &format!("TZID={CUSTOM_TZID}"))
        .replace("BEGIN:VEVENT", &format!("{CUSTOM_VTIMEZONE}BEGIN:VEVENT"));
    sync.save_component(&icalendar, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.time_zone.as_deref(),
        Some(CUSTOM_TZID),
        "the zone the user picked did not reach the server"
    );
    let definitions = stored.time_zones.expect("the zone the event names");
    let zone = definitions
        .get(CUSTOM_TZID)
        .unwrap_or_else(|| panic!("no definition for {CUSTOM_TZID}: {definitions:?}"));
    assert_eq!(zone["standard"][0]["offsetTo"], json!("+0100"));
}

/// A second identifier for an event that already has a `timeZones` map: the case for the
/// pointer path.
///
/// If the server holds no map, the save writes the whole property (RFC 8620 §5.3 requires
/// all path segments before the last to exist). Nothing is at risk, because nothing was
/// there. If the server holds a map, to write the property deletes the series' zone. The
/// appointment's own clock resolves against that definition, so the whole event becomes
/// unresolvable. So the save adds the entry under its own pointer, and the map keeps
/// everything else, `url` included.
#[test]
fn a_second_zone_is_added_to_the_map_the_server_already_holds() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({
            "timeZone": CUSTOM_TZID,
            "timeZones": {CUSTOM_TZID: {
                "@type": "TimeZone",
                "tzId": CUSTOM_TZID,
                "url": "https://example.com/zones/Europe-Berlin",
                "standard": [{
                    "@type": "TimeZoneRule",
                    "start": "1970-10-25T03:00:00",
                    "offsetFrom": "+0200",
                    "offsetTo": "+0100",
                }],
            }},
        }),
    );
    let sync = fixture.sync();

    // The series keeps the server's zone. The occurrence moves into the second zone,
    // which only this document defines.
    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        .replacen(
            "BEGIN:VEVENT",
            &format!("{OTHER_CUSTOM_VTIMEZONE}BEGIN:VEVENT"),
            1,
        );
    let edited = with_instance(
        &icalendar,
        &format!(
            "BEGIN:VEVENT\r\n\
             UID:{id}\r\n\
             RECURRENCE-ID;TZID={CUSTOM_TZID}:20260116T090000\r\n\
             DTSTART;TZID={OTHER_CUSTOM_TZID}:20260116T100000\r\n\
             DURATION:PT1H\r\n\
             STATUS:CONFIRMED\r\n\
             SUMMARY:Standup\r\n\
             END:VEVENT\r\n"
        ),
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-16T09:00:00".to_owned(),
                json!({"start": "2026-01-16T10:00:00", "timeZone": OTHER_CUSTOM_TZID}),
            )]
            .into()
        ),
    );
    let definitions = stored.time_zones.expect("the zones the event names");
    assert_eq!(
        definitions[CUSTOM_TZID]["url"],
        json!("https://example.com/zones/Europe-Berlin"),
        "the series' own zone was overwritten to add the occurrence's: {definitions:?}"
    );
    assert_eq!(
        definitions[OTHER_CUSTOM_TZID]["standard"][0]["offsetTo"],
        json!("-0500"),
        "the zone the occurrence moved into did not reach the server: {definitions:?}"
    );
}

/// A second identifier, for a zone in America, so a definition under it cannot be
/// [`CUSTOM_VTIMEZONE`] under a different name.
const OTHER_CUSTOM_TZID: &str = "/example.com/America-New_York";

const OTHER_CUSTOM_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
TZID:/example.com/America-New_York\r\n\
BEGIN:STANDARD\r\nDTSTART:19701101T020000\r\n\
TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\nTZNAME:EST\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:19700308T020000\r\n\
TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n\
RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\nTZNAME:EDT\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\n";

/// An event with one place, with a server-style key, and the component that Evolution
/// shows for it.
fn placed(fixture: &Fixture, location: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"locations": {"srv1": location}}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

#[test]
fn naming_a_place_reaches_the_server_as_a_location() {
    // The event had no place, so RFC 8620 §5.3 gives no path to patch. The save writes
    // the whole property, under a key that the component invented.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nLOCATION:Room 42");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).locations,
        Some(
            [(
                "l1".to_owned(),
                json!({"@type": "Location", "name": "Room 42"})
            )]
            .into()
        )
    );
}

#[test]
fn renaming_a_place_keeps_what_the_component_could_not_show() {
    // The reason to patch `locations/<key>/name` and not to replace the property: a
    // `LOCATION` line is one string. The renamed place also has coordinates, a
    // description and a zone that were never on the line.
    let fixture = Fixture::start();
    let (id, icalendar) = placed(
        &fixture,
        json!({
            "@type": "Location",
            "name": "Room 42",
            "coordinates": "geo:52.520008,13.404954",
            "locationTypes": {"office": true},
        }),
    );

    let edited = icalendar.replace("Room 42", "Room 43");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).locations,
        Some(
            [(
                "srv1".to_owned(),
                json!({
                    "@type": "Location",
                    "name": "Room 43",
                    "coordinates": "geo:52.520008,13.404954",
                    "locationTypes": {"office": true},
                })
            )]
            .into()
        ),
        "the entry was replaced instead of renamed"
    );
}

#[test]
fn a_place_renamed_without_its_key_still_reaches_the_servers_own_entry() {
    // Evolution's editor writes the LOCATION again, so the component can lose its
    // X-JMAP-KEY. The diff compares the name, and patches the server's own key.
    let fixture = Fixture::start();
    let (id, icalendar) = placed(
        &fixture,
        json!({"@type": "Location", "name": "Room 42", "coordinates": "geo:52,13"}),
    );

    let edited = icalendar.replace("LOCATION;X-JMAP-KEY=srv1:Room 42", "LOCATION:Room 43");
    assert!(edited.contains("LOCATION:Room 43"), "{icalendar}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).locations,
        Some(
            [(
                "srv1".to_owned(),
                json!({"@type": "Location", "name": "Room 43", "coordinates": "geo:52,13"})
            )]
            .into()
        )
    );
}

#[test]
fn a_place_that_did_not_change_is_not_sent_at_all() {
    let fixture = Fixture::start();
    let (id, icalendar) = placed(&fixture, json!({"@type": "Location", "name": "Room 42"}));

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.locations,
        Some(
            [(
                "srv1".to_owned(),
                json!({"@type": "Location", "name": "Room 42"})
            )]
            .into()
        )
    );
}

#[test]
fn clearing_a_place_that_was_only_a_name_removes_it() {
    // Nothing remains of the entry, and `maps_locations` already confirmed that no second
    // place is lost. So the save removes the property.
    let fixture = Fixture::start();
    let (id, icalendar) = placed(&fixture, json!({"@type": "Location", "name": "Room 42"}));

    let edited = icalendar.replace("LOCATION;X-JMAP-KEY=srv1:Room 42\r\n", "");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).locations, None);
}

#[test]
fn clearing_a_place_that_said_more_than_its_name_keeps_the_rest() {
    let fixture = Fixture::start();
    let (id, icalendar) = placed(
        &fixture,
        json!({"@type": "Location", "name": "Room 42", "coordinates": "geo:52,13"}),
    );

    let edited = icalendar.replace("LOCATION;X-JMAP-KEY=srv1:Room 42\r\n", "");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).locations,
        Some(
            [(
                "srv1".to_owned(),
                json!({"@type": "Location", "coordinates": "geo:52,13"})
            )]
            .into()
        ),
        "only the name the user cleared was cleared"
    );
}

#[test]
fn a_second_place_the_component_could_not_show_is_left_alone() {
    // One LOCATION line, two places. The user never saw the second place, so an edit to
    // the first cannot be the user's edit either.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let places = json!({
        "srv1": {"@type": "Location", "name": "Room 42"},
        "srv2": {"@type": "Location", "name": "Cafeteria"},
    });
    fixture.patch(&id, json!({"locations": places.clone()}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = icalendar
        .replace("Room 42", "Room 43")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.locations,
        Some(serde_json::from_value(places).unwrap()),
        "a property shown in part must not be written back"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

/// An event with server-style tags, and the component that Evolution shows for it.
fn tagged(fixture: &Fixture, keywords: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"keywords": keywords}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

#[test]
fn tagging_an_event_reaches_the_server_as_keywords() {
    // Evolution's "Categories…" button writes this. Unlike a place, the set shows whole,
    // so the save replaces it whole, with no key to patch.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = icalendar.replace(
        "SUMMARY:Standup",
        "SUMMARY:Standup\r\nCATEGORIES:offsite,planning",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some(
            [
                ("offsite".to_owned(), json!(true)),
                ("planning".to_owned(), json!(true)),
            ]
            .into()
        )
    );
}

#[test]
fn adding_a_tag_to_a_tagged_event_sends_the_whole_set() {
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": true}));
    assert!(icalendar.contains("CATEGORIES:offsite"), "{icalendar}");

    let edited = icalendar.replace("CATEGORIES:offsite", "CATEGORIES:offsite,travel");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some(
            [
                ("offsite".to_owned(), json!(true)),
                ("travel".to_owned(), json!(true)),
            ]
            .into()
        )
    );
}

#[test]
fn a_tag_set_that_did_not_change_is_not_sent_at_all() {
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": true}));

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.keywords,
        Some([("offsite".to_owned(), json!(true))].into())
    );
}

#[test]
fn clearing_every_tag_removes_the_property() {
    // A PatchObject removes a property to mean "the default", and RFC 8984 §4.2.9's
    // default is no keywords. An empty map is a different value to store and to return.
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": true, "planning": true}));

    let edited = icalendar.replace("CATEGORIES:offsite,planning\r\n", "");
    assert!(!edited.contains("CATEGORIES"), "{edited}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).keywords, None);
}

#[test]
fn a_tag_the_component_could_not_show_survives_the_set_being_rewritten() {
    // RFC 8984 §1.4.3 requires each value of a Set to be `true`. This server sent another
    // value, so the tag never reached the CATEGORIES line. The save replaces the property
    // whole. To write only the line deletes the unseen tag, and to write nothing drops
    // the user's edit. So the save copies the tag into the set exactly as the server
    // stated it. An unseen tag is not the user's to delete, and its odd value is the
    // server's to keep.
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": true, "odd": "yes"}));
    assert_eq!(
        icalendar
            .lines()
            .find(|line| line.starts_with("CATEGORIES"))
            .map(str::trim_end),
        Some("CATEGORIES:offsite"),
        "{icalendar}"
    );

    let edited = icalendar
        .replace("CATEGORIES:offsite", "CATEGORIES:offsite,travel")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.keywords,
        Some(
            [
                ("odd".to_owned(), json!("yes")),
                ("offsite".to_owned(), json!(true)),
                ("travel".to_owned(), json!(true)),
            ]
            .into()
        ),
        "the tag the user typed was dropped, or the one they never saw was"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn clearing_every_tag_leaves_the_tag_nobody_saw_behind() {
    // To empty the field deletes only the tags that it showed. A `null` deletes the tag
    // that had no line, which the user cannot have meant.
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": true, "odd": "yes"}));

    let edited = icalendar.replace("CATEGORIES:offsite\r\n", "");
    assert!(!edited.contains("CATEGORIES"), "{edited}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some([("odd".to_owned(), json!("yes"))].into())
    );
}

#[test]
fn typing_a_tag_the_server_had_set_to_something_else_sets_it() {
    // The only case where both sides name the same tag. The user's word wins: the user
    // typed it into a "filed under" field, so the tag is set. To copy the server's value
    // over it silently unsets the tag that the user just typed.
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"offsite": false}));
    assert!(!icalendar.contains("CATEGORIES"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nCATEGORIES:offsite");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some([("offsite".to_owned(), json!(true))].into())
    );
}

#[test]
fn a_set_holding_a_tag_with_no_line_is_not_an_edit_waiting_to_happen() {
    // The restored tag is the existing tag, so the set to write equals the server's set.
    // A patch that names it can undo a concurrent edit from another client. The save must
    // not name the property, and not only write it back unchanged.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"keywords": {"offsite": true, "odd": "yes"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let (state_before, _) = sync.list_existing().unwrap();
    sync.save_component(&icalendar, Some(id.as_str())).unwrap();
    let (state_after, _) = sync.list_existing().unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some(
            [
                ("odd".to_owned(), json!("yes")),
                ("offsite".to_owned(), json!(true)),
            ]
            .into()
        )
    );
    assert_eq!(
        state_after, state_before,
        "a save with nothing to say about tags rewrote them anyway"
    );
}

#[test]
fn refiling_one_occurrence_reaches_the_server_as_an_override() {
    // One occurrence of a tagged series filed under a different tag. EDS keeps the master
    // and adds a VEVENT with its RECURRENCE-ID. The CATEGORIES on that component replace
    // the series' set for that instance, and do not add to it. A PatchObject that names
    // `keywords` means this.
    let fixture = Fixture::start();
    let (id, _) = tagged(&fixture, json!({"offsite": true}));
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Repeat all series properties except the tags. The comparison with the series is per
    // property, so a missing line is an edit to that property, not to the filing.
    let instance = format!(
        "BEGIN:VEVENT\r\nUID:{id}\r\nRECURRENCE-ID:20260122T090000Z\r\n\
         DTSTART:20260122T090000Z\r\nSUMMARY:Standup\r\nDURATION:PT1H\r\n\
         STATUS:CONFIRMED\r\nCATEGORIES:cancelled\r\nEND:VEVENT\r\n"
    );
    let edited = icalendar.replace("END:VCALENDAR", &format!("{instance}END:VCALENDAR"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-22T09:00:00".to_owned(),
                json!({"keywords": {"cancelled": true}})
            )]
            .into()
        )
    );
    assert_eq!(
        stored.keywords,
        Some([("offsite".to_owned(), json!(true))].into()),
        "the series keeps the tags it had"
    );
}

#[test]
fn a_tag_one_occurrence_could_not_show_leaves_the_overrides_alone() {
    // An unseen tag one level down. This instance's set holds an entry that RFC 8984
    // §1.4.3 does not admit, so only a bare RDATE can place the occurrence. The save
    // replaces `recurrenceOverrides` whole, so to send the drawing deletes the unseen tag
    // and the whole override.
    let fixture = Fixture::start();
    let (id, _) = tagged(&fixture, json!({"offsite": true}));
    let overrides = json!({"2026-01-22T09:00:00": {"keywords": {"cancelled": true, "odd": "yes"}}});
    fixture.patch(
        &id,
        json!({
            "recurrenceRule":{"frequency": "weekly"},
            "recurrenceOverrides": overrides,
        }),
    );
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RDATE:20260122T090000Z"),
        "the occurrence was drawn as more than a bare date\n{icalendar}"
    );

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(serde_json::from_value(overrides).unwrap()),
        "an override shown in part must not be written back"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn a_tag_holding_a_comma_survives_the_save_as_one_tag() {
    // CATEGORIES is a value list, so the escape keeps one tag from becoming two in the
    // round trip.
    let fixture = Fixture::start();
    let (id, icalendar) = tagged(&fixture, json!({"Berlin, offsite": true}));
    assert!(
        icalendar.contains("CATEGORIES:Berlin\\, offsite"),
        "{icalendar}"
    );

    let edited = icalendar.replace(
        "CATEGORIES:Berlin\\, offsite",
        "CATEGORIES:Berlin\\, offsite,travel",
    );
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(
        fixture.event(&id).keywords,
        Some(
            [
                ("Berlin, offsite".to_owned(), json!(true)),
                ("travel".to_owned(), json!(true)),
            ]
            .into()
        )
    );
}

/// An event with server-style reminders, and the component that Evolution shows for it.
fn reminded(fixture: &Fixture, alerts: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"alerts": alerts}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

/// The reminder that a server states: a message 15 minutes before the appointment.
fn quarter_of_an_hour_before() -> serde_json::Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
        "action": "display",
    })
}

/// A reminder at an absolute instant (RFC 8984 §4.5.2's AbsoluteTrigger). The VALARMs
/// that this mapping draws do not state it.
fn at_an_instant() -> serde_json::Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T08:45:00Z"},
        "action": "display",
    })
}

/// The `VALARM` that Evolution's editor writes for such a reminder: no RFC 9074 `UID`,
/// its own `X-EVOLUTION-ALARM-UID`, and the summary as the display text.
const EVOLUTION_VALARM: &str = "BEGIN:VALARM\r\n\
X-EVOLUTION-ALARM-UID:20260115T090000Z-4711\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Standup\r\n\
TRIGGER;VALUE=DURATION;RELATED=START:-PT15M\r\n\
END:VALARM\r\n";

#[test]
fn setting_a_reminder_reaches_the_server_as_an_alert() {
    // Evolution's "Reminder" control writes a VALARM inside the VEVENT: the first mapped
    // property that is a component, not a line.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = icalendar.replace("END:VEVENT", &format!("{EVOLUTION_VALARM}END:VEVENT"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).alerts,
        // The key is the one that this mapping invents, because the component named no id
        // for the `alerts` map.
        Some([("a1".to_owned(), quarter_of_an_hour_before())].into())
    );
}

#[test]
fn moving_a_reminder_sends_the_whole_set_under_the_servers_own_key() {
    // Unlike a place, the save does not patch into an alert: it sends the whole map. The
    // server's key must survive, on the VALARM's RFC 9074 UID. Under a different key, the
    // user gets the same reminder two times.
    let fixture = Fixture::start();
    let (id, icalendar) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    assert!(icalendar.contains("\r\nUID:k1\r\n"), "{icalendar}");
    assert!(icalendar.contains("\r\nTRIGGER:-PT15M\r\n"), "{icalendar}");

    let edited = icalendar.replace("TRIGGER:-PT15M", "TRIGGER:-PT1H");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let mut moved = quarter_of_an_hour_before();
    moved["trigger"]["offset"] = json!("-PT1H");
    assert_eq!(
        fixture.event(&id).alerts,
        Some([("k1".to_owned(), moved)].into())
    );
}

#[test]
fn a_reminder_that_did_not_change_is_not_sent_at_all() {
    let fixture = Fixture::start();
    let (id, icalendar) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.alerts,
        Some([("k1".to_owned(), quarter_of_an_hour_before())].into())
    );
}

#[test]
fn clearing_the_reminder_removes_the_property() {
    // A PatchObject removes a property to mean "the default", and RFC 8984 §4.5.2's
    // default is no alerts. An empty map is a different value to store.
    let fixture = Fixture::start();
    let (id, icalendar) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));

    let opened = icalendar.find("BEGIN:VALARM").expect("a VALARM");
    let closed = icalendar.find("END:VALARM\r\n").expect("a VALARM") + "END:VALARM\r\n".len();
    let edited = format!("{}{}", &icalendar[..opened], &icalendar[closed..]);
    assert!(!edited.contains("VALARM"), "{edited}");

    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).alerts, None);
}

#[test]
fn a_reminder_the_component_could_not_show_leaves_the_whole_set_alone() {
    // The VALARM that this mapping writes cannot hold a reminder at an absolute instant
    // (RFC 8984 §4.5.2's AbsoluteTrigger), so the alert was never drawn. To replace the
    // set whole deletes what the user never saw.
    let fixture = Fixture::start();
    let alerts = json!({"k1": quarter_of_an_hour_before(), "k2": at_an_instant()});
    let (id, icalendar) = reminded(&fixture, alerts.clone());
    assert_eq!(
        icalendar.matches("BEGIN:VALARM").count(),
        1,
        "the reminder at an instant was drawn\n{icalendar}"
    );

    let edited = icalendar
        .replace("TRIGGER:-PT15M", "TRIGGER:-PT1H")
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.alerts,
        Some(serde_json::from_value(alerts).unwrap()),
        "a property shown in part must not be written back"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn dismissing_a_reminder_reaches_the_server_as_its_acknowledged_instant() {
    // To dismiss a reminder, EDS writes RFC 9074 §6.1's ACKNOWLEDGED onto the VALARM and
    // saves the component (`e_cal_util_set_alarm_acknowledged`, as its CalDAV backend's
    // `discard_alarm_sync` uses it). RFC 8984 §4.5.2's `acknowledged` is the same
    // instant. Other clients read it to stop the same reminder.
    let fixture = Fixture::start();
    let (id, icalendar) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));

    let edited = icalendar.replace(
        "TRIGGER:-PT15M\r\n",
        "TRIGGER:-PT15M\r\nACKNOWLEDGED:20260115T084700Z\r\n",
    );
    assert_ne!(edited, icalendar, "{icalendar}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let mut dismissed = quarter_of_an_hour_before();
    dismissed["acknowledged"] = json!("2026-01-15T08:47:00Z");
    let stored = fixture.event(&id);
    assert_eq!(stored.alerts, Some([("k1".to_owned(), dismissed)].into()));

    // The component shows the dismissal again, so the next unrelated save keeps it.
    let shown = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    assert!(
        shown.contains("ACKNOWLEDGED:20260115T084700Z\r\n"),
        "{shown}"
    );
}

#[test]
fn setting_a_reminder_on_one_occurrence_reaches_the_server_as_an_override() {
    // One occurrence of a reminded series with different reminders. EDS keeps the master
    // and adds a VEVENT with its RECURRENCE-ID. The VALARMs in that component replace the
    // series' set for that instance, and do not add to it. A PatchObject that names
    // `alerts` means this.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Repeat all series properties except the reminder. The comparison with the series is
    // per property, so a missing line is an edit to that property, not to the reminder.
    // The alarm keeps the server's key on its RFC 9074 UID: one hour earlier is the same
    // reminder moved, not a second one.
    let instance = format!(
        "BEGIN:VEVENT\r\nUID:{id}\r\nRECURRENCE-ID:20260122T090000Z\r\n\
         DTSTART:20260122T090000Z\r\nSUMMARY:Standup\r\nDURATION:PT1H\r\n\
         STATUS:CONFIRMED\r\nBEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\n\
         DESCRIPTION:Standup\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\nEND:VEVENT\r\n"
    );
    let edited = icalendar.replace("END:VCALENDAR", &format!("{instance}END:VCALENDAR"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let mut moved = quarter_of_an_hour_before();
    moved["trigger"]["offset"] = json!("-PT1H");
    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(
            [(
                "2026-01-22T09:00:00".to_owned(),
                json!({"alerts": {"k1": moved}})
            )]
            .into()
        )
    );
    assert_eq!(
        stored.alerts,
        Some([("k1".to_owned(), quarter_of_an_hour_before())].into()),
        "the series keeps the reminder it had"
    );
}

#[test]
fn a_reminder_one_occurrence_could_not_show_leaves_the_overrides_alone() {
    // The overrides form of
    // `a_reminder_the_component_could_not_show_leaves_the_whole_set_alone`. This
    // instance's set has a reminder at an absolute instant, which no VALARM here states.
    // So only a bare RDATE can place the occurrence. The save replaces
    // `recurrenceOverrides` whole, so to send the drawing deletes that reminder and the
    // whole override.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    let overrides = json!({"2026-01-22T09:00:00": {"alerts": {"k2": at_an_instant()}}});
    fixture.patch(
        &id,
        json!({
            "recurrenceRule":{"frequency": "weekly"},
            "recurrenceOverrides": overrides,
        }),
    );
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RDATE:20260122T090000Z"),
        "the occurrence was drawn as more than a bare date\n{icalendar}"
    );

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(serde_json::from_value(overrides).unwrap()),
        "an override shown in part must not be written back"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn an_occurrence_of_an_event_that_takes_the_default_reminders_keeps_its_own() {
    // `an_event_that_takes_the_default_reminders_keeps_them` one level down. It is also
    // why the *series* decides the coverage of `alerts`, the only restated property where
    // it does. An override cannot restate RFC 8984 §4.5.1's `useDefaultAlerts`, so the
    // series' value applies to every instance. The occurrence's own reminders are then
    // ignored, like the series' reminders. Nothing is drawn for them, so a save must
    // leave the property alone.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    // The override renames the occurrence *and* states reminders for it, so the instance
    // is drawn. The user then edits that title, so the save replaces
    // `recurrenceOverrides` whole. If the reminders counted as covered, that replacement
    // deletes them.
    let overrides = json!({
        "2026-01-22T09:00:00": {
            "title": "Standup (demo)",
            "alerts": {"k1": quarter_of_an_hour_before()},
        },
    });
    fixture.patch(
        &id,
        json!({
            "useDefaultAlerts": true,
            "recurrenceRule":{"frequency": "weekly"},
            "recurrenceOverrides": overrides,
        }),
    );
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        !icalendar.contains("VALARM"),
        "an occurrence was drawn with reminders nothing reads\n{icalendar}"
    );
    assert!(icalendar.contains("SUMMARY:Standup (demo)"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup (demo)", "SUMMARY:Standup (demo, short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    // No part of the override reaches the server, also not the user's rename. That is the
    // cost of a property that goes whole or not at all: one entry that cannot be stated
    // stops all others.
    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(serde_json::from_value(overrides).unwrap()),
        "a property nothing reads must not be written"
    );
}

#[test]
fn an_event_that_takes_the_default_reminders_keeps_them() {
    // RFC 8984 §4.5.1: when `useDefaultAlerts` is true, the user's own default reminders
    // fire and `alerts` has no effect. So there is nothing to show, and nothing useful
    // for a save to write. To use a reminder added here, the save must also clear the
    // flag, which this mapping does not do.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    fixture.patch(&id, json!({"useDefaultAlerts": true}));
    let sync = fixture.sync();
    // Read again after the flag change: the reminder was drawn before the change, and is
    // not drawn now.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("VALARM"), "{icalendar}");

    let edited = icalendar
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)")
        .replace("END:VEVENT", &format!("{EVOLUTION_VALARM}END:VEVENT"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.alerts,
        Some([("k1".to_owned(), quarter_of_an_hour_before())].into()),
        "a property nothing reads must not be written"
    );
    assert_eq!(
        stored.use_default_alerts.or_else(|| stored
            .extra
            .get("useDefaultAlerts")
            .and_then(|v| v.as_bool())),
        Some(true)
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

/// An event with a server-stated transparency, and the component that Evolution shows for
/// it.
fn shown_as(fixture: &Fixture, free_busy_status: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"freeBusyStatus": free_busy_status}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

#[test]
fn showing_an_event_as_free_reaches_the_server_as_a_free_busy_status() {
    // Evolution's "Show Time as: Free" is a TRANSP line on the component, and RFC 8984
    // §4.4.2's `freeBusyStatus` on the server. The mock's event does not state it, so the
    // save also creates a property here, and does not only change one.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("TRANSP"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nTRANSP:TRANSPARENT");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).free_busy_status.as_deref(), Some("free"));
}

#[test]
fn the_transparency_the_server_states_is_shown_and_left_alone_by_an_unrelated_edit() {
    let fixture = Fixture::start();
    let (id, icalendar) = shown_as(&fixture, json!("free"));
    assert!(icalendar.contains("TRANSP:TRANSPARENT"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(stored.free_busy_status.as_deref(), Some("free"));
}

#[test]
fn marking_a_free_event_busy_again_sends_the_other_state() {
    let fixture = Fixture::start();
    let (id, icalendar) = shown_as(&fixture, json!("free"));

    let edited = icalendar.replace("TRANSP:TRANSPARENT", "TRANSP:OPAQUE");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).free_busy_status.as_deref(), Some("busy"));
}

#[test]
fn removing_the_transp_line_removes_the_property() {
    // A PatchObject removes a property to mean "the default". RFC 8984 §4.4.2's default
    // is busy, and iCalendar means the same by a VEVENT with no TRANSP (RFC 5545
    // §3.8.2.7).
    let fixture = Fixture::start();
    let (id, icalendar) = shown_as(&fixture, json!("free"));

    let edited = icalendar.replace("TRANSP:TRANSPARENT\r\n", "");
    assert!(!edited.contains("TRANSP"), "{edited}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).free_busy_status, None);
}

#[test]
fn a_transparency_the_component_could_not_show_is_not_cleared_by_a_save() {
    // JSCalendar's vocabulary is closed, and this server's value is outside it, so no
    // TRANSP line was drawn. The baseline is the server's event through the same
    // rendering. It loses the value too, so the save does not see a cleared field and
    // sends nothing.
    let fixture = Fixture::start();
    let (id, icalendar) = shown_as(&fixture, json!("maybe"));
    assert!(!icalendar.contains("TRANSP"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.free_busy_status.as_deref(),
        Some("maybe"),
        "a value the component never showed cannot have been edited"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn marking_one_occurrence_free_reaches_the_server_as_an_override() {
    // "Show Time as: Free" on one occurrence. EDS keeps the master and adds a VEVENT with
    // its RECURRENCE-ID, and only its transparency differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Repeat all series properties. The comparison with the series is per property, so a
    // missing line is an edit to that property, not to the transparency.
    let instance = format!(
        "BEGIN:VEVENT\r\nUID:{id}\r\nRECURRENCE-ID:20260122T090000Z\r\n\
         DTSTART:20260122T090000Z\r\nSUMMARY:Standup\r\nDURATION:PT1H\r\n\
         STATUS:CONFIRMED\r\nTRANSP:TRANSPARENT\r\nEND:VEVENT\r\n"
    );
    let edited = icalendar.replace("END:VCALENDAR", &format!("{instance}END:VCALENDAR"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-22T09:00:00".to_owned(),
                json!({"freeBusyStatus": "free"})
            )]
            .into()
        )
    );
}

/// An event with a server-stated importance, and the component that Evolution shows for
/// it.
fn prioritised(fixture: &Fixture, priority: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"priority": priority}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

#[test]
fn making_an_event_urgent_reaches_the_server_as_a_priority() {
    // A PRIORITY line on the component (RFC 5545 §3.8.1.9) and RFC 8984 §4.4.1's
    // `priority` on the server are the same integer. The mock's event does not state it,
    // so the save also creates a property here, and does not only change one.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("PRIORITY"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nPRIORITY:1");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).priority, Some(1));
}

#[test]
fn the_priority_the_server_states_is_shown_and_left_alone_by_an_unrelated_edit() {
    let fixture = Fixture::start();
    let (id, icalendar) = prioritised(&fixture, json!(5));
    assert!(icalendar.contains("PRIORITY:5"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(stored.priority, Some(5));
}

#[test]
fn lowering_an_events_priority_sends_the_number_the_component_now_carries() {
    let fixture = Fixture::start();
    let (id, icalendar) = prioritised(&fixture, json!(1));

    let edited = icalendar.replace("PRIORITY:1", "PRIORITY:9");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).priority, Some(9));
}

#[test]
fn removing_the_priority_line_removes_the_property() {
    // A PatchObject removes a property to mean "the default". RFC 8984 §4.4.1's default
    // is 0, and RFC 5545 §3.8.1.9 means the same by a VEVENT with no PRIORITY.
    let fixture = Fixture::start();
    let (id, icalendar) = prioritised(&fixture, json!(5));

    let edited = icalendar.replace("PRIORITY:5\r\n", "");
    assert!(!edited.contains("PRIORITY"), "{edited}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).priority, None);
}

#[test]
fn a_priority_the_component_could_not_show_is_not_cleared_by_a_save() {
    // Both formats share the range 0..=9, and this server's value is outside it, so no
    // PRIORITY line was drawn. The baseline is the server's event through the same
    // rendering. It loses the value too, so the save does not see a cleared field and
    // sends nothing.
    let fixture = Fixture::start();
    let (id, icalendar) = prioritised(&fixture, json!(42));
    assert!(!icalendar.contains("PRIORITY"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.priority,
        Some(42),
        "a value the component never showed cannot have been edited"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn making_one_occurrence_urgent_reaches_the_server_as_an_override() {
    // One occurrence of a series marked important. EDS keeps the master and adds a VEVENT
    // with its RECURRENCE-ID, and only its priority differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Repeat all series properties. The comparison with the series is per property, so a
    // missing line is an edit to that property, not to the priority.
    let instance = format!(
        "BEGIN:VEVENT\r\nUID:{id}\r\nRECURRENCE-ID:20260122T090000Z\r\n\
         DTSTART:20260122T090000Z\r\nSUMMARY:Standup\r\nDURATION:PT1H\r\n\
         STATUS:CONFIRMED\r\nPRIORITY:1\r\nEND:VEVENT\r\n"
    );
    let edited = icalendar.replace("END:VCALENDAR", &format!("{instance}END:VCALENDAR"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some([("2026-01-22T09:00:00".to_owned(), json!({"priority": 1}))].into())
    );
}

/// An event with a server-stated classification, and the component that Evolution shows
/// for it.
fn classified(fixture: &Fixture, privacy: serde_json::Value) -> (jmap_proto::Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"privacy": privacy}));
    let icalendar = fixture
        .sync()
        .load_component(id.as_str())
        .unwrap()
        .icalendar;
    (id, icalendar)
}

#[test]
fn marking_an_event_private_reaches_the_server_as_a_privacy() {
    // Evolution's Options ▸ Classification ▸ Private is a CLASS line on the component
    // (RFC 5545 §3.8.1.3), and RFC 8984 §4.4.3's `privacy` on the server. The mock's
    // event does not state it, so the save also creates a property here, and does not
    // only change one.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(!icalendar.contains("CLASS"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nCLASS:PRIVATE");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).privacy.as_deref(), Some("private"));
}

#[test]
fn the_privacy_the_server_states_is_shown_and_left_alone_by_an_unrelated_edit() {
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("secret"));
    assert!(icalendar.contains("CLASS:CONFIDENTIAL"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(stored.privacy.as_deref(), Some("secret"));
}

#[test]
fn saving_a_public_event_back_unchanged_sends_no_patch() {
    // Evolution's editor makes this case routine: it writes CLASS on every save from its
    // Classification menu, and the menu default is public. So the component states the
    // default explicitly, and the save must send *nothing*. That works only because the
    // baseline also renders the line. This is why the mapping writes `CLASS:PUBLIC`,
    // although it is the default. Without it, the baseline always differs from a public
    // event's component, and each save sends a redundant `"privacy": "public"`.
    //
    // `get_changes` is the witness. A save with no patch does not reach the server, so
    // the account state stays. A save that patches the default makes the event show in
    // the delta.
    //
    // The test shapes the component as the editor leaves it: with a CLASS line, whether
    // or not the rendering wrote one. An unchanged rendering passes for any writer,
    // because both sides of the diff agree by construction. Only the editor's line
    // against a baseline without it costs a patch.
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("public"));
    let as_the_editor_leaves_it = if icalendar.contains("CLASS:") {
        icalendar
    } else {
        icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup\r\nCLASS:PUBLIC")
    };

    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    sync.save_component(&as_the_editor_leaves_it, Some(id.as_str()))
        .unwrap();

    let changes = sync.get_changes(&state).unwrap();
    assert!(
        changes.changed.is_empty() && changes.removed.is_empty(),
        "the save patched something: {changes:?}"
    );
}

#[test]
fn the_public_classification_survives_an_unrelated_edit() {
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("public"));
    assert!(icalendar.contains("CLASS:PUBLIC"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(stored.privacy.as_deref(), Some("public"));
}

#[test]
fn hiding_a_private_event_completely_sends_the_other_classification() {
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("private"));

    let edited = icalendar.replace("CLASS:PRIVATE", "CLASS:CONFIDENTIAL");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).privacy.as_deref(), Some("secret"));
}

#[test]
fn removing_the_class_line_removes_the_property() {
    // A PatchObject removes a property to mean "the default". RFC 8984 §4.4.3's default
    // is public, and RFC 5545 §3.8.1.3 means the same by a VEVENT with no CLASS.
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("secret"));

    let edited = icalendar.replace("CLASS:CONFIDENTIAL\r\n", "");
    assert!(!edited.contains("CLASS"), "{edited}");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    assert_eq!(fixture.event(&id).privacy, None);
}

#[test]
fn a_privacy_the_component_could_not_show_is_not_cleared_by_a_save() {
    // RFC 8984 §4.4.3 leaves the vocabulary open, and this server's value is none of the
    // three that iCalendar can spell. So no CLASS line was drawn. The baseline is the
    // server's event through the same rendering. It loses the value too, so the save does
    // not see a change to public and sends nothing. This matters more here than for the
    // closed vocabularies: the lost value controls who can read the event.
    let fixture = Fixture::start();
    let (id, icalendar) = classified(&fixture, json!("x-eyes-only"));
    assert!(!icalendar.contains("CLASS"), "{icalendar}");

    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    fixture
        .sync()
        .save_component(&edited, Some(id.as_str()))
        .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.privacy.as_deref(),
        Some("x-eyes-only"),
        "a value the component never showed cannot have been edited"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

#[test]
fn hiding_one_occurrence_reaches_the_server_as_an_override() {
    // One occurrence of a series marked private. EDS keeps the master and adds a VEVENT
    // with its RECURRENCE-ID, and only its classification differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Repeat all series properties. The comparison with the series is per property, so a
    // missing line is an edit to that property, not to the classification.
    let instance = format!(
        "BEGIN:VEVENT\r\nUID:{id}\r\nRECURRENCE-ID:20260122T090000Z\r\n\
         DTSTART:20260122T090000Z\r\nSUMMARY:Standup\r\nDURATION:PT1H\r\n\
         STATUS:CONFIRMED\r\nCLASS:PRIVATE\r\nEND:VEVENT\r\n"
    );
    let edited = icalendar.replace("END:VCALENDAR", &format!("{instance}END:VCALENDAR"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).recurrence_overrides,
        Some(
            [(
                "2026-01-22T09:00:00".to_owned(),
                json!({"privacy": "private"})
            )]
            .into()
        )
    );
}

#[test]
fn saving_over_an_unknown_identifier_is_not_found() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_component(NEW_EVENT, Some("no-such-event"))
        .unwrap_err();

    assert!(
        matches!(&error, jmap_cal_sync::SyncError::NotFound(uid) if uid == "no-such-event"),
        "{error:?}"
    );
}

#[test]
fn saving_something_that_holds_no_event_fails_before_any_request() {
    let fixture = Fixture::start();
    let error = fixture
        .sync()
        .save_component("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n", None)
        .unwrap_err();

    assert!(
        matches!(error, jmap_cal_sync::SyncError::ICal(_)),
        "{error:?}"
    );
    assert!(fixture.sync().list_existing().unwrap().1.is_empty());
}

#[test]
fn editing_location_and_categories_preserves_unmodeled_alarms_and_links() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "keywords": {"offsite": true},
            "locations": {"loc1": {"@type": "Location", "name": "Room 42"}},
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/agenda.pdf",
                    "contentType": "application/pdf",
                    "size": 51200,
                }
            },
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "start"},
                    "action": "display",
                }
            },
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = loaded
        .replace(
            "LOCATION;X-JMAP-KEY=loc1:Room 42",
            "LOCATION;X-JMAP-KEY=loc1:Conference Hall B",
        )
        .replace("CATEGORIES:offsite", "CATEGORIES:offsite,engineering");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Conference Hall B"));
    let keywords = stored.keywords.expect("keywords");
    assert!(keywords.contains_key("offsite"));
    assert!(keywords.contains_key("engineering"));

    // The links and alerts that the edit did not touch stay.
    let links = stored.links.expect("links");
    assert!(links.contains_key("l1"));
    assert_eq!(links["l1"]["size"], json!(51200));
    let alerts = stored.alerts.expect("alerts");
    assert!(alerts.contains_key("a1"));
}

#[test]
fn clearing_location_and_categories_generates_targeted_null_patches() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "keywords": {"offsite": true},
            "locations": {"loc1": {"@type": "Location", "name": "Room 42"}},
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/agenda.pdf",
                    "contentType": "application/pdf",
                }
            },
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = loaded
        .replace("LOCATION;X-JMAP-KEY=loc1:Room 42\r\n", "")
        .replace("CATEGORIES:offsite\r\n", "");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.locations, None);
    assert_eq!(stored.keywords, None);

    // The links stay.
    let links = stored.links.expect("links");
    assert!(links.contains_key("l1"));
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/agenda.pdf")
    );
}

#[test]
fn editing_priority_preserves_unmodeled_participants_and_geo_coordinates() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "priority": 1,
            "participants": {
                "alice": {
                    "@type": "Participant",
                    "name": "Alice Example",
                    "sendTo": {"imip": "mailto:alice@example.com"},
                    "roles": {"owner": true, "chair": true},
                    "participationStatus": "accepted"
                },
                "bob": {
                    "@type": "Participant",
                    "name": "Bob Example",
                    "sendTo": {"imip": "mailto:bob@example.com"},
                    "roles": {"attendee": true},
                    "participationStatus": "declined"
                }
            },
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Room 42",
                    "coordinates": "geo:52.520008,13.404954"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/agenda.pdf"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = loaded.replace("PRIORITY:1\r\n", "PRIORITY:5\r\n");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.priority, Some(5));

    // The participants stay unchanged.
    let parts = stored.participants.expect("participants");
    assert_eq!(parts["alice"]["name"], json!("Alice Example"));
    assert_eq!(parts["alice"]["roles"]["chair"], json!(true));
    assert_eq!(parts["bob"]["participationStatus"], json!("declined"));

    // The locations and links stay unchanged.
    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 42"));
    assert_eq!(
        locs["loc1"]["coordinates"],
        json!("geo:52.520008,13.404954")
    );
    let links = stored.links.expect("links");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/agenda.pdf")
    );
}

#[test]
fn editing_unrelated_field_preserves_participants_priority_and_geo_locations() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "priority": 3,
            "participants": {
                "alice": {
                    "@type": "Participant",
                    "name": "Alice Example",
                    "sendTo": {"imip": "mailto:alice@example.com"},
                    "roles": {"owner": true}
                }
            },
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Conference Hall A",
                    "coordinates": "geo:48.8566,2.3522"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = loaded.replace(
        "SUMMARY:Sprint Planning\r\n",
        "SUMMARY:Sprint Planning v2\r\n",
    );

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title, Some("Sprint Planning v2".to_owned()));
    assert_eq!(stored.priority, Some(3));

    let parts = stored.participants.expect("participants");
    assert_eq!(parts["alice"]["name"], json!("Alice Example"));

    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Conference Hall A"));
    assert_eq!(locs["loc1"]["coordinates"], json!("geo:48.8566,2.3522"));
}

#[test]
fn editing_alerts_preserves_unmodeled_locations_and_participants() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            },
            "participants": {
                "alice": {
                    "@type": "Participant",
                    "name": "Alice Example",
                    "sendTo": {"imip": "mailto:alice@example.com"},
                    "roles": {"owner": true}
                }
            },
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Conference Hall A",
                    "coordinates": "geo:48.8566,2.3522"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/agenda.pdf",
                    "rel": "enclosure"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = loaded.replace("TRIGGER:-PT15M\r\n", "TRIGGER:-PT30M\r\n");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT30M"));

    let parts = stored.participants.expect("participants");
    assert_eq!(parts["alice"]["name"], json!("Alice Example"));

    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Conference Hall A"));
    assert_eq!(locs["loc1"]["coordinates"], json!("geo:48.8566,2.3522"));

    let links = stored.links.expect("links");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/agenda.pdf")
    );
}

#[test]
fn editing_unrelated_field_preserves_alerts_intact() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = loaded.replace(
        "SUMMARY:Sprint Planning\r\n",
        "SUMMARY:Sprint Planning v2\r\n",
    );

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title, Some("Sprint Planning v2".to_owned()));

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));
}

#[test]
fn removing_all_alerts_generates_targeted_null_patch() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            },
            "participants": {
                "alice": {
                    "@type": "Participant",
                    "name": "Alice Example",
                    "sendTo": {"imip": "mailto:alice@example.com"},
                    "roles": {"owner": true}
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let valarm_start = loaded.find("BEGIN:VALARM\r\n").expect("VALARM start");
    let valarm_end = loaded.find("END:VALARM\r\n").expect("VALARM end") + "END:VALARM\r\n".len();
    let mut edited = loaded.clone();
    edited.replace_range(valarm_start..valarm_end, "");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.alerts, None);

    let parts = stored.participants.expect("participants");
    assert_eq!(parts["alice"]["name"], json!("Alice Example"));
}

#[test]
fn editing_recurring_event_summary_preserves_recurrence_rule_and_overrides() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "recurrenceRule":{
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "interval": 2,
                "count": 10
            },
            "recurrenceOverrides": {
                "2026-01-29T13:00:00": {
                    "title": "Sprint Review & Retro",
                    "status": "confirmed"
                },
                "2026-02-12T13:00:00": {
                    "excluded": true
                }
            },
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/planning",
                    "name": "Planning Room"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = loaded.replacen(
        "SUMMARY:Sprint Planning\r\n",
        "SUMMARY:Weekly Sprint Planning\r\n",
        1,
    );

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Weekly Sprint Planning"));

    let rules = stored.recurrence_rule.expect("rules");
    assert_eq!(rules.frequency, "weekly");
    assert_eq!(rules.interval, Some(2));
    assert_eq!(rules.count, Some(10));

    let overrides = stored.recurrence_overrides.expect("overrides");
    assert_eq!(
        overrides["2026-01-29T13:00:00"]["title"],
        json!("Sprint Review & Retro")
    );
    assert_eq!(overrides["2026-02-12T13:00:00"]["excluded"], json!(true));

    let vlocs = stored.virtual_locations.expect("virtual locations");
    assert_eq!(
        vlocs["v1"]["uri"],
        json!("https://meet.example.com/planning")
    );
}

#[test]
fn clearing_exdates_and_overrides_patches_server_recurrence_map() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Daily Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({
            "recurrenceRule":{
                "@type": "RecurrenceRule",
                "frequency": "daily",
                "interval": 1,
                "count": 5
            },
            "recurrenceOverrides": {
                "2026-01-17T09:00:00": {
                    "excluded": true
                }
            },
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT10M"
                    },
                    "action": "display"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let exdate_line = loaded
        .lines()
        .find(|l| l.starts_with("EXDATE"))
        .expect("EXDATE line");
    let edited = loaded.replace(&format!("{exdate_line}\r\n"), "");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Daily Standup"));
    assert_eq!(stored.recurrence_overrides, None);

    let rules = stored.recurrence_rule.expect("rules");
    assert_eq!(rules.frequency, "daily");

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT10M"));
}

#[test]
fn editing_event_datetime_and_duration_preserves_unmodeled_locations_and_links() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Planning", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "timeZone": "Europe/Berlin",
            "duration": "PT1H",
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Room 101",
                    "coordinates": "geo:48.8566,2.3522"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/spec.pdf",
                    "contentType": "application/pdf"
                }
            },
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT10M"
                    },
                    "action": "display"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = loaded
        .replace("20260115T130000", "20260116T150000")
        .replace("DURATION:PT1H", "DURATION:PT2H30M");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-16T15:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H30M"));
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));

    // The locations, links and alerts stay unchanged.
    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 101"));
    assert_eq!(locs["loc1"]["coordinates"], json!("geo:48.8566,2.3522"));

    let links = stored.links.expect("links");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/spec.pdf")
    );

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT10M"));
}

#[test]
fn editing_allday_dates_preserves_unmodeled_virtual_locations_and_keywords() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Hackathon", "2026-01-15T00:00:00");
    fixture.patch(
        &id,
        json!({
            "showWithoutTime": true,
            "timeZone": null,
            "duration": "P2D",
            "keywords": {"internal": true, "hackathon": true},
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/hackathon",
                    "name": "Hackathon Main Room"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(loaded.contains("VALUE=DATE:20260115"), "{loaded}");

    let edited = loaded
        .replace("20260115", "20260117")
        .replace("DURATION:P2D", "DURATION:P3D");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-17T00:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("P3D"));
    assert_eq!(stored.show_without_time, Some(true));

    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("internal"));
    assert!(kws.contains_key("hackathon"));

    let vlocs = stored.virtual_locations.expect("virtual locations");
    assert_eq!(
        vlocs["v1"]["uri"],
        json!("https://meet.example.com/hackathon")
    );
}

#[test]
fn editing_descriptions_and_timestamps_preserves_unmodeled_event_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Architecture Sync", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "description": "Initial architecture review",
            "keywords": {"architecture": true, "review": true},
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            },
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/arch",
                    "name": "Arch Channel"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/arch-spec.pdf",
                    "contentType": "application/pdf"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = loaded.replace(
        "DESCRIPTION:Initial architecture review\r\n",
        "DESCRIPTION:Updated architecture review with team\\; all welcome\r\n",
    );

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.description.as_deref(),
        Some("Updated architecture review with team; all welcome")
    );

    // The keywords, alerts, virtualLocations and links that the edit did not touch stay.
    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("architecture"));
    assert!(kws.contains_key("review"));

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));

    let vlocs = stored.virtual_locations.expect("virtual locations");
    assert_eq!(vlocs["v1"]["uri"], json!("https://meet.example.com/arch"));

    let links = stored.links.expect("links");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/arch-spec.pdf")
    );
}

#[test]
fn clearing_descriptions_and_comments_patches_server_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Architecture Sync", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "description": "Initial architecture review",
            "keywords": {"architecture": true},
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            },
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Room 101"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let desc_line = loaded
        .lines()
        .find(|l| l.starts_with("DESCRIPTION"))
        .expect("DESCRIPTION line");
    let edited = loaded.replace(&format!("{desc_line}\r\n"), "");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.description, None);

    // The locations and alerts stay.
    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 101"));

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));
}

#[test]
fn cancelling_an_event_reaches_the_server_as_cancelled_status() {
    // An organiser marks an event as cancelled: Evolution replaces `STATUS:CONFIRMED`
    // with `STATUS:CANCELLED`. The server must get the new status and keep the
    // description and location.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Offsite Workshop", "2026-03-10T09:00:00");
    fixture.patch(
        &id,
        json!({
            "status": "confirmed",
            "description": "Full-day offsite session",
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Conference Centre"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(loaded.contains("STATUS:CONFIRMED\r\n"), "{loaded}");

    let edited = loaded.replace("STATUS:CONFIRMED\r\n", "STATUS:CANCELLED\r\n");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.status.as_deref(),
        Some("cancelled"),
        "cancelled status did not reach the server"
    );
    assert_eq!(
        stored.description.as_deref(),
        Some("Full-day offsite session"),
        "description was wiped by the save"
    );
    let locs = stored.locations.expect("locations");
    assert_eq!(
        locs["loc1"]["name"],
        json!("Conference Centre"),
        "location was wiped by the save"
    );
}

#[test]
fn clearing_status_removes_it_from_the_server() {
    // The user removes a tentative mark: Evolution drops the `STATUS` line. The server's
    // `status` must become absent (null patch).
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Uncertain Meeting", "2026-03-12T14:00:00");
    fixture.patch(
        &id,
        json!({
            "status": "tentative",
            "keywords": {"planning": true},
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(loaded.contains("STATUS:TENTATIVE\r\n"), "{loaded}");

    let edited: String = loaded
        .lines()
        .filter(|l| !l.starts_with("STATUS"))
        .map(|l| format!("{l}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.status, None, "status was not cleared on the server");
    // The keyword that the user did not touch must stay.
    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("planning"), "keyword was lost: {kws:?}");
}

#[test]
fn clearing_privacy_removes_the_classification_from_the_server() {
    // The user removes the Classification: Evolution drops the `CLASS` line. The server's
    // `privacy` must become absent (null patch), so the RFC 8984 default `public`
    // applies.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Strategic Review", "2026-03-15T11:00:00");
    fixture.patch(
        &id,
        json!({
            "privacy": "secret",
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(loaded.contains("CLASS:CONFIDENTIAL\r\n"), "{loaded}");

    let edited: String = loaded
        .lines()
        .filter(|l| !l.starts_with("CLASS"))
        .map(|l| format!("{l}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(
        stored.privacy, None,
        "privacy was not cleared on the server"
    );
    // The alert that the user did not touch must stay.
    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));
}

#[test]
fn editing_location_and_priority_preserves_unmodeled_event_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Architecture Sync", "2026-01-15T13:00:00");
    fixture.patch(
        &id,
        json!({
            "priority": 3,
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Room 101",
                    "coordinates": "geo:52.520008,13.404954",
                    "description": "Main Building"
                }
            },
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/arch"
                }
            },
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": "https://files.example.com/arch-spec.pdf"
                }
            },
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M"
                    },
                    "action": "display"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(loaded.contains("PRIORITY:3\r\n"), "{loaded}");
    assert!(
        loaded.contains("LOCATION;X-JMAP-KEY=loc1:Room 101\r\n")
            || loaded.contains("LOCATION:Room 101\r\n"),
        "{loaded}"
    );

    // PRIORITY:1 is high priority.
    let edited = loaded
        .replace("Room 101", "Room 204 (West Wing)")
        .replace("PRIORITY:3", "PRIORITY:1");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.priority, Some(1));

    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 204 (West Wing)"));
    // The location's unmapped coordinates and description stay.
    assert_eq!(
        locs["loc1"]["coordinates"],
        json!("geo:52.520008,13.404954")
    );
    assert_eq!(locs["loc1"]["description"], json!("Main Building"));

    // The virtual locations, links and alerts stay.
    let vlocs = stored.virtual_locations.expect("virtual locations");
    assert_eq!(vlocs["v1"]["uri"], json!("https://meet.example.com/arch"));

    let links = stored.links.expect("links");
    assert_eq!(
        links["l1"]["href"],
        json!("https://files.example.com/arch-spec.pdf")
    );

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));
}

#[test]
fn clearing_location_and_priority_patches_server_fields() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Status Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({
            "priority": 2,
            "description": "Weekly status check",
            "keywords": {"standup": true},
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": "Room 302"
                }
            }
        }),
    );

    let sync = fixture.sync();
    let loaded = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited: String = loaded
        .lines()
        .filter(|l| !l.starts_with("LOCATION") && !l.starts_with("PRIORITY"))
        .map(|l| format!("{l}\r\n"))
        .collect();

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.priority, None);
    assert_eq!(stored.locations, None);

    // The unchanged description and keywords stay.
    assert_eq!(stored.description.as_deref(), Some("Weekly status check"));
    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("standup"));
}

#[test]
fn no_op_save_mints_no_patch_across_all_calendar_mapped_surfaces_matrix() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    // The cases cover:
    // - alerts (relative triggers, display action)
    // - recurrenceRule (monthly RRULE with INTERVAL and COUNT)
    // - recurrenceOverrides (one EXDATE)
    // - an IANA timeZone
    // - locations and virtualLocations (LOCATION, CONFERENCE)
    // - links (ATTACH)
    // - keywords (CATEGORIES)
    // - privacy, status, priority, freeBusyStatus, showWithoutTime
    // - an unmapped extra property
    let cases = vec![
        (
            "composite_recurring_event",
            json!({
                "title": "Quarterly Planning Workshop",
                "description": "Full-day strategic roadmap alignment",
                "start": "2026-03-01T09:00:00",
                "duration": "PT8H",
                "timeZone": "Europe/Berlin",
                "status": "confirmed",
                "privacy": "private",
                "priority": 1,
                "freeBusyStatus": "busy",
                "showWithoutTime": false,
                "locations": {
                    "loc1": {
                        "@type": "Location",
                        "name": "Main Auditorium",
                        "description": "Building A, Floor 2",
                        "coordinates": "geo:52.520008,13.404954"
                    }
                },
                "virtualLocations": {
                    "v1": {
                        "@type": "VirtualLocation",
                        "name": "Video Conference",
                        "uri": "https://meet.example.com/quarterly-planning",
                        "features": {"video": true, "audio": true}
                    }
                },
                "links": {
                    "l1": {
                        "@type": "Link",
                        "href": "https://docs.example.com/q1-slides.pdf",
                        "contentType": "application/pdf",
                        "size": 1048576,
                        "rel": "enclosure"
                    }
                },
                "keywords": {
                    "Strategy": true,
                    "Planning": true
                },
                "alerts": {
                    "a1": {
                        "@type": "Alert",
                        "action": "display",
                        "trigger": {
                            "@type": "OffsetTrigger",
                            "offset": "-PT30M",
                            "relativeTo": "start"
                        }
                    }
                },
                "recurrenceRule": {
                    "@type": "RecurrenceRule",
                    "frequency": "monthly",
                    "interval": 3,
                    "count": 4
                },
                "recurrenceOverrides": {
                    "2026-06-01T09:00:00": {
                        "excluded": true
                    }
                },
                "unmodeledCalendarBags": {"costCenter": "1040", "organizerNote": "cater lunch"}
            }),
        ),
        (
            "allday_event_with_custom_timezone_and_tags",
            json!({
                "title": "Annual Company Offsite",
                "start": "2026-07-15",
                "duration": "P3D",
                "showWithoutTime": true,
                "status": "confirmed",
                "privacy": "public",
                "keywords": {
                    "Offsite": true,
                    "Company": true
                },
                "alerts": {
                    "a1": {
                        "@type": "Alert",
                        "action": "display",
                        "trigger": {
                            "@type": "OffsetTrigger",
                            "offset": "-P1D",
                            "relativeTo": "start"
                        }
                    }
                }
            }),
        ),
    ];

    for (name, initial_patch) in cases {
        let id = fixture.seed(&fixture.ours, "Seed Event", "2026-01-15T10:00:00");
        fixture.patch(&id, initial_patch);

        let loaded1 = sync.load_component(id.as_str()).expect(name);

        let saved1 = sync
            .save_component(&loaded1.icalendar, Some(id.as_str()))
            .expect(name);

        let loaded2 = sync.load_component(id.as_str()).expect(name);

        // The second save must send no patch and keep the same revision.
        let saved2 = sync
            .save_component(&loaded2.icalendar, Some(id.as_str()))
            .expect(name);
        assert_eq!(
            saved1.revision, saved2.revision,
            "case '{name}': second save must not update revision"
        );

        // The diff between the server state and the parsed event must be empty.
        let current_event = fixture.event(&id);
        let parsed_event = jmap_ical::ical_to_event(&loaded2.icalendar).expect(name);
        let patch_diff = jmap_cal_sync::patch::diff(&current_event, &parsed_event);
        assert!(
            patch_diff.is_empty(),
            "case '{name}': diff after round-trip must be empty, found: {patch_diff:?}"
        );
    }
}

#[test]
fn no_op_save_mints_no_patch_on_empty_container_collections_and_empty_strings() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let id = fixture.seed(&fixture.ours, "Seed Event", "2026-01-15T10:00:00");
    fixture.patch(
        &id,
        json!({
            "title": "",
            "description": "",
            "locations": {
                "loc1": {
                    "@type": "Location",
                    "name": ""
                }
            },
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/standup",
                    "name": "",
                    "features": {}
                }
            },
            "keywords": {},
            "alerts": {},
            "recurrenceOverrides": {},
            "timeZone": "",
            "links": {
                "l1": {
                    "@type": "Link",
                    "href": ""
                }
            }
        }),
    );

    let loaded1 = sync.load_component(id.as_str()).expect("load 1");
    let saved1 = sync
        .save_component(&loaded1.icalendar, Some(id.as_str()))
        .expect("save 1");

    let loaded2 = sync.load_component(id.as_str()).expect("load 2");
    let saved2 = sync
        .save_component(&loaded2.icalendar, Some(id.as_str()))
        .expect("save 2");

    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must not bump revision for empty containers, empty timeZone, and empty strings"
    );

    let current_event = fixture.event(&id);
    let parsed_event = jmap_ical::ical_to_event(&loaded2.icalendar).expect("parse loaded2");
    let patch_diff = jmap_cal_sync::patch::diff(&current_event, &parsed_event);
    assert!(
        patch_diff.is_empty(),
        "diff after round-trip must be empty, found: {patch_diff:?}"
    );

    // A direct diff, where the edited event has explicit empty containers and empty
    // names.
    let mut edited_with_empty = parsed_event.clone();
    edited_with_empty.time_zone = Some("".to_owned());
    edited_with_empty.locations =
        Some([("1".into(), json!({"@type": "Location", "name": ""}))].into());
    edited_with_empty.virtual_locations = Some(
        [(
            "v1".into(),
            json!({
                "@type": "VirtualLocation",
                "uri": "https://meet.example.com/standup",
                "name": "",
                "features": {}
            }),
        )]
        .into(),
    );
    edited_with_empty.links = Some(
        [(
            "l1".into(),
            json!({
                "@type": "Link",
                "href": ""
            }),
        )]
        .into(),
    );
    edited_with_empty.alerts = Some(std::collections::BTreeMap::new());
    edited_with_empty.recurrence_overrides = Some(std::collections::BTreeMap::new());

    let direct_diff = jmap_cal_sync::patch::diff(&current_event, &edited_with_empty);
    assert!(
        direct_diff.is_empty(),
        "diff with empty containers must be empty, found: {direct_diff:?}"
    );
}

#[test]
fn diff_normalizes_empty_timezone_and_start_against_absent_baseline() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Planning", "2026-01-15T13:00:00");
    let mut current = fixture.event(&id);
    current.time_zone = None;

    let mut edited = current.clone();
    edited.time_zone = Some("".to_owned());

    let patch = jmap_cal_sync::patch::diff(&current, &edited);
    assert!(
        patch.is_empty(),
        "diff with empty timeZone against None must be empty, found: {patch:?}"
    );

    let mut current_empty_tz = current.clone();
    current_empty_tz.time_zone = Some("".to_owned());
    let mut edited_none_tz = current.clone();
    edited_none_tz.time_zone = None;

    let patch2 = jmap_cal_sync::patch::diff(&current_empty_tz, &edited_none_tz);
    assert!(
        patch2.is_empty(),
        "diff with None timeZone against empty timeZone must be empty, found: {patch2:?}"
    );

    // An empty `timeZone` on an event that has a zone patches null.
    let mut current_with_tz = current.clone();
    current_with_tz.time_zone = Some("Europe/Berlin".to_owned());
    let mut edited_cleared_tz = current_with_tz.clone();
    edited_cleared_tz.time_zone = Some("".to_owned());
    let patch3 = jmap_cal_sync::patch::diff(&current_with_tz, &edited_cleared_tz);
    assert_eq!(
        patch3.get("timeZone"),
        Some(&serde_json::Value::Null),
        "clearing timeZone via empty string must patch timeZone to null"
    );
}

#[test]
fn no_op_save_mints_no_patch_across_all_calendar_surfaces_matrix() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sprint Retrospective", "2026-06-15T10:00:00");
    let sync = fixture.sync();

    // Fill every mapped and unmapped property of the JSCalendar schema.
    fixture.patch(
        &id,
        json!({
            "title": "Sprint Retrospective & Roadmap Planning",
            "description": "Bi-weekly review of engineering progress and milestones.",
            "start": "2026-06-15T10:00:00",
            "timeZone": "Europe/Berlin",
            "duration": "PT1H30M",
            "status": "confirmed",
            "freeBusyStatus": "busy",
            "privacy": "private",
            "priority": 2,
            "showWithoutTime": false,
            "locations": {
                "l1": {
                    "@type": "Location",
                    "name": "Main Auditorium, Building B",
                    "description": "Floor 2, Room 201",
                    "timeZone": "Europe/Berlin"
                }
            },
            "virtualLocations": {
                "v1": {
                    "@type": "VirtualLocation",
                    "uri": "https://meet.example.com/retro",
                    "name": "Main Video Room",
                    "features": {"video": true, "audio": true},
                    "description": "PIN: 1234"
                }
            },
            "links": {
                "link1": {
                    "@type": "Link",
                    "href": "https://files.example.com/retro_agenda.pdf",
                    "contentType": "application/pdf",
                    "size": 102400,
                    "title": "Retro Agenda"
                }
            },
            "alerts": {
                "a1": {
                    "@type": "Alert",
                    "action": "display",
                    "trigger": {
                        "@type": "OffsetTrigger",
                        "offset": "-PT15M",
                        "relativeTo": "start"
                    }
                }
            },
            "keywords": {
                "Engineering": true,
                "Sprint": true,
                "Roadmap": true
            },
            "recurrenceRule": {
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "interval": 2,
                "byDay": [{"day": "mo"}]
            },
            "recurrenceOverrides": {
                "2026-06-29T10:00:00": {
                    "excluded": true
                }
            },
            "participants": {
                "p1": {
                    "@type": "Participant",
                    "name": "Vera Oldenburg",
                    "email": "vera@example.com",
                    "roles": {"owner": true, "attendee": true}
                }
            },
            "sequence": 4,
            "color": "#336699",
            "locale": "en-US",
            "customEventBag": "custom-event-data"
        }),
    );

    // The first load and save, as when the user opens and saves the event unchanged in
    // Evolution.
    let loaded1 = sync.load_component(id.as_str()).expect("load 1");
    let saved1 = sync
        .save_component(&loaded1.icalendar, Some(id.as_str()))
        .expect("save 1");

    let loaded2 = sync.load_component(id.as_str()).expect("load 2");
    let parsed_event2 = jmap_ical::ical_to_event(&loaded2.icalendar).expect("parse 2");
    let current_event = fixture.event(&id);

    let second_patch = jmap_cal_sync::patch::diff(&current_event, &parsed_event2);
    assert!(
        second_patch.is_empty(),
        "second diff on event after round-trip must mint no patch, found: {second_patch:?}"
    );

    let saved2 = sync
        .save_component(&loaded2.icalendar, Some(id.as_str()))
        .expect("save 2");
    assert_eq!(
        saved1.revision, saved2.revision,
        "second save must not change server revision for untouched event"
    );

    // All mapped and unmapped fields must stay unchanged on the server.
    let stored = fixture.event(&id);
    assert_eq!(
        stored.title.as_deref(),
        Some("Sprint Retrospective & Roadmap Planning")
    );
    assert_eq!(stored.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(stored.privacy.as_deref(), Some("private"));
    assert_eq!(stored.priority, Some(2));
    assert!(stored.participants.is_some() || stored.extra.contains_key("participants"));
    assert_eq!(stored.color.as_deref(), Some("#336699"));
    assert_eq!(stored.locale.as_deref(), Some("en-US"));
    assert_eq!(stored.extra.get("sequence"), Some(&json!(4)));
    assert_eq!(
        stored.extra.get("customEventBag"),
        Some(&json!("custom-event-data"))
    );
}

/// The invitation that a guest accepts from mail, as Evolution's invitation view gives it
/// to `receive_objects`. Its `UID` is the meeting's own, and the server already delivered
/// the meeting into this calendar. The local cache uses JMAP ids as keys, so it finds
/// nothing and creates the event.
const ACCEPTED_INVITATION: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
METHOD:REQUEST\r\n\
BEGIN:VEVENT\r\n\
UID:meeting-4711@example.org\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT1H\r\n\
ORGANIZER:mailto:organizer@example.org\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:organizer@example.org\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:guest@example.org\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn a_new_event_whose_uid_this_calendar_already_holds_is_saved_into_that_event() {
    let fixture = Fixture::start();
    let mut delivered = jmap_proto::calendars::CalendarEvent::simple(
        fixture.ours.clone(),
        "Planning",
        "2026-01-15T13:00:00",
        "PT1H",
    );
    delivered.uid = Some("meeting-4711@example.org".to_owned());
    let id = fixture
        .client()
        .event_create(&fixture.account_id, &delivered)
        .unwrap()
        .id
        .unwrap();
    let sync = fixture.sync();

    let saved = sync
        .save_component(ACCEPTED_INVITATION, None)
        .expect("the accept is saved, not refused as a second copy");

    assert_eq!(saved.uid, id.to_string(), "saved into the delivered copy");
    let (_, events) = sync.list_existing().unwrap();
    assert_eq!(events.len(), 1, "still one copy: {events:?}");
    let stored = fixture.event(&id);
    let statuses: Vec<_> = stored
        .participants
        .iter()
        .flatten()
        .filter_map(|(_, participant)| participant.get("participationStatus"))
        .collect();
    assert_eq!(statuses, [&json!("accepted"), &json!("accepted")]);
}

#[test]
fn a_new_event_whose_uid_only_another_calendar_holds_is_still_created_here() {
    let fixture = Fixture::start();
    let mut elsewhere = jmap_proto::calendars::CalendarEvent::simple(
        fixture.theirs.clone(),
        "Planning",
        "2026-01-15T13:00:00",
        "PT1H",
    );
    elsewhere.uid = Some("meeting-4711@example.org".to_owned());
    let theirs = fixture
        .client()
        .event_create(&fixture.account_id, &elsewhere)
        .unwrap()
        .id
        .unwrap();

    let result = fixture.sync().save_component(ACCEPTED_INVITATION, None);

    // This calendar does not edit the event in the other calendar. The create continues,
    // and a server that allows a `uid` only once per account refuses it.
    assert!(result.is_err(), "{result:?}");
    assert_eq!(
        fixture.event(&theirs).participants,
        None,
        "the other calendar's event is untouched"
    );
}

#[test]
fn an_edit_of_one_property_does_not_revert_a_concurrent_remote_edit_of_another() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"description": "old", "timeZone": "Europe/Berlin"}),
    );
    let sync = fixture.sync();
    let base = sync.load_component(id.as_str()).unwrap().icalendar;
    // A second client retitles and moves the event after this client loaded it.
    fixture.patch(
        &id,
        json!({"title": "Standup (moved)", "timeZone": "America/New_York"}),
    );

    let edited = base.replace("DESCRIPTION:old", "DESCRIPTION:new");
    assert_ne!(edited, base);
    sync.save_component_since(
        Some(Base {
            icalendar: &base,
            on_conflict: OnConflict::KeepLocal,
        }),
        &edited,
        Some(id.as_str()),
        Itip::Send,
    )
    .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.description.as_deref(), Some("new"));
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (moved)"),
        "the save reverted a title that the user did not edit"
    );
    assert_eq!(stored.time_zone.as_deref(), Some("America/New_York"));
}

#[test]
fn a_property_that_the_user_edited_takes_the_users_value_over_a_remote_edit() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let base = sync.load_component(id.as_str()).unwrap().icalendar;
    fixture.patch(&id, json!({"title": "Standup (remote)"}));

    let edited = base.replace("SUMMARY:Standup", "SUMMARY:Standup (local)");
    sync.save_component_since(
        Some(Base {
            icalendar: &base,
            on_conflict: OnConflict::KeepLocal,
        }),
        &edited,
        Some(id.as_str()),
        Itip::Send,
    )
    .unwrap();

    assert_eq!(fixture.event(&id).title.as_deref(), Some("Standup (local)"));
}

#[test]
fn a_new_custom_zone_still_brings_its_definition_when_saved_over_a_base() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"timeZone": "Europe/Berlin"}));
    let sync = fixture.sync();
    let base = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = base
        .replace("TZID=Europe/Berlin", "TZID=/example.com/Office")
        .replace(
            "BEGIN:VEVENT",
            "BEGIN:VTIMEZONE\r\nTZID:/example.com/Office\r\nBEGIN:STANDARD\r\n\
DTSTART:19700101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n\
END:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT",
        );
    sync.save_component_since(
        Some(Base {
            icalendar: &base,
            on_conflict: OnConflict::KeepLocal,
        }),
        &edited,
        Some(id.as_str()),
        Itip::Send,
    )
    .unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.time_zone.as_deref(), Some("/example.com/Office"));
    assert!(
        stored
            .time_zones
            .as_ref()
            .is_some_and(|zones| zones.contains_key("/example.com/Office")),
        "{stored:?}"
    );
}

#[test]
fn a_link_served_as_a_local_file_is_neither_shown_nor_sent_back() {
    let fixture = Fixture::start_with(MockServer::builder().calendar_event_link_uses_file_uri());
    let minutes = json!({
        "@type": "Link",
        "href": "https://files.example.com/minutes.txt",
    });
    let (id, sync) = points_at(&fixture, json!({"l1": agenda(), "l2": minutes.clone()}));
    let served = fixture.event(&id).links.expect("links");
    assert!(
        served
            .values()
            .all(|link| link["href"] == "file:///dev/zero"),
        "the mock must serve local-file links: {served:?}"
    );

    let icalendar = unfolded(&sync.load_component(id.as_str()).unwrap().icalendar);
    assert!(!icalendar.contains("ATTACH"), "{icalendar}");
    assert!(!icalendar.contains("file:"), "{icalendar}");
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    assert_ne!(edited, icalendar, "the summary was not found");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    // Read the store, because the mock rewrites the link addresses on every `CalendarEvent/get`.
    let stored = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        account
            .calendar_events
            .get(&id)
            .expect("the seeded event")
            .clone()
    };
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.links.expect("the save removed the links"),
        serde_json::from_value(json!({"l1": agenda(), "l2": minutes})).unwrap(),
        "the save changed or wrote back a local-file link"
    );
}
