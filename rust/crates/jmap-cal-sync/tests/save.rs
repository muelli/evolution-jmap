// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. The theme throughout is that saving a component must not
//! destroy what the component could not carry: the mapping keeps sixteen
//! properties of a JSCalendar event and drops the rest, so a save that
//! replaced properties wholesale would delete data the user never touched and
//! cannot even see.

mod common;

use common::Fixture;
use jmap_cal_sync::{SyncError, Unsendable};
use jmap_proto::calendars::NDay;
use serde_json::{Value, json};

/// The component Evolution hands to `save_component_sync` for a brand new
/// appointment: the `UID` is a name the local cache invented, not a server
/// identifier.
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
    // The listing agrees with what save reported.
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
    // What Evolution's appointment editor actually writes: DTEND, never
    // DURATION. An event saved from it used to reach the server with no
    // duration at all — RFC 8984's P0D — so the meeting the user scheduled for
    // an hour and a half was shared as a zero-length one.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace("DURATION:PT90M", "DTEND;TZID=Europe/Berlin:20260115T143000");

    let saved = fixture.sync().save_component(&icalendar, None).unwrap();

    let stored = fixture.event(&saved.uid.as_str().into());
    assert_eq!(stored.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT1H30M"));
    // And what EDS is handed back says the same, in the one spelling this
    // mapping writes.
    assert!(
        saved.icalendar.contains("\r\nDURATION:PT1H30M\r\n"),
        "{saved:?}"
    );
}

#[test]
fn a_new_all_day_event_reaches_the_server_as_one() {
    // What Evolution writes for an all-day appointment: DTSTART and DTEND as
    // DATE values. Without showWithoutTime the server — and every other client
    // reading from it — was told about a midnight appointment instead.
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
    // RFC 5545 §3.2.19 and RFC 8984 §4.1.5 agree that a day has no zone.
    assert_eq!(stored.time_zone, None);
    // And EDS gets the same event back, still without a time.
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
    // A day has no zone (RFC 8984 §4.1.5), and `CalendarEvent::simple` seeds
    // one, so it goes as part of making this event all-day.
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
    // A server may set showWithoutTime on an event no DATE value can hold — one
    // that starts at 09:00, or, as here, one carrying a zone. The component
    // shows it as timed, which loses the flag; the save must not read that loss
    // back as the user having cleared it.
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
    // `created` and `updated` (RFC 8984 §4.1.7, §4.1.8) are the server's record
    // of the event. They are drawn onto the component — as CREATED, and as the
    // DTSTAMP and LAST-MODIFIED RFC 5545 §3.8.7.2 makes the same instant — and
    // never read back off it, which is what this pins: an editor rewrites all
    // three from its own clock on every save, so a save that read them would
    // report the *client's* moment as when the server last changed the event.
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
    // A property no component we produce can carry. (`locations` was the
    // exemplar here until the place an event happens at became mapped,
    // `keywords` until the tags did, `freeBusyStatus` until the transparency
    // did, `priority` until the importance did and `useDefaultAlerts` until the
    // reminders did; the scheduling revision is still nowhere on a component.)
    //
    // The guest list beside it is the other half of the same rule, one step on:
    // it *is* drawn now — the ATTENDEE below — and still may not be written
    // back, because changing it means an iTIP message this backend does not
    // send. So it stands here as the property that survives a save it appears
    // on, not one it is missing from.
    let guest = json!({
        "@type": "Participant",
        "name": "Vera Example",
        "sendTo": {"imip": "mailto:vera@example.com"},
        "roles": {"attendee": true},
        "participationStatus": "accepted",
    });
    // And where the event is joined online, which is neither: it is drawn as the
    // CONFERENCE below *and* editable, but only one member at a time — the
    // `description` beside the URI has no room on the line, so it survives a
    // save the way an unmapped property does. Here nothing about it was edited,
    // so the whole entry has to come back untouched.
    let online = json!({
        "@type": "VirtualLocation",
        "name": "Team room",
        "description": "Ask Vera for the passcode",
        "uri": "https://meet.example.com/standup",
        "features": {"video": true},
    });
    // And what the event points at, which is the conference's case again: the
    // ATTACH below is drawn *and* editable, but only the address on it — a
    // Link's `title` (RFC 8984 §1.4.11) has no room on the line, and the media
    // type and size are the server's own description of the resource. Nothing
    // about it was edited here, so the whole entry has to come back untouched.
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
    // Unfolded first: RFC 5545 §3.1 splits a content line longer than 75 octets,
    // and this one is.
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

/// A component with its folds undone. RFC 5545 §3.1 splits a content line
/// longer than 75 octets, and a `CONFERENCE` carrying a label and a key is
/// longer than that — so an edit expressed as a text substitution has to name
/// the line the reader sees rather than the first fragment the emitter wrote.
fn unfolded(icalendar: &str) -> String {
    icalendar.replace("\r\n ", "")
}

/// An event with somewhere to join it online, and the sync that serves it.
fn joined_online(fixture: &Fixture, places: Value) -> (jmap_proto::Id, jmap_cal_sync::CalSync) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"virtualLocations": places}));
    let sync = fixture.sync();
    (id, sync)
}

/// The one virtual location the tests below start from: a link, a name, the
/// ways of taking part, and a note with no room on the `CONFERENCE` line.
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
    // The second property patched *into* rather than replaced, for the reason
    // `locations` was the first: a VirtualLocation says more than its line does.
    // Naming `virtualLocations` in the patch would replace the entry whole and
    // take the description with it, so the save names the one member the user
    // edited and everything beside it stays as the server had it.
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
    // The other two members a CONFERENCE shows: RFC 7986 §6.4's LABEL is the
    // VirtualLocation's `name`, and §6.3's FEATURE its `features`. The set goes
    // back replaced whole, which is safe exactly because `maps_virtual_locations`
    // has already refused the save for a set the line could not draw in full.
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
    // A LABEL the user deleted is `"virtualLocations/v1/name": null`: RFC 8620
    // §5.3 removes a property to mean "back to the default", and RFC 8984 §4.2.6
    // defaults `name` to the empty string. Storing "" instead would be a place
    // whose name is nothing, which is a different claim.
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
    // The conservative half of this mapping, and the reason is that a missing
    // line does not say who removed it. Evolution 3.52 has no UI for a
    // conference; whether its editor writes back a property it does not
    // understand is not something this repository can answer without a real
    // Evolution. Deleting the server's entry on that reading would destroy a
    // link the user never touched, so a save that names fewer conferences than
    // the server holds simply says nothing about the ones it does not name.
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
    // The other half of the same caution, and a rule RFC 8620 §5.3 also asks
    // for: a patch may only reach through objects that already exist. A line
    // carrying a key the server never chose — or none at all, which is what any
    // other client's component looks like — is therefore not an entry this save
    // can name, so it is left for the create path, where the whole property is
    // written at once.
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
    // The rule every conditionally-mapped property has: a drawing that left
    // something out is not a drawing the user can have edited. Here it is a way
    // of taking part outside RFC 7986 §6.3's vocabulary, which the CONFERENCE
    // line cannot carry — so the whole property is left alone, including the
    // entry whose URI the save really did change.
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
    // The create path writes the property whole — there is no server entry to
    // patch into — so a component that already names somewhere to join, which is
    // what an event copied out of another calendar looks like, arrives with it.
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

