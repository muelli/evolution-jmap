// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_ical::ical_to_event;
use jmap_proto::Id;
use jmap_proto::calendars::{
    Alert, CalendarEvent, Link, Location, NDay, Participant, RecurrenceRule, VirtualLocation,
};
use jmap_proto::state::UtcDate;
use serde_json::Value;

/// Reserved or obsoleted properties in the JSCalendar-bis registry
/// (draft-ietf-calext-jscalendarbis-22 Section 7.4.2 and Appendix A).
const GLOBALLY_FORBIDDEN_PROPERTIES: &[&str] = &[
    "aliases",
    "cid",
    "comments",
    "daylight",
    "excludedRecurrenceRules",
    "extra",
    "invitedBy",
    "language",
    "locationId",
    "localizations",
    "names",
    "offsetFrom",
    "offsetTo",
    "participationComment",
    "progressUpdated",
    "recurrenceRules",
    "replyTo",
    "requestStatus",
    "scheduleAgent",
    "scheduleForceSend",
    "scheduleStatus",
    "sendTo",
    "standard",
    "tzId",
    "validUntil",
    // Retain `timeZones` to preserve custom VTIMEZONE definitions (ICAL-MAPPING 11 and 4.2).
];

fn assert_no_reserved_or_obsoleted_properties(val: &Value, path: &str) {
    match val {
        Value::Object(map) => {
            let is_location = map.get("@type").and_then(Value::as_str) == Some("Location")
                || path.contains("Location");
            let is_virtual_location = map.get("@type").and_then(Value::as_str)
                == Some("VirtualLocation")
                || path.contains("VirtualLocation");
            let is_participant = map.get("@type").and_then(Value::as_str) == Some("Participant")
                || path.contains("Participant");

            for (k, v) in map {
                let current_path = format!("{path}/{k}");
                assert!(
                    !GLOBALLY_FORBIDDEN_PROPERTIES.contains(&k.as_str()),
                    "Found forbidden property '{k}' at {current_path}"
                );

                if is_location {
                    assert!(
                        k != "description" && k != "relativeTo" && k != "timeZone",
                        "Found Location-forbidden property '{k}' at {current_path}"
                    );
                }
                if is_virtual_location {
                    assert!(
                        k != "description",
                        "Found VirtualLocation-forbidden property '{k}' at {current_path}"
                    );
                }
                if is_participant {
                    assert!(
                        k != "scheduleSequence" && k != "scheduleUpdated",
                        "Found Participant-forbidden property '{k}' at {current_path}"
                    );
                }

                // Do not recurse into `timeZones` RFC 8984 definitions.
                if k != "timeZones" {
                    assert_no_reserved_or_obsoleted_properties(v, &current_path);
                }
            }
        }
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                let current_path = format!("{path}[{idx}]");
                assert_no_reserved_or_obsoleted_properties(item, &current_path);
            }
        }
        _ => {}
    }
}