/// An event pointing at whatever external resources are passed, and the sync
/// that serves it.
fn points_at(fixture: &Fixture, links: Value) -> (jmap_proto::Id, jmap_cal_sync::CalSync) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"links": links}));
    (id, fixture.sync())
}

/// The one link the tests below start from: an address, the media type and size
/// the server knows for it, and the two members no `ATTACH` line has room for.
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
    // The third property patched *into* rather than replaced, for the reason the
    // other two are: a Link (RFC 8984 §1.4.11) holds a `cid`, a `rel` and a
    // `title` that no ATTACH line has room for, so naming `links` in the patch
    // would delete half of a resource the user was never shown. The save names
    // `links/<key>/href` under the key the line was drawn with, and everything
    // beside it stays as the server had it.
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
    // `href` is the only member of a Link a save ever names, and this is why:
    // `contentType` and `size` are the *server's* description of the resource —
    // §1.4.11 calls the size an estimate — not a field the user was offered. An
    // editor that rewrites the line without the parameters it has no UI for is
    // the ordinary case, and reading that as "the user cleared the media type"
    // would delete what the server knows on the first save of an unrelated edit.
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
    // The same caution a missing CONFERENCE gets, and for the same reason: a
    // line that is gone does not say who removed it. Evolution's editor keeps
    // attachments in a store of its own and writes what it kept, so a component
    // that names fewer resources than the server holds is as likely to be an
    // editor that dropped a line it had no URI for as a user who removed the
    // file. Reading it as a deletion would destroy a document nobody touched;
    // the cost of the other reading is that a removal made elsewhere comes back
    // on the next sync.
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
    // RFC 8620 §5.3 lets a patch reach only through objects that already exist,
    // so a line carrying a key the server never chose — or none at all, which is
    // what another client's component looks like — names no entry this save can
    // patch. Creating one instead is not available either: a resource the user
    // added is a file to upload as a blob, not an address the server can fetch.
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
    // A key outside RFC 8984 §1.4.4's `Id` grammar cannot ride on the line, so
    // the drawing of that entry reads back under an invented key — and an
    // invented key avoids only the keys the *document* names, so it can collide
    // with a key the server holds for an entry the drawing left out. Here it
    // does: `k1` is a resource with no address to draw. Patching `links/k1/href`
    // would give that entry the address of the agenda the user edited and lose
    // the edit itself, so the address the server stated is checked against the
    // one that was drawn, and an edit under a key this side invented is dropped.
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
    // The create path writes the property whole — there is no server entry to
    // patch into — so a component that already points somewhere, which is what
    // an event copied out of another calendar looks like, arrives with it.
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
    // Where Evolution keeps a file the user attached: a `file:` URI into its own
    // store. It is not an address anybody else could fetch, and the path names
    // the user's home directory — so it is not filed as a Link, and the event is
    // created without it. Sending the file means uploading it as a blob, which
    // this backend does not do yet.
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
    // The server's own `duration` can be one iCalendar has no room for: RFC 5545
    // §3.3.6 spells a negative length and RFC 8984 §1.4.6 does not, so a value
    // this way round is a value the mapping refuses to write — libical refusing
    // the content line would cost the whole component, and the appointment with
    // it. What must not follow is a save *clearing* it. The baseline a save diffs
    // against is the server's event put through the same rendering, so the length
    // is absent on both sides and an edit elsewhere leaves the server's value
    // where it was.
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
    // One level down, where the property is replaced whole rather than patched
    // key by key. An override the component cannot describe comes back as the
    // empty patch, so writing the map would delete the length the server holds —
    // the same reason a rule with a `byDay` leaves `recurrenceRule` untouched.
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    fixture.patch(
        &id,
        json!({"recurrenceOverrides": {"2026-01-20T09:00:00": {"duration": "-PT1H"}}}),
    );
    let sync = fixture.sync();

    // Placed by an RDATE, at the series' length: the occurrence is shown, and
    // the length it really has is not.
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
    // `rscale` has no place in the RecurrenceRule this crate models — a rule
    // counted in the Chinese calendar has no `RRULE` spelling libical reads — so
    // the RRULE the user edited is a narrower rule than the one on the server.
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
    assert_eq!(
        rules.extra.get("rscale"),
        Some(&json!("chinese")),
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

    // RFC 8984 §4.3.3's `until` is a local time in the event's own zone, so it
    // is drawn the way DTSTART is.
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("UNTIL=20260901T090000\r\n")
            || icalendar.contains("UNTIL=20260901T090000;"),
        "{icalendar}"
    );

    // RFC 5545 §3.3.10 asks for a UTC instant there instead, whenever DTSTART
    // names a zone — so that is what a conformant editor writes back, and what
    // an event imported from anywhere else carries: the same moment, two hours
    // earlier on the clock. Read as a local time it would tell the server the
    // series stops at 07:00 Zurich time, which is before the last occurrence
    // begins, so the day the user could still see would be gone from it.
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
    // The ordinary series, so that the refusal below is known to be about the
    // rule it cannot state rather than about recurrence on a create at all.
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
    // RFC 5545 §3.3.10 requires `UNTIL` to be a UTC instant wherever `DTSTART`
    // names a zone, and turning that instant into RFC 8984 §4.3.3's local time
    // needs a zone database `jmap-ical` deliberately does not carry — so the
    // value is kept as it was stated and `maps_recurrence_rule` reports that the
    // rule cannot be sent. This is the commonest such rule there is: every
    // "repeat until <date>" a conformant editor writes in a zoned calendar.
    //
    // An edit leaves `recurrenceRule` alone and the server's own rule stands
    // (see `a_series_end_restated_as_a_utc_instant_does_not_move_the_recurrence`).
    // A create has no rule to leave standing, so the save is refused instead:
    // sending it invites a strict server to reject the whole set, and a lenient
    // one to store a `until` that is no LocalDateTime — which this mapping then
    // cannot draw, so Evolution would show a single appointment while the
    // server ran a series.
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
    // The refusal above, carrying what the person it happens to has to be
    // told. What is left of it after the document's own `VTIMEZONE` became the
    // conversion (see below) is narrow — a zone the entry names and does not
    // define, or defines in a shape `jmap-ical`'s evaluator will not guess at —
    // and a message that blames "an end date stated as a UTC instant" now
    // describes the case that *works*. So the refusal names which instant and
    // which zone instead: the two facts that tell the user which appointment to
    // change, and tell whoever reads the bug report which zone definition to
    // look at.
    //
    // Two facts rather than a sentence, because the sentence is written where
    // it can be translated — `jmap_backend_cal::ops`, over this reason. The
    // instant is the value as it was *kept*, normalised out of the component's
    // `20260331T120000Z`, which is the form the user is shown.
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
    // The other half, and the one that survives a document doing everything
    // right: Berlin, defined, but with a transition rule written in a shape
    // `jmap-ical`'s zone evaluator refuses whole — here an `INTERVAL` other
    // than 1. The conversion is unavailable for the same reason as above and
    // the user gets the same answer, which is the point: one message covering
    // "not defined" and "defined unreadably", because from where the user sits
    // those are one problem with their calendar entry's time zone.
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

/// Berlin as libical writes it, and as every component Evolution hands a save
/// carries it: the `VTIMEZONE` RFC 5545 §3.6.5 says defines the `TZID` beside
/// it.
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
    // The refusal above is what is left when a document names a zone and does
    // not define it. A real one defines it, and then the UTC instant §3.3.10
    // requires converts into the local time §4.3.3 wants without any zone
    // database: the rules are in the file. So the commonest recurring save
    // there is — "repeat weekly until <date>" in a zoned calendar — reaches the
    // server as the series it is, rather than as an error the user has to work
    // around.
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
    // Two hours on, the last Sunday of March having passed two days earlier.
    assert_eq!(rules.until.as_deref(), Some("2026-03-31T14:00:00"));
}

#[test]
fn a_new_event_whose_rule_the_rrule_narrowed_is_not_created_at_all() {
    // The other half of the same refusal, and the one that is not about time
    // zones: RFC 7529's leap-month spelling is a month JSCalendar states only
    // beside the `rscale` this mapping drops, so the rule that would go out
    // names a month the server is entitled to reject.
    let fixture = Fixture::start();
    let icalendar = NEW_EVENT.replace(
        "DURATION:PT90M",
        "DURATION:PT90M\r\nRRULE:FREQ=YEARLY;BYMONTH=5L",
    );

    let failure = fixture.sync().save_component(&icalendar, None).unwrap_err();

    // And it must not be reported as a time-zone problem: this rule has no end
    // at all, so a refusal naming the event's zone would send the user to
    // change something that is not what stopped the save.
    assert_eq!(
        failure_reason(&failure),
        &Unsendable::Recurrence,
        "a refusal that is not about the zone must not name one"
    );
}

/// The reason a refused save carries, or a panic naming what came instead.
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
    // Adding the Thursday, which is what the appointment editor's recurrence
    // page does to the RRULE.
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
    // Moving it to the last day of the month, which is what the appointment
    // editor's recurrence page writes for "on the last day".
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
    // Adding the second half-year, which is what the appointment editor's
    // recurrence page writes for a yearly series in two months.
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
    // Adding the last day of the year, which is the negative form RFC 8984
    // §4.3.3 counts back from 31 December with.
    let edited = icalendar.replace("BYYEARDAY=1", "BYYEARDAY=1,-1");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_year_day.as_deref(), Some(&[1, -1][..]));
}

#[test]
fn a_day_of_the_year_the_rrule_should_not_carry_is_not_sent() {
    // `FREQ=MONTHLY;BYYEARDAY=100` is a rule RFC 5545 §3.3.10 does not admit — a
    // month is not a period a day of the year sits inside — and neither calcard
    // nor libical judges it, so the check is on the way out: `recurrenceRule` goes
    // to the server replaced whole, and one part it is entitled to reject would
    // cost every other edit in the save.
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
    // A month iCalendar can only name under RFC 7529's `RSCALE` (RFC 8984
    // §4.3.3's `5L`). calcard carries the token rather than judging it, so the
    // check is on the way out: `recurrenceRule` goes to the server replaced
    // whole, and one part it is entitled to reject would cost every other edit in
    // the save.
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
    // Counting the weeks from Saturday instead, which is what the appointment
    // editor's recurrence page writes when the calendar's week start changes.
    let edited = icalendar.replace("WKST=SU", "WKST=SA");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.first_day_of_week.as_deref(), Some("sa"));
}

#[test]
fn a_day_no_week_starts_on_is_not_sent() {
    // A `firstDayOfWeek` outside RFC 8984 §4.3.3's closed vocabulary is one no
    // `WKST` can say, and libical refuses a component carrying `WKST=XX` outright
    // — so the rule is shown without it, and a save must not write the property
    // back: `recurrenceRule` goes to the server replaced whole, so the day would
    // be dropped from the server's own rule by a save that never touched the
    // recurrence.
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
    // So an edit to the recurrence is an edit to a rule the user was shown in
    // part, and is dropped whole rather than sent as the narrower rule it is.
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
    // Adding the last week of the year, which is the negative form RFC 8984
    // §4.3.3 counts back from the end of the year with.
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
    // `FREQ=MONTHLY;BYWEEKNO=20` is a rule RFC 5545 §3.3.10 does not admit — it
    // admits `BYWEEKNO` beside `YEARLY` and nothing else — and neither calcard nor
    // libical judges it, so the check is on the way out: `recurrenceRule` goes to
    // the server replaced whole, and one part it is entitled to reject would cost
    // every other edit in the save.
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
    // A `byWeekNo` outside RFC 5545's `ordwk` is one no `BYWEEKNO` this mapping
    // will write can carry — 54 is a week no year has — so the rule is shown
    // without it, and a save must not write the property back: `recurrenceRule`
    // goes to the server replaced whole, so the week would be dropped from the
    // server's own rule by a save that only narrowed it.
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
    // So an edit to the recurrence is an edit to a rule the user was shown in
    // part, and is dropped whole rather than sent as the narrower rule it is.
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
    // Adding the first Friday of the month, which is the positive form counting
    // from the start of the set the `BYDAY` expands to.
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
    // `FREQ=MONTHLY;BYSETPOS=2` is a rule RFC 5545 §3.3.10 does not admit —
    // `BYSETPOS` MUST only be used together with another `BYxxx` part — and
    // libical keeps it rather than judging it, so the check is on the way out.
    // Sent, it would name a series whose second-and-only occurrence per month
    // does not exist.
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
    // The mirror of the above, in the direction that loses data. A
    // `bySetPosition` the server holds with no other `by*` beside it is one no
    // `RRULE` this mapping writes can carry, so the rule is shown without it —
    // and `recurrenceRule` goes back replaced whole, so a save that only
    // narrowed the rule would delete the position from the server's own copy.
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
    // A second standup after lunch — the hours are a set, so the added one goes
    // out beside the one that was there.
    let edited = icalendar.replace("BYHOUR=9", "BYHOUR=9,14");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_hour.as_deref(), Some(&[9, 14][..]));
}