#[test]
fn never_emit_reserved_or_obsoleted_names_in_serialized_types_and_ical_output() {
    // 1. RecurrenceRule with all fields populated
    let rrule = {
        let mut __built = RecurrenceRule::default();
        __built.rule_type = Some("RecurrenceRule".to_owned());
        __built.frequency = "monthly".to_owned();
        __built.interval = Some(2);
        __built.count = Some(10);
        __built.until = Some("2026-12-31T23:59:59Z".to_owned());
        __built.by_second = Some(vec![0, 30]);
        __built.by_minute = Some(vec![15, 45]);
        __built.by_hour = Some(vec![9, 17]);
        __built.by_day = Some(vec![NDay::new("mo")]);
        __built.by_month_day = Some(vec![1, -1]);
        __built.by_year_day = Some(vec![100, -100]);
        __built.by_week_no = Some(vec![1, 52]);
        __built.by_month = Some(vec!["3".to_owned(), "9".to_owned()]);
        __built.by_set_position = Some(vec![1, -1]);
        __built.first_day_of_week = Some("mo".to_owned());
        __built.rscale = Some("gregorian".to_owned());
        __built.skip = Some("omit".to_owned());
        __built.extra = BTreeMap::new();
        __built
    };
    let rrule_val = serde_json::to_value(&rrule).expect("serializes RecurrenceRule");
    assert_no_reserved_or_obsoleted_properties(&rrule_val, "RecurrenceRule");

    // 2. Alert with all fields populated
    let alert = Alert::new(
        "display",
        serde_json::json!({
            "@type": "OffsetTrigger",
            "offset": "-PT15M",
            "relativeTo": "start"
        }),
    )
    .with_acknowledged(UtcDate::new("2026-10-14T12:00:00Z"))
    .with_related_to("alert-parent");
    let alert_val = serde_json::to_value(&alert).expect("serializes Alert");
    assert_no_reserved_or_obsoleted_properties(&alert_val, "Alert");

    // 3. Link with all fields populated (including obsoleted cid)
    let link: Link = serde_json::from_value(serde_json::json!({
        "@type": "Link",
        "href": "https://example.com/agenda.pdf",
        "contentType": "application/pdf",
        "size": 1024,
        "rel": "enclosure",
        "display": {"badge": true},
        "title": "Meeting Agenda",
        "cid": "cid:12345@example.com",
    }))
    .expect("deserializes Link with obsoleted cid");
    let link_val = serde_json::to_value(&link).expect("serializes Link");
    assert_no_reserved_or_obsoleted_properties(&link_val, "Link");

    // 4. Location with all fields populated (including obsoleted/reserved relativeTo, timeZone, description)
    let location = Location::new("Conference Room 1")
        .with_description("Third floor next to elevators")
        .with_relative_to("start")
        .with_time_zone("Europe/Berlin")
        .with_coordinates("geo:52.5200,13.4050")
        .with_location_types(BTreeMap::from([("room".to_owned(), true)]));
    let loc_val = serde_json::to_value(&location).expect("serializes Location");
    assert_no_reserved_or_obsoleted_properties(&loc_val, "Location");

    // 5. VirtualLocation with all fields populated (including reserved description)
    let vlocation = VirtualLocation::new("https://meet.example.com/bridge")
        .with_name("Video Bridge")
        .with_description("PIN: 4321")
        .with_features(BTreeMap::from([("video".to_owned(), true)]));
    let vloc_val = serde_json::to_value(&vlocation).expect("serializes VirtualLocation");
    assert_no_reserved_or_obsoleted_properties(&vloc_val, "VirtualLocation");

    // 6. Participant with all fields populated (including reserved/obsoleted sendTo, locationId, scheduleAgent, etc.)
    let participant = Participant::new("Alice Developer", "alice@example.com")
        .with_description("Core maintainer")
        .with_description_content_type("text/plain")
        .with_calendar_address("mailto:alice@example.com")
        .with_kind("individual")
        .with_roles(BTreeMap::from([("required".to_owned(), true)]))
        .with_location_id("loc-1")
        .with_participation_status("accepted")
        .with_participation_comment("Will join online")
        .with_attendance("required")
        .expect_reply(true)
        .with_schedule_agent("server")
        .with_schedule_sequence(1)
        .with_schedule_status(["2.0".to_owned()])
        .with_schedule_updated(UtcDate::new("2026-10-14T10:00:00Z"))
        .with_delegated_to(["mailto:bob@example.com".to_owned()])
        .with_delegated_from(["mailto:carol@example.com".to_owned()])
        .with_member_of(BTreeMap::from([(
            "mailto:team@example.com".to_owned(),
            true,
        )]))
        .with_progress("completed")
        .with_progress_updated(UtcDate::new("2026-10-14T11:00:00Z"));
    let part_val = serde_json::to_value(&participant).expect("serializes Participant");
    assert_no_reserved_or_obsoleted_properties(&part_val, "Participant");

    // 7. CalendarEvent with all fields populated (including obsoleted timeZones, localizations, etc.)
    let mut cal_ids = BTreeMap::new();
    cal_ids.insert(Id::new("cal-1"), true);
    let mut locations = BTreeMap::new();
    locations.insert("loc-1".to_owned(), loc_val.clone());
    let mut virtual_locations = BTreeMap::new();
    virtual_locations.insert("vloc-1".to_owned(), vloc_val.clone());
    let mut links = BTreeMap::new();
    links.insert("link-1".to_owned(), link_val.clone());
    let mut keywords = BTreeMap::new();
    keywords.insert("tag1".to_owned(), Value::Bool(true));
    let mut alerts = BTreeMap::new();
    alerts.insert("alert-1".to_owned(), alert_val.clone());
    let mut participants = BTreeMap::new();
    participants.insert("part-1".to_owned(), part_val.clone());
    let mut recurrence_overrides = BTreeMap::new();
    recurrence_overrides.insert(
        "2026-10-21T10:00:00".to_owned(),
        serde_json::json!({
            "title": "Shifted Standup",
            "excluded": true
        }),
    );
    let mut time_zones = BTreeMap::new();
    time_zones.insert(
        "Custom/TZ".to_owned(),
        serde_json::json!({"tzId": "Custom/TZ"}),
    );
    let mut localizations = BTreeMap::new();
    localizations.insert("de".to_owned(), serde_json::json!({"title": "Standup"}));

    let event = {
        let mut event_out = CalendarEvent::default();
        event_out.id = Some(Id::new("evt-1"));
        event_out.calendar_ids = Some(cal_ids);
        event_out.event_type = Some("Event".to_owned());
        event_out.version = Some("2.0".to_owned());
        event_out.uid = Some("uid-12345".to_owned());
        event_out.created = Some("2026-10-01T12:00:00Z".to_owned());
        event_out.updated = Some("2026-10-14T12:00:00Z".to_owned());
        event_out.title = Some("Sprint Planning".to_owned());
        event_out.description = Some("Sprint planning session".to_owned());
        event_out.start = Some("2026-10-15T09:00:00".to_owned());
        event_out.time_zone = Some("Europe/Berlin".to_owned());
        event_out.duration = Some("PT1H".to_owned());
        event_out.show_without_time = Some(false);
        event_out.status = Some("confirmed".to_owned());
        event_out.free_busy_status = Some("busy".to_owned());
        event_out.priority = Some(1);
        event_out.privacy = Some("public".to_owned());
        event_out.locations = Some(locations);
        event_out.virtual_locations = Some(virtual_locations);
        event_out.links = Some(links);
        event_out.keywords = Some(keywords);
        event_out.alerts = Some(alerts);
        event_out.participants = Some(participants);
        event_out.organizer_calendar_address = Some("mailto:organizer@example.com".to_owned());
        event_out.is_draft = Some(false);
        event_out.hide_attendees = Some(false);
        event_out.recurrence_rule = Some(rrule);
        event_out.recurrence_overrides = Some(recurrence_overrides);
        event_out.time_zones = Some(time_zones);
        event_out.use_default_alerts = Some(false);
        event_out.color = Some("#aabbcc".to_owned());
        event_out.locale = Some("en".to_owned());
        event_out.localizations = Some(localizations);
        event_out.is_origin = Some(true);
        event_out.end_time_zone = Some("Europe/Berlin".to_owned());
        event_out.main_location_id = Some(Id::new("loc-1"));
        event_out.schedule_sequence = Some(0);
        event_out.schedule_updated = Some(UtcDate::new("2026-10-14T12:00:00Z"));
        event_out.may_invite_self = Some(true);
        event_out.may_invite_others = Some(true);
        event_out.extra = BTreeMap::new();
        event_out
    };

    let event_val = serde_json::to_value(&event).expect("serializes CalendarEvent");
    assert_no_reserved_or_obsoleted_properties(&event_val, "CalendarEvent");

    // 8. jmap-ical output across fixtures with all properties exercised
    let fixture_ics = &[
        include_str!("fixtures/cyrus_caldav_export.ics"),
        include_str!("fixtures/evolution_calendar_export.ics"),
        include_str!("fixtures/sogo_calendar_export.ics"),
        include_str!("fixtures/nextcloud_calendar_export.ics"),
        include_str!("fixtures/apple_calendar_export.ics"),
        include_str!("fixtures/outlook_m365_export.ics"),
        include_str!("fixtures/custom_vtimezone_export.ics"),
        include_str!("fixtures/google_calendar_export.ics"),
        include_str!("fixtures/thunderbird_calendar_export.ics"),
    ];

    for (idx, ics) in fixture_ics.iter().enumerate() {
        let parsed = ical_to_event(ics).expect("parses fixture ics");
        let parsed_val = serde_json::to_value(&parsed).expect("serializes parsed fixture event");
        assert_no_reserved_or_obsoleted_properties(
            &parsed_val,
            &format!("ical_to_event_fixture_{idx}"),
        );
    }
}