#[test]
fn an_hour_no_day_has_is_not_sent() {
    // 24 is outside RFC 5545 §3.3.10's `hour`, and libical answers such a rule by
    // dropping the whole `RRULE` — so the check is on the way out, before a rule
    // the server might keep and no reader can expand reaches it.
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
    // The direction that loses data. The server's own `byHour` is one this mapping
    // *can* write — so what has to be checked is that it goes back out again
    // rather than being dropped by a save that touched the rule for another
    // reason, since `recurrenceRule` is replaced whole.
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
    // A second poll on the half hour — both parts are sets, so the added value
    // goes out beside the one that was there.
    let edited = icalendar.replace("BYMINUTE=0", "BYMINUTE=0,30");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rule.unwrap();
    assert_eq!(rules.by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules.by_second.as_deref(), Some(&[0][..]));
}

#[test]
fn the_sixtieth_second_is_sent_and_the_sixtieth_minute_is_not() {
    // RFC 5545 §3.3.10's `seconds` runs to 60 and its `minutes` only to 59, and
    // libical answers a rule naming the sixtieth minute by dropping the whole
    // `RRULE` — so the check is on the way out, before a rule the server might
    // keep and no reader can expand reaches it. The leap second, in the same save,
    // is a value that must *not* be caught by that check.
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
    // The direction that loses data: `recurrenceRule` is replaced whole, so a
    // save that touched the rule for another reason has to carry the minutes and
    // seconds the server already held back out with it.
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
    // `FREQ=WEEKLY;BYMONTHDAY=15` is a rule RFC 5545 §3.3.10 does not admit, and
    // calcard hands it back rather than judging it — so, as with an ordinal
    // weekday, the check is on the way out: `recurrenceRule` goes to the server
    // replaced whole, and one part it may reject would cost every other edit in
    // the save.
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
    // `FREQ=WEEKLY;BYDAY=2MO` is a rule RFC 5545 §3.3.10 does not admit — an
    // ordinal needs a month or a year to count within. calcard hands it back
    // rather than judging it, so the check is on the way out, like the series'
    // own `timeZone`: `recurrenceRule` goes to the server replaced whole, and
    // one part it is entitled to reject would cost every other edit in the save.
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

/// A daily event on the server, so that the tests below have a recurrence to
/// name single instances of.
fn seed_daily(fixture: &Fixture) -> jmap_proto::Id {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRule":
            {"@type": "RecurrenceRule", "frequency": "daily"}}),
    );
    id
}

/// The component with `line` inserted ahead of its `END:VEVENT`, which is what
/// deleting one occurrence in Evolution amounts to.
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
    // The rule it is an exception to is untouched.
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

    // Removing the property is how a PatchObject says "back to the default",
    // which for recurrenceOverrides is no named instances at all.
    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_edited_on_its_own_survives_an_edit_to_another_instance() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    // An override that changes the instance is a VEVENT of its own in the
    // component, so deleting a *different* occurrence has to leave it standing.
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

/// The component with `vevent` appended inside its envelope, which is what
/// editing one occurrence in Evolution amounts to: a second instance of the
/// same uid, carrying the `RECURRENCE-ID` of the day it replaces.
fn with_instance(icalendar: &str, vevent: &str) -> String {
    icalendar.replace("END:VCALENDAR\r\n", &format!("{vevent}END:VCALENDAR\r\n"))
}

#[test]
fn editing_one_occurrence_reaches_the_server_as_a_patched_override() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    // A detached instance is a whole component, not a patch: Evolution clones
    // the series and edits that, so what it restates unchanged — here the
    // status, and the length it states as a DTEND — is not an edit, and only
    // the moved start and the new title reach the server.
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
    // The series is what it was: one daily rule, and the title the other
    // occurrences still carry.
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
    // Deleting the detached instance is how Evolution says "this occurrence is
    // like the others again".
    let detached = icalendar.rfind("BEGIN:VEVENT").expect("two instances");
    let series = format!("{}END:VCALENDAR\r\n", &icalendar[..detached]);
    sync.save_component(&series, Some(id.as_str())).unwrap();

    assert_eq!(fixture.event(&id).recurrence_overrides, None);
}

#[test]
fn an_instance_the_server_says_is_not_excluded_stays_spelled_that_way() {
    // RFC 8984 §4.3.4 defaults `excluded` to false, so an override that says so
    // out loud and one that says nothing are the same instance, and the
    // component has the same single RDATE for both. Re-saving it must not
    // rewrite the property merely because the spelling comes back shorter —
    // the baseline is the round trip, not the server's own event.
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

    // JSCalendar has no way to say "no start", so a component whose DTSTART
    // the mapping cannot read must not be turned into one that says it.
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
    // Three things the component cannot say exactly: a time zone spelled the
    // other legal way, a recurrence part the RRULE has to drop — here a
    // `bySetPosition` with no other `by*` beside it to select from — and an
    // instance edited on its own, which an RDATE can place but not describe.
    // None of them is an edit, and none may look like one.
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

/// The `TZID` Evolution puts on every zoned component it saves: libical names
/// its builtin zones with a solidus-prefixed identifier of its own, and the
/// appointment editor sets the start with the zone object, so this is what
/// comes back even for a component we handed out spelling the zone plainly.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

/// The `VTIMEZONE` libical writes beside it, trimmed to the two lines that
/// matter here; `X-LIC-LOCATION` is its record of which IANA zone this is.
///
/// The envelope the backend builds does carry it:
/// `marshal::icalendar_from_instances` copies in a definition for every zone the
/// instances refer to, which is the other half of the answer these tests are the
/// mapping's half of. `jmap-functional`'s calendar leg is what says the two
/// halves meet through real EDS — this crate can only supply the identifier by
/// hand.
const LIBICAL_VTIMEZONE: &str = "BEGIN:VTIMEZONE\r\n\
TZID:/freeassociation.sourceforge.net/Europe/Berlin\r\n\
X-LIC-LOCATION:Europe/Berlin\r\n\
BEGIN:STANDARD\r\nTZNAME:CET\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
DTSTART:19701025T030000\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n";

/// Respelling a zone is not changing it. Every save of a zoned appointment
/// arrives with libical's own identifier in place of the name we wrote, and
/// reading that as an edit would put a string into `timeZone` that RFC 8984
/// §1.4.9 only admits beside a `timeZones` definition — a value a server is
/// entitled to reject, taking the user's real edits down with it.
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

/// The same respelling on the other side of a real move: the zone the user
/// picked has to reach the server as the name JSCalendar spells it with.
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

/// A zone only the document knows about, on a *new* appointment.
///
/// This is the one place where leaving the zone alone is not available. On an
/// edit the server already holds a zone and keeping it is the conservative
/// answer; on a create there is nothing to keep, so a zone that cannot be stated
/// files the appointment floating — a wall-clock time in no particular zone,
/// which every reader resolves to its own.
///
/// RFC 8984 §1.4.9 gives such a zone a way to be stated all the same: the custom
/// identifier, sent beside the §4.7.2 `timeZones` entry that defines it. The
/// definition is the document's own — read off the `VTIMEZONE` an Exchange
/// invitation or another client's `.ics` carries — so the event goes out saying
/// which zone it is in and what that zone does, rather than saying nothing.
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
    // And EDS gets the event back with the definition still beside it, because
    // that is the only thing that makes the `TZID` resolvable.
    assert!(
        saved.icalendar.contains(&format!("TZID:{CUSTOM_TZID}\r\n")),
        "{saved:?}"
    );
}

/// A zone the document leaves undefined still files the appointment floating: a
/// solidus-prefixed identifier with no `VTIMEZONE` beside it is a reference to
/// nothing, and a server is entitled to refuse the whole `CalendarEvent/set` for
/// it — which would cost the user the appointment rather than its zone.
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

/// A series in a zone that cannot be sent, over an occurrence in one that can.
///
/// The series' `TZID` is a Windows name, so the create files the series
/// floating — there is no `TimeZoneId` to state it by. The occurrence the user
/// moved is in a different zone, one the document *defines*, which RFC 8984
/// §1.4.9 does admit; dropping `timeZones` wholesale along with the series'
/// zone took that definition with it and left the override's identifier
/// dangling, which is the shape a server may refuse the whole
/// `CalendarEvent/set` for.
///
/// So the map is pruned to what is still referred to, not emptied: the series
/// goes floating, and the occurrence keeps the zone it was moved into.
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

/// The identifier a server invents for a zone no database names — RFC 8984
/// §1.4.9's second form — and the `VTIMEZONE` that defines it, as a document
/// written elsewhere carries the pair.
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

/// A zone nothing in the document explains: a Windows name from Exchange, and
/// libical's own identifier with no `VTIMEZONE` beside it to translate it. The
/// backend's envelope now defines the zones its components name, so the second
/// shape no longer reaches this crate from Evolution — but a document is not
/// only ever built there, and an identifier no zone database knows (the first
/// shape) still arrives undefined however careful the envelope is.
///
/// Neither is a value JSCalendar can carry, so neither is sent; the server
/// keeps the zone it had, which is the zone the component was showing. The cost
/// stays on the record for the case that remains: a zone the user really did
/// change to something unresolvable is not seen either.
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

/// The same value on a create, where there is no server zone to keep. The
/// appointment is filed floating rather than not at all: a wall-clock time
/// with no zone shows correctly for the user who typed it, and an event the
/// server refused shows nothing.
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

/// And a create carrying libical's spelling, which is what Evolution actually
/// hands the backend for a brand-new zoned appointment.
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

/// One occurrence moved into another zone, which is what Evolution's appointment
/// editor writes when the user opens a single day of a series and changes its
/// time zone: a detached component whose own `DTSTART` carries a `TZID` the
/// series' does not. The zone has to reach the server inside the override, or the
/// wall-clock time the user typed is resolved against the series' zone and the
/// occurrence lands hours away.
#[test]
fn moving_one_occurrence_to_another_zone_arrives_under_its_iana_name() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    let icalendar = sync
        .load_component(id.as_str())
        .unwrap()
        .icalendar
        // The definition the zone the instance moves to needs, once, ahead of
        // the series: two copies would be two zones under one TZID.
        .replacen(
            "BEGIN:VEVENT",
            &format!("{LIBICAL_VTIMEZONE}BEGIN:VEVENT"),
            1,
        );
    // The series is in UTC; the instance goes to Berlin, spelled the way libical
    // spells its builtin zones. What the instance restates unchanged — the title
    // and the status — is not an edit, so only the zone and the start arrive.
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

/// And the zone no document explains, one level down from
/// [`a_zone_the_document_could_not_name_leaves_the_servers_alone`]:
/// `recurrenceOverrides` is replaced whole, so an entry holding a value RFC 8984
/// §1.4.9 does not admit cannot be sent at all — the server would be entitled to
/// reject the whole `CalendarEvent/set` and take the user's other edits with it.
/// The property is left alone and the rest of the save still arrives.
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

/// One occurrence moved into a zone the *document* defines — the other half of
/// [`an_occurrences_unnameable_zone_leaves_the_overrides_alone`], and the half
/// that has to arrive.
///
/// This is the user who drags one day of a series into the zone an invitation
/// brought: Evolution files that `VTIMEZONE` in the calendar's own timezone
/// store, the backend's envelope copies it back beside the components that name
/// it, and what reaches this crate is a custom identifier *with* its RFC 8984
/// §4.7.2 definition. §1.4.9 admits that pair, so the save has something to send
/// — and must send both halves: the identifier alone is a dangling reference the
/// server may reject the whole `CalendarEvent/set` over.
///
/// The definition goes out as its own entry, `timeZones/<pointer>`, not as the
/// property replaced whole. That is what keeps the module's rule where a
/// `VTIMEZONE` cannot show a zone's `aliases`, `url` or `validUntil`: an entry
/// the server did not have is added, and nothing it did have is touched — see
/// [`an_occurrence_moved_into_a_zone_the_server_defines_leaves_the_definition_alone`].
#[test]
fn moving_one_occurrence_into_a_zone_only_the_document_defines_sends_the_definition() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    let sync = fixture.sync();

    // The definition ahead of the series, once: two copies would be two zones
    // under one `TZID`.
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