#[test]
fn deserialization_still_tolerates_obsolete_and_reserved_forms_without_reemitting() {
    // 1. Location reads description, relativeTo, timeZone from legacy JSON
    let loc_json = serde_json::json!({
        "@type": "Location",
        "name": "Room 42",
        "description": "Floor 4",
        "relativeTo": "start",
        "timeZone": "Europe/Berlin",
        "coordinates": "geo:52.5,13.4"
    });
    let loc: Location = serde_json::from_value(loc_json).expect("deserializes legacy Location");
    assert_eq!(loc.name.as_deref(), Some("Room 42"));
    assert_eq!(loc.description.as_deref(), Some("Floor 4"));
    assert_eq!(loc.relative_to.as_deref(), Some("start"));
    assert_eq!(loc.time_zone.as_deref(), Some("Europe/Berlin"));
    let loc_reemit = serde_json::to_value(&loc).expect("serializes Location");
    assert_no_reserved_or_obsoleted_properties(&loc_reemit, "Location");

    // 2. VirtualLocation reads description from legacy JSON
    let vloc_json = serde_json::json!({
        "@type": "VirtualLocation",
        "name": "Bridge 1",
        "description": "Meeting description",
        "uri": "https://meet.example.com/b1"
    });
    let vloc: VirtualLocation =
        serde_json::from_value(vloc_json).expect("deserializes legacy VirtualLocation");
    assert_eq!(vloc.description.as_deref(), Some("Meeting description"));
    let vloc_reemit = serde_json::to_value(&vloc).expect("serializes VirtualLocation");
    assert_no_reserved_or_obsoleted_properties(&vloc_reemit, "VirtualLocation");

    // 3. Link reads cid from legacy JSON
    let link_json = serde_json::json!({
        "@type": "Link",
        "href": "data:text/plain,hello",
        "cid": "cid:test@example.com"
    });
    let link: Link = serde_json::from_value(link_json).expect("deserializes legacy Link");
    assert_eq!(link.cid.as_deref(), Some("cid:test@example.com"));
    let link_reemit = serde_json::to_value(&link).expect("serializes Link");
    assert_no_reserved_or_obsoleted_properties(&link_reemit, "Link");

    // 4. Participant reads legacy sendTo, locationId, schedule properties
    let part_json = serde_json::json!({
        "@type": "Participant",
        "name": "Bob",
        "sendTo": { "imip": "mailto:bob@example.com" },
        "locationId": "loc-99",
        "participationComment": "Attending",
        "scheduleAgent": "server",
        "scheduleSequence": 3,
        "scheduleStatus": ["2.0"],
        "scheduleUpdated": "2026-10-14T10:00:00Z",
        "progressUpdated": "2026-10-14T11:00:00Z"
    });
    let part: Participant =
        serde_json::from_value(part_json).expect("deserializes legacy Participant");
    assert_eq!(part.name.as_deref(), Some("Bob"));
    assert_eq!(
        part.calendar_address.as_deref(),
        Some("mailto:bob@example.com")
    );
    assert_eq!(part.location_id.as_deref(), Some("loc-99"));
    assert_eq!(part.participation_comment.as_deref(), Some("Attending"));
    assert_eq!(part.schedule_agent.as_deref(), Some("server"));
    assert_eq!(part.schedule_sequence, Some(3));
    let part_reemit = serde_json::to_value(&part).expect("serializes Participant");
    assert_no_reserved_or_obsoleted_properties(&part_reemit, "Participant");

    // 5. CalendarEvent reads timeZones and localizations from legacy JSON
    let ev_json = serde_json::json!({
        "@type": "Event",
        "title": "Legacy Event",
        "start": "2026-10-15T10:00:00",
        "timeZones": {
            "Custom/Zone": { "@type": "TimeZone" }
        },
        "localizations": {
            "fr": { "title": "Evenement" }
        }
    });
    let ev: CalendarEvent =
        serde_json::from_value(ev_json).expect("deserializes legacy CalendarEvent");
    assert_eq!(ev.title.as_deref(), Some("Legacy Event"));
    assert!(ev.time_zones.as_ref().unwrap().contains_key("Custom/Zone"));
    assert!(ev.localizations.as_ref().unwrap().contains_key("fr"));
    let ev_reemit = serde_json::to_value(&ev).expect("serializes CalendarEvent");
    assert_no_reserved_or_obsoleted_properties(&ev_reemit, "CalendarEvent");
}