/// The same move onto an identifier the **server** already defines, which is
/// what a second edit of the same occurrence is.
///
/// Nothing needs adding, and nothing may be written: the server's own entry may
/// carry an `url`, a `validUntil` or a set of `aliases`, none of which a
/// `VTIMEZONE` has room for and none of which the user was ever shown. So the
/// override goes out naming the zone and the definition beside it is left
/// exactly as it stood.
#[test]
fn an_occurrence_moved_into_a_zone_the_server_defines_leaves_the_definition_alone() {
    let fixture = Fixture::start();
    let id = seed_daily(&fixture);
    // A definition richer than any document can state, filed under the same
    // identifier the component below names.
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

/// And the whole series moved into a zone only the document defines, which is
/// the same pair one level up: the user changes the appointment's time zone to
/// the one an invitation brought, and `timeZone` and its `timeZones` entry have
/// to travel together or the identifier reaches the server naming nothing.
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

/// A second identifier arriving at an event that already has a `timeZones` map —
/// the case the pointer path exists for.
///
/// Where the server holds no map at all the property has to be written whole
/// (RFC 8620 §5.3 wants every path segment before the last to exist already),
/// and nothing is at risk because there was nothing there. Where it *does* hold
/// one, writing the property would delete the zone the series is in — which is
/// the definition the appointment's own clock resolves against, so the whole
/// event would go unresolvable to add one occurrence's zone. So the entry goes
/// in under its own pointer and the map keeps everything else, `url` and all.
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

    // The series keeps the zone the server named; the occurrence moves into the
    // second one, which only this document defines.
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

/// A second identifier for a zone on the other side of the Atlantic, so that a
/// definition arriving under it cannot be [`CUSTOM_VTIMEZONE`] by another name.
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

/// An event with one place, keyed the way a server keys it, and the component
/// Evolution is shown for it.
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
    // The event had none, so RFC 8620 §5.3 leaves nowhere to patch into: the
    // property is written whole, under a key the component invented.
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
    // The point of patching `locations/<key>/name` rather than replacing the
    // property: a `LOCATION` line is one string, and the place the user renamed
    // also has coordinates, a description and a zone that were never on it.
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
    // Evolution's appointment editor writes the LOCATION afresh, so the
    // X-JMAP-KEY the component was shown with may not come back. The name is
    // what the diff compares, and the key it patches is the server's own.
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
    // Nothing is left of the entry, and `maps_locations` has already said there
    // is no second place that would be stranded, so the property goes.
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
    // One LOCATION line, two places: the user was never shown the second, so
    // an edit to the first is not theirs to have made either.
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

/// An event tagged the way a server tags it, and the component Evolution is
/// shown for it.
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
    // What Evolution's "Categories…" button writes. Unlike a place, the set is
    // shown whole, so it goes back replaced whole — no key to patch into.
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
    // A PatchObject removes a property to say "back to the default", and RFC 8984
    // §4.2.9's default is no keywords at all. An empty map would be a different
    // thing to store and to send back.
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
    // RFC 8984 §1.4.3 has every value of a Set be `true`; this server said
    // otherwise, so the tag never reached the CATEGORIES line. The property goes
    // back replaced whole, so a save that wrote only what the line showed would
    // delete the tag the user never saw — and one that refused to write at all
    // would drop the edit they did make. The tag is carried onto the set
    // instead, exactly as the server stated it: an unshown tag is not the
    // user's to delete, and its odd value is the server's word to keep.
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
    // Emptying the field deletes the tags it showed and nothing else. A `null`
    // here would delete the tag that had no line to be shown on, which is the
    // one thing the user cannot have meant by clearing a field it was not in.
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
    // The one place the two sides name the same tag. The user's word wins: they
    // typed it into a field that says nothing but "filed under", so they mean it
    // set, whatever the server had against that name before. Carrying the
    // server's value back over it would take the tag the user just typed and
    // quietly unset it.
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
    // The tag put back is the tag that was already there, so the set the save
    // would write is the set the server holds — and a patch naming it would undo
    // a concurrent edit on another client for no reason at all. The property
    // must be left unnamed, not merely written back unchanged.
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
    // One occurrence of a tagged series filed elsewhere: EDS keeps the master and
    // adds a VEVENT carrying its RECURRENCE-ID, and the CATEGORIES on that
    // component are what that occurrence is now filed under — replacing the
    // series' set for that instance rather than adding to it, which is what a
    // PatchObject naming `keywords` means.
    let fixture = Fixture::start();
    let (id, _) = tagged(&fixture, json!({"offsite": true}));
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Everything the series states, restated but for the tags — an instance is
    // compared against the series property by property, so a line left off here
    // would be an edit to that property and not to the filing.
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
    // The `a_tag_the_component_could_not_show_leaves_the_whole_set_alone` rule one
    // level down. This instance's set holds an entry RFC 8984 §1.4.3 does not
    // admit, so the occurrence could only be placed by a bare RDATE — and
    // `recurrenceOverrides` goes back replaced whole, so sending what was drawn
    // would delete the tag the user never saw along with the whole override.
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
    // CATEGORIES is a value list, so the escaping is what keeps one tag from
    // becoming two on the way through the component and back.
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

/// An event reminded the way a server reminds, and the component Evolution is
/// shown for it.
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

/// The reminder a server states: a message a quarter of an hour before the
/// appointment.
fn quarter_of_an_hour_before() -> serde_json::Value {
    json!({
        "@type": "Alert",
        "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
        "action": "display",
    })
}

/// The `VALARM` Evolution's own editor writes for such a reminder: no RFC 9074
/// `UID`, an `X-EVOLUTION-ALARM-UID` of its own, and the summary repeated as the
/// text to display.
const EVOLUTION_VALARM: &str = "BEGIN:VALARM\r\n\
X-EVOLUTION-ALARM-UID:20260115T090000Z-4711\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Standup\r\n\
TRIGGER;VALUE=DURATION;RELATED=START:-PT15M\r\n\
END:VALARM\r\n";

#[test]
fn setting_a_reminder_reaches_the_server_as_an_alert() {
    // What Evolution's "Reminder" control writes: a VALARM inside the VEVENT,
    // which is the first mapped property that is a component rather than a line.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    let edited = icalendar.replace("END:VEVENT", &format!("{EVOLUTION_VALARM}END:VEVENT"));
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    assert_eq!(
        fixture.event(&id).alerts,
        // Keyed by the one this mapping invents: the component named no id the
        // `alerts` map could use.
        Some([("a1".to_owned(), quarter_of_an_hour_before())].into())
    );
}

#[test]
fn moving_a_reminder_sends_the_whole_set_under_the_servers_own_key() {
    // Unlike a place, an alert is not patched into: the whole map goes back. What
    // must survive that is the key the server chose, which rides on the VALARM's
    // RFC 9074 UID — a save under a different key would leave the user with the
    // same reminder twice.
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
    // A PatchObject removes a property to say "back to the default", and RFC 8984
    // §4.5.2's default is no alerts at all — an empty map would be a different
    // thing to store.
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
    // RFC 9074 §6.1's ACKNOWLEDGED says the user has already dismissed this
    // reminder, and the VALARM this mapping writes does not carry it — so the
    // alert was never drawn, and a set replaced whole would un-dismiss it along
    // with deleting what the user never saw.
    let fixture = Fixture::start();
    let mut dismissed = quarter_of_an_hour_before();
    dismissed["acknowledged"] = json!("2026-01-15T08:46:00Z");
    let alerts = json!({"k1": quarter_of_an_hour_before(), "k2": dismissed});
    let (id, icalendar) = reminded(&fixture, alerts.clone());
    assert_eq!(
        icalendar.matches("BEGIN:VALARM").count(),
        1,
        "the dismissed reminder was drawn\n{icalendar}"
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
fn setting_a_reminder_on_one_occurrence_reaches_the_server_as_an_override() {
    // One occurrence of a reminded series reminded differently: EDS keeps the
    // master and adds a VEVENT carrying its RECURRENCE-ID, and the VALARMs inside
    // that component are the reminders that occurrence now has — replacing the
    // series' set for that instance rather than adding to it, which is what a
    // PatchObject naming `alerts` means.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Everything the series states, restated but for the reminder — an instance is
    // compared against the series property by property, so a line left off here
    // would be an edit to that property and not to the reminder. The alarm keeps
    // the server's own key on its RFC 9074 UID: an hour earlier is the same
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
    // The `a_reminder_the_component_could_not_show_leaves_the_whole_set_alone`
    // rule one level down. This instance's set holds a reminder the user has
    // already dismissed, which no VALARM says, so the occurrence could only be
    // placed by a bare RDATE — and `recurrenceOverrides` goes back replaced whole,
    // so sending what was drawn would un-dismiss that reminder along with deleting
    // the whole override.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    let mut dismissed = quarter_of_an_hour_before();
    dismissed["acknowledged"] = json!("2026-01-22T08:46:00Z");
    let overrides = json!({"2026-01-22T09:00:00": {"alerts": {"k2": dismissed}}});
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
    // `an_event_that_takes_the_default_reminders_keeps_them` one level down, and
    // the reason `alerts` is the one restated property whose coverage the *series*
    // decides: RFC 8984 §4.5.1's `useDefaultAlerts` is not a property an override
    // may restate, so it is the series' answer for every instance, and an
    // occurrence's own reminders are ignored exactly as the series' are. Nothing is
    // drawn for them, so a save must leave the property alone rather than replace
    // it with what was drawn.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    // The override renames the occurrence *and* states reminders for it, so the
    // instance is drawn — and what the user then edits is the title on that
    // component, which is what makes `recurrenceOverrides` go back replaced whole.
    // With the reminders called covered, that replacement is what would delete
    // them.
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

    // Nothing about the override reaches the server — including the rename the
    // user just made, which is the cost of the property going back whole or not at
    // all: one entry that cannot be stated holds the rest of them still.
    let stored = fixture.event(&id);
    assert_eq!(
        stored.recurrence_overrides,
        Some(serde_json::from_value(overrides).unwrap()),
        "a property nothing reads must not be written"
    );
}

#[test]
fn an_event_that_takes_the_default_reminders_keeps_them() {
    // RFC 8984 §4.5.1: with `useDefaultAlerts` true it is the user's own default
    // reminders that fire and the `alerts` property is ignored, so there is
    // nothing to show and nothing a save could usefully write. Honouring a
    // reminder added here would take a save that cleared the flag too, which this
    // mapping does not do.
    let fixture = Fixture::start();
    let (id, _) = reminded(&fixture, json!({"k1": quarter_of_an_hour_before()}));
    fixture.patch(&id, json!({"useDefaultAlerts": true}));
    let sync = fixture.sync();
    // Re-read after the flag was set: the reminder was drawn before it, and is
    // not now.
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
    assert_eq!(stored.extra.get("useDefaultAlerts"), Some(&json!(true)));
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
    );
}

/// An event whose transparency the server states, and the component Evolution is
/// shown for it.
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
    // Evolution's "Show Time as: Free", which is a TRANSP line on the component
    // and RFC 8984 §4.4.2's `freeBusyStatus` on the server. The event the mock
    // holds says nothing about it, so this is also the case of a property the
    // save creates rather than changes.
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
    // A PatchObject removes a property to say "back to the default", and RFC 8984
    // §4.4.2's default is busy — which is also what iCalendar means by a VEVENT
    // with no TRANSP on it (RFC 5545 §3.8.2.7), so the two agree about what the
    // user just asked for.
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
    // JSCalendar's vocabulary is closed and this server answered outside it, so
    // no TRANSP line was drawn. The baseline is what keeps that from reading as
    // the user clearing the field: the server's own event goes through the same
    // rendering, loses the value on both sides, and the save sends nothing.
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
    // "Show Time as: Free" on a single occurrence: EDS keeps the master and adds
    // a VEVENT carrying its RECURRENCE-ID, and the transparency on that component
    // is the one property of it that differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Everything the series states, restated — an instance is compared against
    // the series property by property, so a line left off here would be an edit
    // to that property and not to the transparency.
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

/// An event whose importance the server states, and the component Evolution is
/// shown for it.
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
    // `priority` on the server, which are the same integer. The event the mock
    // holds says nothing about it, so this is also the case of a property the save
    // creates rather than changes.
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
    // A PatchObject removes a property to say "back to the default", and RFC 8984
    // §4.4.1's default is 0 — which is also what RFC 5545 §3.8.1.9 means by a
    // VEVENT with no PRIORITY on it, so the two agree about what the user just
    // asked for.
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
    // The range both formats share is 0..=9 and this server answered outside it,
    // so no PRIORITY line was drawn. The baseline is what keeps that from reading
    // as the user clearing the field: the server's own event goes through the same
    // rendering, loses the value on both sides, and the save sends nothing.
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
    // One occurrence of a series marked important: EDS keeps the master and adds a
    // VEVENT carrying its RECURRENCE-ID, and the priority on that component is the
    // one property of it that differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Everything the series states, restated — an instance is compared against
    // the series property by property, so a line left off here would be an edit
    // to that property and not to the priority.
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

/// An event whose classification the server states, and the component Evolution
/// is shown for it.
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
    // Evolution's Options ▸ Classification ▸ Private, which is a CLASS line on the
    // component (RFC 5545 §3.8.1.3) and RFC 8984 §4.4.3's `privacy` on the server.
    // The event the mock holds says nothing about it, so this is also the case of a
    // property the save creates rather than changes.
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
    // The case Evolution's appointment editor makes routine: it writes CLASS on
    // every save from its Classification menu, and public is what that menu
    // defaults to. So the component comes back stating the default explicitly, and
    // the save must send *nothing* — which it only can because the baseline is
    // rendered with the line too. That is the whole reason `CLASS:PUBLIC` is
    // written out rather than left off as the default it also is: were it left
    // off, the baseline would differ from the component on every save of a public
    // event, forever, and each one would carry a redundant `"privacy": "public"`.
    //
    // `get_changes` is the witness. A save that patches nothing never reaches the
    // server, so the account state does not move; one that patches the default
    // back in does, and the event turns up in the delta.
    //
    // The component is shaped the way the editor leaves it — carrying a CLASS
    // line whether or not the rendering already put one there — rather than saved
    // back verbatim. Saving the rendering verbatim would pass no matter what the
    // writer does, since both sides of the diff would then agree by construction;
    // it is the editor's line meeting a baseline that lacks it that costs a patch.
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
    // A PatchObject removes a property to say "back to the default", and RFC 8984
    // §4.4.3's default is public — which is also what RFC 5545 §3.8.1.3 means by a
    // VEVENT with no CLASS on it, so the two agree about what the user just asked
    // for.
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
    // RFC 8984 §4.4.3 leaves the vocabulary open and this server answered outside
    // the three values iCalendar can spell, so no CLASS line was drawn. The
    // baseline is what keeps that from reading as the user making the event
    // public: the server's own event goes through the same rendering, loses the
    // value on both sides, and the save sends nothing. Which matters more here
    // than for the other closed vocabularies — the value being dropped is the one
    // that says who may read the event.
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
    // One occurrence of a series marked private: EDS keeps the master and adds a
    // VEVENT carrying its RECURRENCE-ID, and the classification on that component
    // is the one property of it that differs.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(&id, json!({"recurrenceRule":{"frequency": "weekly"}}));
    let sync = fixture.sync();
    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;

    // Everything the series states, restated — an instance is compared against
    // the series property by property, so a line left off here would be an edit
    // to that property and not to the classification.
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

    // Unmodeled links and alerts are preserved
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

    // Links are preserved
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

    // Participants are preserved intact
    let parts = stored.participants.expect("participants");
    assert_eq!(parts["alice"]["name"], json!("Alice Example"));
    assert_eq!(parts["alice"]["roles"]["chair"], json!(true));
    assert_eq!(parts["bob"]["participationStatus"], json!("declined"));

    // Locations and links are preserved intact
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

    // Remove the entire VALARM component
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

    // Modify SUMMARY on the master VEVENT
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

    // Remove EXDATE line
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

    // Modify DTSTART and DURATION
    let edited = loaded
        .replace("20260115T130000", "20260116T150000")
        .replace("DURATION:PT1H", "DURATION:PT2H30M");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.start.as_deref(), Some("2026-01-16T15:00:00"));
    assert_eq!(stored.duration.as_deref(), Some("PT2H30M"));
    assert_eq!(stored.time_zone.as_deref(), Some("Europe/Berlin"));

    // Locations, links, and alerts are preserved intact
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

    // Modify all-day start and duration
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

    // Modify DESCRIPTION in place
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

    // Unmodeled keywords, alerts, virtualLocations, and links are preserved
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

    // Remove DESCRIPTION line
    let desc_line = loaded
        .lines()
        .find(|l| l.starts_with("DESCRIPTION"))
        .expect("DESCRIPTION line");
    let edited = loaded.replace(&format!("{desc_line}\r\n"), "");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.description, None);

    // Locations and alerts are preserved
    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 101"));

    let alerts = stored.alerts.expect("alerts");
    assert_eq!(alerts["a1"]["trigger"]["offset"], json!("-PT15M"));
}

#[test]
fn cancelling_an_event_reaches_the_server_as_cancelled_status() {
    // An organiser marks an event as cancelled: Evolution removes the
    // `STATUS:CONFIRMED` line and writes `STATUS:CANCELLED`. The server must
    // learn the new state and must not lose the description or location that
    // were already on the event.
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

    // Simulate what Evolution does when the user marks the event cancelled.
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
    // The user undoes a tentative mark: Evolution drops the `STATUS` line
    // entirely. The server's `status` must become absent (null patch).
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

    // Drop the STATUS line.
    let edited: String = loaded
        .lines()
        .filter(|l| !l.starts_with("STATUS"))
        .map(|l| format!("{l}\r\n"))
        .collect();
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.status, None, "status was not cleared on the server");
    // The keyword the user never touched must survive.
    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("planning"), "keyword was lost: {kws:?}");
}

#[test]
fn clearing_privacy_removes_the_classification_from_the_server() {
    // The user removes the Classification setting: Evolution drops the
    // `CLASS` line. The server's `privacy` must become absent (null patch),
    // reverting to the RFC 8984 default of `public`.
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

    // Drop the CLASS line (the user cleared the Classification field).
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
    // The alert the user never touched must survive.
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

    // Edit location name and change priority to 1 (high priority).
    let edited = loaded
        .replace("Room 101", "Room 204 (West Wing)")
        .replace("PRIORITY:3", "PRIORITY:1");

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.priority, Some(1));

    let locs = stored.locations.expect("locations");
    assert_eq!(locs["loc1"]["name"], json!("Room 204 (West Wing)"));
    // Unmodeled coordinates and description on the location survive
    assert_eq!(
        locs["loc1"]["coordinates"],
        json!("geo:52.520008,13.404954")
    );
    assert_eq!(locs["loc1"]["description"], json!("Main Building"));

    // Virtual locations, links, and alerts survive
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

    // Drop LOCATION and PRIORITY lines.
    let edited: String = loaded
        .lines()
        .filter(|l| !l.starts_with("LOCATION") && !l.starts_with("PRIORITY"))
        .map(|l| format!("{l}\r\n"))
        .collect();

    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.priority, None);
    assert_eq!(stored.locations, None);

    // Unmodified description and keywords survive
    assert_eq!(stored.description.as_deref(), Some("Weekly status check"));
    let kws = stored.keywords.expect("keywords");
    assert!(kws.contains_key("standup"));
}

#[test]
fn no_op_save_mints_no_patch_across_all_calendar_mapped_surfaces_matrix() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    // Matrix covering:
    // - alerts (relative/absolute triggers, action, useDefaultAlerts)
    // - recurrenceRule (daily/weekly/monthly RRULE, INTERVAL, UNTIL, COUNT)
    // - recurrenceOverrides (EXDATE, RDATE, RECURRENCE-ID overrides)
    // - zones (IANA, custom timeZones)
    // - locations and virtualLocations (LOCATION, CONFERENCE)
    // - links (ATTACH, IMAGE)
    // - keywords / categories
    // - privacy, status, priority, freeBusyStatus, showWithoutTime
    // - unmodeled extra properties
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

        // Load 1: Read event as rendered iCalendar
        let loaded1 = sync.load_component(id.as_str()).expect(name);

        // Save 1: Save untouched iCalendar back
        let saved1 = sync
            .save_component(&loaded1.icalendar, Some(id.as_str()))
            .expect(name);

        // Load 2: Fresh reload after Save 1
        let loaded2 = sync.load_component(id.as_str()).expect(name);

        // Assert: Save 2 MUST produce NO patch and preserve identical revision
        let saved2 = sync
            .save_component(&loaded2.icalendar, Some(id.as_str()))
            .expect(name);
        assert_eq!(
            saved1.revision, saved2.revision,
            "case '{name}': second save must not update revision"
        );

        // Assert: diff between server state and parsed event is completely empty
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

    // Direct diff when edited carries explicit empty containers / empty names:
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
