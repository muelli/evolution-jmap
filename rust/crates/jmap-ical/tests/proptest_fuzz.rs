// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structure-aware fuzzing of the JSCalendar ↔ iCalendar mapping using `proptest`.
//!
//! Asserts:
//! 1. `event_to_ical` never panics on arbitrary `CalendarEvent` instances.
//! 2. `ical_to_event` never panics on arbitrary strings or arbitrary iCalendar envelopes.
//! 3. Round-trip stability: Emitting an event, parsing it back, and re-emitting reaches a fixed point.
//! 4. Domain-specific fixpoint properties across dates/zones (incl. Windows timezones and
//!    globally-unique TZIDs), recurrence rules, recurrence overrides (EXDATE, RDATE, detached
//!    VEVENTs), alerts / VALARM, participants / organizer, physical and virtual locations,
//!    attachments and image links, categories / keywords, and metadata.

use std::collections::BTreeMap;

use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use proptest::prelude::*;
use serde_json::{Value, json};

prop_compose! {
    fn arb_nday()(
        day in prop_oneof![
            Just("mo".to_string()),
            Just("tu".to_string()),
            Just("we".to_string()),
            Just("th".to_string()),
            Just("fr".to_string()),
            Just("sa".to_string()),
            Just("su".to_string()),
            "[a-z]{1,4}",
        ],
        nth_of_period in prop::option::of(-53..=53i32),
    ) -> NDay {
        NDay {
            day_type: Some("NDay".to_string()),
            day,
            nth_of_period,
            extra: BTreeMap::new(),
        }
    }
}

prop_compose! {
    fn arb_recurrence_rule()(
        frequency in prop_oneof![
            Just("daily".to_string()),
            Just("weekly".to_string()),
            Just("monthly".to_string()),
            Just("yearly".to_string()),
            Just("hourly".to_string()),
            Just("minutely".to_string()),
            Just("secondly".to_string()),
            "[a-z]{1,8}",
        ],
        interval in prop::option::of(1..100u32),
        count in prop::option::of(1..500u32),
        until in prop::option::of(prop_oneof![
            Just("20261231T235959Z".to_string()),
            Just("2026-12-31T23:59:59".to_string()),
            Just("20261231".to_string()),
            "\\PC*",
        ]),
        by_second in prop::option::of(prop::collection::vec(0..60u32, 0..5)),
        by_minute in prop::option::of(prop::collection::vec(0..60u32, 0..5)),
        by_hour in prop::option::of(prop::collection::vec(0..24u32, 0..5)),
        by_day in prop::option::of(prop::collection::vec(arb_nday(), 0..5)),
        by_month_day in prop::option::of(prop::collection::vec(-31..=31i32, 0..5)),
        by_year_day in prop::option::of(prop::collection::vec(-366..=366i32, 0..5)),
        by_week_no in prop::option::of(prop::collection::vec(-53..=53i32, 0..5)),
        by_month in prop::option::of(prop::collection::vec(
            prop_oneof![
                Just("1".to_string()),
                Just("6".to_string()),
                Just("12".to_string()),
                Just("5L".to_string()),
                "[0-9]{1,2}",
            ],
            0..5,
        )),
        by_set_position in prop::option::of(prop::collection::vec(-366..=366i32, 0..4)),
        first_day_of_week in prop::option::of(prop_oneof![
            Just("mo".to_string()),
            Just("tu".to_string()),
            Just("we".to_string()),
            Just("th".to_string()),
            Just("fr".to_string()),
            Just("sa".to_string()),
            Just("su".to_string()),
        ]),
    ) -> RecurrenceRule {
        RecurrenceRule {
            rule_type: Some("RecurrenceRule".to_string()),
            frequency,
            interval,
            count,
            until,
            by_second,
            by_minute,
            by_hour,
            by_day,
            by_month_day,
            by_year_day,
            by_week_no,
            by_month,
            by_set_position,
            first_day_of_week,
            rscale: None,
            skip: None,
            extra: BTreeMap::new(),
        }
    }
}

fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,8}",
        Just("l1".to_string()),
        Just("l2".to_string()),
        Just("v1".to_string()),
        Just("k1".to_string()),
        Just("a1".to_string()),
        Just("a2".to_string()),
        Just("p1".to_string()),
        Just("k1\r\nSUMMARY:Injected".to_string()),
        Just("alert1\"quoted".to_string()),
        "\\PC{1,8}",
    ]
}

prop_compose! {
    fn arb_ids()(
        id in prop::option::of("[a-zA-Z0-9_-]{1,16}"),
        uid in prop::option::of("[a-zA-Z0-9_-]{1,16}"),
    ) -> (Option<String>, Option<String>) {
        (id, uid)
    }
}

fn arb_time_zone_string() -> impl Strategy<Value = String> {
    prop_oneof![
        // Canonical IANA zones
        Just("Etc/UTC".to_string()),
        Just("Europe/Berlin".to_string()),
        Just("Europe/Paris".to_string()),
        Just("Europe/London".to_string()),
        Just("America/New_York".to_string()),
        Just("America/Chicago".to_string()),
        Just("America/Los_Angeles".to_string()),
        Just("America/Argentina/Buenos_Aires".to_string()),
        Just("Asia/Tokyo".to_string()),
        Just("Asia/Shanghai".to_string()),
        Just("Asia/Kolkata".to_string()),
        Just("Australia/Sydney".to_string()),
        Just("Africa/Cairo".to_string()),
        Just("Pacific/Auckland".to_string()),
        Just("Etc/GMT+5".to_string()),
        // Windows time zone names from real exporters (Unicode CLDR / batch 8)
        Just("W. Europe Standard Time".to_string()),
        Just("Romance Standard Time".to_string()),
        Just("GMT Standard Time".to_string()),
        Just("Greenwich Standard Time".to_string()),
        Just("Central European Standard Time".to_string()),
        Just("Eastern Standard Time".to_string()),
        Just("Central Standard Time".to_string()),
        Just("Pacific Standard Time".to_string()),
        Just("Tokyo Standard Time".to_string()),
        Just("China Standard Time".to_string()),
        Just("India Standard Time".to_string()),
        Just("Russian Standard Time".to_string()),
        Just("E. South America Standard Time".to_string()),
        Just("UTC-11".to_string()),
        Just("UTC+12".to_string()),
        // Globally-unique TZID forms (RFC 5545 §3.8.3.1 / batch 8)
        Just("/mozilla.org/20070129_1/Europe/Berlin".to_string()),
        Just("/citadel.org/20080105_1/Europe/Paris".to_string()),
        Just("/freeassociation.sourceforge.net/Tzfile/Europe/Berlin".to_string()),
        Just("/apple.com/timezones/America/Argentina/Buenos_Aires".to_string()),
        Just("/kde.org/tz/Europe/Rome".to_string()),
        Just("/google.com/20260101_1/Asia/Tokyo".to_string()),
        // Custom solidus definitions
        Just("/custom/zone_1".to_string()),
        Just("/example.com/CorporateZone".to_string()),
        // Arbitrary strings
        "[A-Za-z0-9/_+-]{1,20}",
        "\\PC*",
    ]
}

prop_compose! {
    fn arb_event_core()(
        title in prop::option::of("\\PC*"),
        description in prop::option::of("\\PC*"),
        start in prop::option::of(prop_oneof![
            Just("2026-01-15T13:00:00".to_string()),
            Just("2026-08-19T00:00:00".to_string()),
            Just("2000-02-29T12:00:00".to_string()),
            Just("1900-01-01T00:00:00".to_string()),
            Just("20260115T130000Z".to_string()),
            "20[0-9]{2}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]",
            "\\PC*",
        ]),
        time_zone in prop::option::of(arb_time_zone_string()),
        duration in prop::option::of(prop_oneof![
            Just("PT1H".to_string()),
            Just("PT30M".to_string()),
            Just("PT15M".to_string()),
            Just("PT45M".to_string()),
            Just("P1D".to_string()),
            Just("P1W".to_string()),
            Just("P2D".to_string()),
            Just("PT0S".to_string()),
            Just("PT2H30M".to_string()),
            "P[0-9]{1,2}D",
            "PT[0-9]{1,2}H",
            "\\PC*",
        ]),
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        (title, description, start, time_zone, duration)
    }
}

prop_compose! {
    fn arb_event_metadata()(
        show_without_time in prop::option::of(any::<bool>()),
        status in prop::option::of(prop_oneof![
            Just("confirmed".to_string()),
            Just("tentative".to_string()),
            Just("cancelled".to_string()),
            "[a-z]{1,10}",
        ]),
        free_busy_status in prop::option::of(prop_oneof![
            Just("free".to_string()),
            Just("busy".to_string()),
            "[a-z]{1,8}",
        ]),
        priority in prop::option::of(-5..=15i64),
        privacy in prop::option::of(prop_oneof![
            Just("public".to_string()),
            Just("private".to_string()),
            Just("secret".to_string()),
            "[a-z]{1,8}",
        ]),
    ) -> (
        Option<bool>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
    ) {
        (
            show_without_time,
            status,
            free_busy_status,
            priority,
            privacy,
        )
    }
}

fn arb_location_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"name": "Conference Room A"})),
        Just(
            json!({"name": "HQ", "description": "Main office, 3rd floor", "coordinates": "geo:37.7749,-122.4194"})
        ),
        Just(json!({"name": "Room 101; Building B", "description": "East Wing"})),
        Just(json!({"name": "Café & Bakery, München", "coordinates": "geo:48.1351,11.5820"})),
        Just(json!({"name": "", "description": "Empty room name"})),
        Just(json!({"name": "Project Site", "timeZone": "Europe/Berlin"})),
        Just(json!({"coordinates": "geo:51.5074,-0.1278"})),
        Just(json!(123)),
    ]
}

fn arb_virtual_location_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(
            json!({"uri": "https://meet.example.com/room", "name": "Video call", "features": {"audio": true, "video": true, "screen": true}})
        ),
        Just(json!({"uri": "tel:+1234567890", "features": {"phone": true, "audio": true}})),
        Just(json!({"uri": "sip:room123@sip.example.com", "name": "SIP Bridge"})),
        Just(
            json!({"uri": "https://zoom.us/j/123456789", "features": {"video": true, "chat": true, "moderator": true}})
        ),
        Just(json!({"name": "Audio bridge"})),
    ]
}

fn arb_link_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(
            json!({"href": "https://example.com/agenda.pdf", "contentType": "application/pdf", "title": "Meeting Agenda", "size": 102400})
        ),
        Just(
            json!({"href": "https://example.com/badge.png", "display": "badge", "rel": "icon", "contentType": "image/png"})
        ),
        Just(
            json!({"href": "https://photos.example.org/photo.jpg", "display": "fullsize", "rel": "icon", "contentType": "image/jpeg"})
        ),
        Just(json!({"href": "cid:doc123", "title": "Inline Document"})),
        Just(json!({"href": "https://example.com/minutes.txt", "contentType": "text/plain"})),
    ]
}

fn arb_alert_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "action": "display"})
        ),
        Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "-P1D"}, "action": "display"})),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT2H"}, "action": "display"})
        ),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "PT15M"}, "action": "display"})
        ),
        Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "PT0S"}, "action": "display"})),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT0S"}, "action": "display"})
        ),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "PT0S", "relativeTo": "end"}, "action": "display"})
        ),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT30M", "relativeTo": "end"}, "action": "display"})
        ),
        Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT10M"}})),
        // Refused shapes to test safe dropping / refusal boundaries
        Just(
            json!({"trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T12:45:00Z"}, "action": "display"})
        ),
        Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "action": "email"})),
        Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "action": "audio"})),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "acknowledged": "2026-01-15T12:00:00Z"})
        ),
        Just(
            json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}, "description": "Custom Reminder Text"})
        ),
    ]
}

fn arb_participant_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(
            json!({"name": "Alice Organizer", "email": "alice@example.com", "sendTo": {"imip": "mailto:alice@example.com"}, "roles": {"owner": true}, "kind": "individual"})
        ),
        Just(
            json!({"name": "Bob Attendee", "email": "bob@example.com", "sendTo": {"imip": "mailto:bob@example.com"}, "roles": {"attendee": true}, "participationStatus": "accepted", "kind": "individual"})
        ),
        Just(
            json!({"name": "Carol Chair", "sendTo": {"imip": "mailto:carol@example.com"}, "roles": {"chair": true}, "participationStatus": "tentative"})
        ),
        Just(
            json!({"name": "Dave Optional", "sendTo": {"imip": "mailto:dave@example.com"}, "roles": {"optional": true}, "participationStatus": "needs-action", "expectReply": true})
        ),
        Just(
            json!({"name": "Eve Info", "sendTo": {"imip": "mailto:eve@example.com"}, "roles": {"informational": true}, "kind": "individual"})
        ),
        Just(
            json!({"name": "Projector Room 1", "sendTo": {"imip": "mailto:room1@example.com"}, "kind": "location", "participationStatus": "accepted"})
        ),
    ]
}

fn arb_keyword_tag() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,10}",
        Just("Work, Urgent".to_string()),
        Just("Acme, Inc.".to_string()),
        Just("Project;Alpha".to_string()),
        Just("Dept\\Core".to_string()),
        Just("Line 1\nLine 2".to_string()),
        Just("Büro & Verwaltung".to_string()),
        Just("🚀 VIP".to_string()),
        Just(" leading".to_string()),
        Just("trailing ".to_string()),
        Just("with\rcr".to_string()),
        "\\PC{1,10}",
    ]
}

prop_compose! {
    fn arb_event_maps()(
        locations in prop::option::of(prop::collection::btree_map(arb_key(), arb_location_value(), 0..3)),
        virtual_locations in prop::option::of(prop::collection::btree_map(arb_key(), arb_virtual_location_value(), 0..3)),
        links in prop::option::of(prop::collection::btree_map(arb_key(), arb_link_value(), 0..3)),
        keywords in prop::option::of(prop::collection::btree_map(
            arb_keyword_tag(),
            prop_oneof![Just(json!(true)), Just(json!(false)), Just(json!("tag")), Just(json!(1))],
            0..4,
        )),
        alerts in prop::option::of(prop::collection::btree_map(arb_key(), arb_alert_value(), 0..3)),
        participants in prop::option::of(prop::collection::btree_map(arb_key(), arb_participant_value(), 0..3)),
    ) -> (
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
    ) {
        (
            locations,
            virtual_locations,
            links,
            keywords,
            alerts,
            participants,
        )
    }
}

fn arb_override_key() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("2026-01-16T13:00:00".to_string()),
        Just("2026-01-23T13:00:00".to_string()),
        Just("2026-01-30T13:00:00".to_string()),
        Just("2026-02-06T13:00:00".to_string()),
        "20[0-9]{2}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]",
    ]
}

fn arb_override_patch_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        // Cancelled occurrence (EXDATE)
        Just(json!({"excluded": true})),
        // Added occurrence (RDATE)
        Just(json!({})),
        Just(json!({"duration": "PT2H"})),
        // Rescheduled occurrence
        Just(json!({"start": "2026-01-16T14:30:00", "duration": "PT1H30M"})),
        // Timezone shift
        Just(json!({"start": "2026-01-16T15:00:00", "timeZone": "Asia/Tokyo"})),
        // Modified scalar properties
        Just(json!({"title": "Special Session", "description": "Keynote Lecture in Room A"})),
        Just(json!({"status": "tentative", "priority": 1, "privacy": "secret"})),
        Just(json!({"freeBusyStatus": "free"})),
        // Modified map properties (keywords & alerts)
        Just(json!({"keywords": {"Special": true, "Keynote": true}})),
        Just(
            json!({"alerts": {"a1": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT30M"}, "action": "display"}}})
        ),
        // Property removal via null
        Just(json!({"description": null, "duration": null, "priority": null, "privacy": null})),
    ]
}

prop_compose! {
    fn arb_event_recurrence()(
        recurrence_rule in prop::option::of(arb_recurrence_rule()),
        recurrence_overrides in prop::option::of(prop::collection::btree_map(
            arb_override_key(),
            arb_override_patch_value(),
            0..3,
        )),
        time_zones in prop::option::of(prop::collection::btree_map(
            Just("/custom/zone_1".to_string()),
            prop_oneof![
                Just(json!({
                    "standard": [{
                        "start": "1970-01-01T00:00:00",
                        "offsetFrom": "+01:00",
                        "offsetTo": "+01:00",
                        "name": "CET"
                    }]
                })),
                Just(json!({})),
            ],
            0..2,
        )),
    ) -> (
        Option<RecurrenceRule>,
        Option<BTreeMap<String, Value>>,
        Option<BTreeMap<String, Value>>,
    ) {
        (recurrence_rule, recurrence_overrides, time_zones)
    }
}

fn arb_calendar_event() -> impl Strategy<Value = CalendarEvent> {
    (
        arb_ids(),
        arb_event_core(),
        arb_event_metadata(),
        arb_event_maps(),
        arb_event_recurrence(),
    )
        .prop_map(
            |(
                (id, uid),
                (title, description, start, time_zone, duration),
                (show_without_time, status, free_busy_status, priority, privacy),
                (locations, virtual_locations, links, keywords, alerts, participants),
                (recurrence_rule, recurrence_overrides, time_zones),
            )| {
                CalendarEvent {
                    id: id.map(Into::into),
                    uid,
                    event_type: Some("Event".to_string()),
                    version: Some("2.0".to_string()),
                    title,
                    description,
                    start,
                    time_zone,
                    duration,
                    show_without_time,
                    status,
                    free_busy_status,
                    priority,
                    privacy,
                    locations,
                    virtual_locations,
                    links,
                    keywords,
                    alerts,
                    participants,
                    recurrence_rule,
                    recurrence_overrides,
                    time_zones,
                    ..CalendarEvent::default()
                }
            },
        )
}

prop_compose! {
    fn arb_ical_property_line()(
        name in prop_oneof![
            Just("SUMMARY".to_string()),
            Just("DESCRIPTION".to_string()),
            Just("DTSTART".to_string()),
            Just("DTEND".to_string()),
            Just("DURATION".to_string()),
            Just("RRULE".to_string()),
            Just("STATUS".to_string()),
            Just("TRANSP".to_string()),
            Just("PRIORITY".to_string()),
            Just("CLASS".to_string()),
            Just("LOCATION".to_string()),
            Just("CONFERENCE".to_string()),
            Just("ATTACH".to_string()),
            Just("IMAGE".to_string()),
            Just("CATEGORIES".to_string()),
            Just("ORGANIZER".to_string()),
            Just("ATTENDEE".to_string()),
            Just("RECURRENCE-ID".to_string()),
            Just("EXDATE".to_string()),
            Just("RDATE".to_string()),
            Just("UID".to_string()),
            Just("URL".to_string()),
            Just("CREATED".to_string()),
            Just("LAST-MODIFIED".to_string()),
            Just("SEQUENCE".to_string()),
            Just("X-CUSTOM".to_string()),
            Just("X-LIC-LOCATION".to_string()),
            Just("X-EVOLUTION-ALARM-UID".to_string()),
            "[A-Z0-9-]{1,12}",
        ],
        params in prop::collection::vec(
            prop_oneof![
                Just(";TZID=Europe/Berlin".to_string()),
                Just(";TZID=America/New_York".to_string()),
                Just(";TZID=Asia/Tokyo".to_string()),
                Just(";TZID=\"W. Europe Standard Time\"".to_string()),
                Just(";TZID=\"Eastern Standard Time\"".to_string()),
                Just(";TZID=/mozilla.org/20070129_1/Europe/Berlin".to_string()),
                Just(";TZID=/custom/zone_1".to_string()),
                Just(";VALUE=DATE".to_string()),
                Just(";VALUE=DATE-TIME".to_string()),
                Just(";VALUE=DURATION".to_string()),
                Just(";CN=\"Alice Example\"".to_string()),
                Just(";ROLE=REQ-PARTICIPANT".to_string()),
                Just(";ROLE=OPT-PARTICIPANT".to_string()),
                Just(";ROLE=CHAIR".to_string()),
                Just(";PARTSTAT=ACCEPTED".to_string()),
                Just(";PARTSTAT=DECLINED".to_string()),
                Just(";PARTSTAT=TENTATIVE".to_string()),
                Just(";CUTYPE=INDIVIDUAL".to_string()),
                Just(";CUTYPE=ROOM".to_string()),
                Just(";FMTTYPE=application/pdf".to_string()),
                Just(";FMTTYPE=image/png".to_string()),
                Just(";DISPLAY=BADGE".to_string()),
                Just(";DISPLAY=FULLSIZE".to_string()),
                Just(";FEATURE=VIDEO,AUDIO,SCREEN".to_string()),
                Just(";X-JMAP-KEY=k1".to_string()),
                Just(";X-JMAP-KEY=l1".to_string()),
                Just(";X-JMAP-KEY=v1".to_string()),
                Just(";X-JMAP-UID=evt1".to_string()),
                Just(";RELATED=END".to_string()),
                ";[A-Z-]+=[A-Za-z0-9-]+",
            ],
            0..3,
        ),
        value in prop_oneof![
            "\\PC*",
            Just("Team Sync Meeting".to_string()),
            Just("Discuss project roadmap\\, architecture\\; next steps.".to_string()),
            Just("20260115T130000Z".to_string()),
            Just("20260115T130000".to_string()),
            Just("20260115".to_string()),
            Just("PT1H".to_string()),
            Just("P1D".to_string()),
            Just("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR".to_string()),
            Just("FREQ=MONTHLY;BYMONTHDAY=15".to_string()),
            Just("CONFIRMED".to_string()),
            Just("TENTATIVE".to_string()),
            Just("OPAQUE".to_string()),
            Just("TRANSPARENT".to_string()),
            Just("PUBLIC".to_string()),
            Just("PRIVATE".to_string()),
            Just("CONFIDENTIAL".to_string()),
            Just("Conference Room A, 3rd Floor".to_string()),
            Just("https://meet.example.com/room-abc".to_string()),
            Just("https://example.com/agenda.pdf".to_string()),
            Just("Work, Planning, VIP".to_string()),
            Just("mailto:alice@example.com".to_string()),
            Just("mailto:bob@example.com".to_string()),
            Just("20260116T130000Z".to_string()),
            Just("20260123T130000Z".to_string()),
            Just("evt1-2026-uid".to_string()),
        ],
    ) -> String {
        let param_str = params.join("");
        format!("{name}{param_str}:{value}")
    }
}

prop_compose! {
    fn arb_raw_valarm_block()(
        action in prop_oneof![
            Just("DISPLAY".to_string()),
            Just("EMAIL".to_string()),
            Just("AUDIO".to_string()),
        ],
        trigger in prop_oneof![
            Just("-PT15M".to_string()),
            Just("-PT1H".to_string()),
            Just("-P1D".to_string()),
            Just("PT0S".to_string()),
            Just(";RELATED=END:-PT30M".to_string()),
            Just(";VALUE=DATE-TIME:20260115T124500Z".to_string()),
        ],
        uid in prop_oneof![
            Just("UID:alarm1\r\n".to_string()),
            Just("X-EVOLUTION-ALARM-UID:evo-alarm-1\r\n".to_string()),
            Just("X-WR-ALARMUID:outlook-alarm-1\r\n".to_string()),
            Just("".to_string()),
        ],
        desc in prop::option::of("\\PC*"),
    ) -> String {
        let desc_line = desc.map(|d| format!("DESCRIPTION:{d}\r\n")).unwrap_or_default();
        format!("BEGIN:VALARM\r\nACTION:{action}\r\nTRIGGER{trigger}\r\n{uid}{desc_line}END:VALARM\r\n")
    }
}

prop_compose! {
    fn arb_raw_vtimezone_block()(
        tzid in prop_oneof![
            Just("Europe/Berlin".to_string()),
            Just("America/New_York".to_string()),
            Just("W. Europe Standard Time".to_string()),
            Just("/custom/zone_1".to_string()),
        ],
        x_lic in prop::option::of(prop_oneof![
            Just("Europe/Berlin".to_string()),
            Just("America/New_York".to_string()),
        ]),
    ) -> String {
        let x_lic_line = x_lic.map(|l| format!("X-LIC-LOCATION:{l}\r\n")).unwrap_or_default();
        format!(
            "BEGIN:VTIMEZONE\r\nTZID:{tzid}\r\n{x_lic_line}BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\nTZNAME:CET\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n"
        )
    }
}

prop_compose! {
    fn arb_raw_detached_vevent_block()(
        rec_id in prop_oneof![
            Just("20260116T130000Z".to_string()),
            Just("20260123T130000Z".to_string()),
            Just(";TZID=Europe/Berlin:20260116T130000".to_string()),
        ],
        summary in prop_oneof![
            Just("Special Session".to_string()),
            Just("Rescheduled Occurrence".to_string()),
        ],
    ) -> String {
        format!(
            "BEGIN:VEVENT\r\nUID:evt1\r\nRECURRENCE-ID:{rec_id}\r\nSUMMARY:{summary}\r\nDTSTART:20260116T140000Z\r\nDURATION:PT1H\r\nEND:VEVENT\r\n"
        )
    }
}

prop_compose! {
    fn arb_raw_ical()(
        lines in prop::collection::vec(arb_ical_property_line(), 0..8),
        alarms in prop::collection::vec(arb_raw_valarm_block(), 0..2),
        timezones in prop::collection::vec(arb_raw_vtimezone_block(), 0..1),
        detached in prop::collection::vec(arb_raw_detached_vevent_block(), 0..2),
        trailing in prop::option::of("\\PC*"),
    ) -> String {
        let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\n");
        for tz in timezones {
            out.push_str(&tz);
        }
        out.push_str("BEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\n");
        for line in lines {
            out.push_str(&line);
            out.push_str("\r\n");
        }
        for alarm in alarms {
            out.push_str(&alarm);
        }
        out.push_str("END:VEVENT\r\n");
        for det in detached {
            out.push_str(&det);
        }
        out.push_str("END:VCALENDAR\r\n");
        if let Some(t) = trailing {
            out.push_str(&t);
        }
        out
    }
}

fn identify_oscillating_ical_property(export2: &str, export3: &str) -> String {
    let lines2: Vec<&str> = export2.lines().collect();
    let lines3: Vec<&str> = export3.lines().collect();

    for (i, (l2, l3)) in lines2.iter().zip(lines3.iter()).enumerate() {
        if l2 != l3 {
            let prop_name = l2
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated at line {}:\n  Export₂: {l2}\n  Export₃: {l3}",
                i + 1
            );
        }
    }

    if lines2.len() != lines3.len() {
        if lines2.len() > lines3.len() {
            let extra = &lines2[lines3.len()..];
            let prop_name = extra[0]
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated (lines missing in Export₃):\n  {}",
                extra.join("\n  ")
            );
        } else {
            let extra = &lines3[lines2.len()..];
            let prop_name = extra[0]
                .split([';', ':'])
                .next()
                .unwrap_or("UNKNOWN")
                .trim_start_matches(' ');
            return format!(
                "Property '{prop_name}' oscillated (spurious lines in Export₃):\n  {}",
                extra.join("\n  ")
            );
        }
    }

    "Byte/content mismatch without line divergence".to_string()
}

fn identify_oscillating_event_field(event2: &CalendarEvent, event3: &CalendarEvent) -> String {
    if event2.title != event3.title {
        return format!(
            "Field 'title' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.title, event3.title
        );
    }
    if event2.description != event3.description {
        return format!(
            "Field 'description' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.description, event3.description
        );
    }
    if event2.start != event3.start {
        return format!(
            "Field 'start' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.start, event3.start
        );
    }
    if event2.time_zone != event3.time_zone {
        return format!(
            "Field 'time_zone' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.time_zone, event3.time_zone
        );
    }
    if event2.duration != event3.duration {
        return format!(
            "Field 'duration' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.duration, event3.duration
        );
    }
    if event2.show_without_time != event3.show_without_time {
        return format!(
            "Field 'show_without_time' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.show_without_time, event3.show_without_time
        );
    }
    if event2.status != event3.status {
        return format!(
            "Field 'status' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.status, event3.status
        );
    }
    if event2.free_busy_status != event3.free_busy_status {
        return format!(
            "Field 'free_busy_status' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.free_busy_status, event3.free_busy_status
        );
    }
    if event2.priority != event3.priority {
        return format!(
            "Field 'priority' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.priority, event3.priority
        );
    }
    if event2.privacy != event3.privacy {
        return format!(
            "Field 'privacy' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.privacy, event3.privacy
        );
    }
    if event2.locations != event3.locations {
        return format!(
            "Field 'locations' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.locations, event3.locations
        );
    }
    if event2.virtual_locations != event3.virtual_locations {
        return format!(
            "Field 'virtual_locations' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.virtual_locations, event3.virtual_locations
        );
    }
    if event2.links != event3.links {
        return format!(
            "Field 'links' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.links, event3.links
        );
    }
    if event2.keywords != event3.keywords {
        return format!(
            "Field 'keywords' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.keywords, event3.keywords
        );
    }
    if event2.alerts != event3.alerts {
        return format!(
            "Field 'alerts' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.alerts, event3.alerts
        );
    }
    if event2.participants != event3.participants {
        return format!(
            "Field 'participants' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.participants, event3.participants
        );
    }
    if event2.recurrence_rule != event3.recurrence_rule {
        return format!(
            "Field 'recurrence_rule' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.recurrence_rule, event3.recurrence_rule
        );
    }
    if event2.recurrence_overrides != event3.recurrence_overrides {
        return format!(
            "Field 'recurrence_overrides' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.recurrence_overrides, event3.recurrence_overrides
        );
    }
    if event2.time_zones != event3.time_zones {
        return format!(
            "Field 'time_zones' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.time_zones, event3.time_zones
        );
    }
    if event2.extra != event3.extra {
        return format!(
            "Field 'extra' oscillated:\n  Event₂: {:?}\n  Event₃: {:?}",
            event2.extra, event3.extra
        );
    }
    format!("Unknown event field oscillated:\n  Event₂: {event2:?}\n  Event₃: {event3:?}")
}

fn assert_ical_fixpoint(export2: &str, export3: &str) -> Result<(), TestCaseError> {
    if export2 != export3 {
        let explanation = identify_oscillating_ical_property(export2, export3);
        return Err(TestCaseError::fail(format!(
            "iCalendar roundtrip failed to reach fixed point (Export₂ != Export₃)!\n{explanation}"
        )));
    }
    Ok(())
}

fn assert_event_fixpoint(
    event2: &CalendarEvent,
    event3: &CalendarEvent,
) -> Result<(), TestCaseError> {
    if event2 != event3 {
        let explanation = identify_oscillating_event_field(event2, event3);
        return Err(TestCaseError::fail(format!(
            "JSCalendar roundtrip failed to reach fixed point (Event₂ != Event₃)!\n{explanation}"
        )));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn prop_event_to_ical_never_panics(event in arb_calendar_event()) {
        let ical = event_to_ical(&event);
        prop_assert!(!ical.is_empty());
        prop_assert!(ical.starts_with("BEGIN:VCALENDAR\r\n"));
        prop_assert!(ical.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn prop_ical_to_event_never_panics_on_raw_ical(ical_text in arb_raw_ical()) {
        let _ = ical_to_event(&ical_text);
    }

    #[test]
    fn prop_ical_to_event_never_panics_on_arbitrary_string(text in ".*") {
        let _ = ical_to_event(&text);
    }

    #[test]
    fn prop_event_roundtrip_reaches_fixed_point_stability(event in arb_calendar_event()) {
        let ical1 = event_to_ical(&event);
        if let Ok(parsed1) = ical_to_event(&ical1) {
            let ical2 = event_to_ical(&parsed1);
            let parsed2 = ical_to_event(&ical2).expect("second roundtrip must parse cleanly");
            let ical3 = event_to_ical(&parsed2);
            let parsed3 = ical_to_event(&ical3).expect("third roundtrip must parse cleanly");

            assert_ical_fixpoint(&ical2, &ical3)?;
            assert_event_fixpoint(&parsed2, &parsed3)?;
        }
    }

    #[test]
    fn prop_ical_roundtrip_reaches_fixed_point_stability(ical_text in arb_raw_ical()) {
        if let Ok(parsed1) = ical_to_event(&ical_text) {
            let ical1 = event_to_ical(&parsed1);
            let parsed2 = ical_to_event(&ical1).expect("re-parsing emitted iCal must succeed");
            let ical2 = event_to_ical(&parsed2);
            let parsed3 = ical_to_event(&ical2).expect("third roundtrip must parse cleanly");
            let ical3 = event_to_ical(&parsed3);

            assert_ical_fixpoint(&ical2, &ical3)?;
            assert_event_fixpoint(&parsed2, &parsed3)?;
        }
    }

    #[test]
    fn prop_emitted_ical_lines_target_75_octets_and_are_valid_utf8(event in arb_calendar_event()) {
        let ical = event_to_ical(&event);
        for line in ical.split("\r\n") {
            // Exactly the RFC 5545 §3.1 width: calcard's own writer overshoots
            // on long properties like RRULE and when empty structured slots
            // trail a value at the boundary (stalwartlabs/calcard#25), and
            // `Component::to_ics`'s refold pass takes it back to 75.
            prop_assert!(
                line.len() <= 75,
                "Physical line exceeds maximum line length (len = {}): {:?}",
                line.len(),
                line
            );
            // Multi-byte UTF-8 code points must never be split across a fold
            prop_assert!(
                std::str::from_utf8(line.as_bytes()).is_ok(),
                "Invalid UTF-8 sequence in line slice: {:?}",
                line
            );
        }
    }

    #[test]
    fn prop_fixpoint_dates_and_timezones_domain(
        start in prop_oneof![
            Just("2026-01-15T13:00:00".to_string()),
            Just("2026-08-19T00:00:00".to_string()),
            Just("2000-02-29T12:00:00".to_string()),
            Just("1900-01-01T00:00:00".to_string()),
            "20[0-9]{2}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]",
        ],
        time_zone in arb_time_zone_string(),
        show_without_time in any::<bool>(),
    ) {
        let event = CalendarEvent {
            id: Some("E-TZ".into()),
            start: Some(start),
            time_zone: Some(time_zone),
            show_without_time: Some(show_without_time),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("dates/tz ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("dates/tz ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_recurrence_rule_domain(
        rule in arb_recurrence_rule(),
    ) {
        let event = CalendarEvent {
            id: Some("E-RRULE".into()),
            start: Some("2026-01-15T10:00:00".to_string()),
            time_zone: Some("Europe/Berlin".to_string()),
            recurrence_rule: Some(rule),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("rrule ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("rrule ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_recurrence_overrides_domain(
        overrides in prop::collection::btree_map(
            arb_override_key(),
            arb_override_patch_value(),
            1..4,
        ),
    ) {
        let event = CalendarEvent {
            id: Some("E-OVR".into()),
            title: Some("Series Title".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            time_zone: Some("Europe/Berlin".to_string()),
            duration: Some("PT1H".to_string()),
            recurrence_rule: Some(RecurrenceRule::new("weekly")),
            recurrence_overrides: Some(overrides),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("overrides ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("overrides ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_alerts_and_valarm_domain(
        alerts in prop::collection::btree_map(arb_key(), arb_alert_value(), 1..4),
    ) {
        let event = CalendarEvent {
            id: Some("E-ALERTS".into()),
            title: Some("Meeting Reminder".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            alerts: Some(alerts),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("alerts ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("alerts ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_participants_and_organizer_domain(
        participants in prop::collection::btree_map(arb_key(), arb_participant_value(), 1..5),
    ) {
        let event = CalendarEvent {
            id: Some("E-PART".into()),
            title: Some("Board Meeting".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            participants: Some(participants),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        // Participants are drawn for output rendering and not read back into JSCalendar
        let parsed1 = ical_to_event(&ical1).expect("participants ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("participants ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_locations_and_conferences_domain(
        locations in prop::collection::btree_map(arb_key(), arb_location_value(), 1..3),
        virtual_locations in prop::collection::btree_map(arb_key(), arb_virtual_location_value(), 1..3),
    ) {
        let event = CalendarEvent {
            id: Some("E-LOC".into()),
            title: Some("Hybrid Planning Session".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            locations: Some(locations),
            virtual_locations: Some(virtual_locations),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("locations ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("locations ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_attachments_and_links_domain(
        links in prop::collection::btree_map(arb_key(), arb_link_value(), 1..4),
    ) {
        let event = CalendarEvent {
            id: Some("E-LINKS".into()),
            title: Some("Presentation with Attachments".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            links: Some(links),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("links ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("links ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_categories_and_keywords_domain(
        keywords in prop::collection::btree_map(
            arb_keyword_tag(),
            prop_oneof![Just(json!(true)), Just(json!(false)), Just(json!("tag")), Just(json!(1))],
            1..6,
        ),
    ) {
        let event = CalendarEvent {
            id: Some("E-CAT".into()),
            title: Some("Categorized Event".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            keywords: Some(keywords),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("categories ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("categories ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_metadata_priority_privacy_status_domain(
        status in prop_oneof![
            Just("confirmed".to_string()),
            Just("tentative".to_string()),
            Just("cancelled".to_string()),
        ],
        free_busy_status in prop_oneof![
            Just("free".to_string()),
            Just("busy".to_string()),
        ],
        priority in 0..=9i64,
        privacy in prop_oneof![
            Just("public".to_string()),
            Just("private".to_string()),
            Just("secret".to_string()),
        ],
    ) {
        let event = CalendarEvent {
            id: Some("E-META".into()),
            title: Some("Metadata Audit".to_string()),
            start: Some("2026-01-15T10:00:00".to_string()),
            status: Some(status),
            free_busy_status: Some(free_busy_status),
            priority: Some(priority),
            privacy: Some(privacy),
            ..CalendarEvent::default()
        };
        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("metadata ical1 parse");
        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("metadata ical2 parse");
        let ical3 = event_to_ical(&parsed2);

        assert_ical_fixpoint(&ical2, &ical3)?;
        assert_event_fixpoint(&parsed1, &parsed2)?;
    }

    #[test]
    fn prop_fixpoint_real_exporter_stream_simulation(
        summary in "[A-Z][a-z]{1,10} [A-Z][a-z]{1,10}",
        zone_tzid in prop_oneof![
            Just("W. Europe Standard Time"),
            Just("Romance Standard Time"),
            Just("Eastern Standard Time"),
            Just("Tokyo Standard Time"),
            Just("/mozilla.org/20070129_1/Europe/Berlin"),
            Just("Europe/Berlin"),
        ],
        long_uid in "[a-zA-Z0-9_-]{60,100}",
    ) {
        let raw_ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Microsoft Corporation//Outlook 16.0 MIMEDIR//EN\r\nBEGIN:VTIMEZONE\r\nTZID:{zone_tzid}\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:{long_uid}\r\nSUMMARY:{summary}\r\nDTSTART;TZID=\"{zone_tzid}\":20260115T130000\r\nDURATION:PT1H\r\nLOCATION;X-JMAP-KEY=l1:Executive Boardroom\r\nCATEGORIES:Business,Planning\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:REMINDER\r\nTRIGGER:-PT15M\r\nUID:alarm-out-1\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );

        if let Ok(parsed1) = ical_to_event(&raw_ics) {
            let ical1 = event_to_ical(&parsed1);
            let parsed2 = ical_to_event(&ical1).expect("sim ical1 parse");
            let ical2 = event_to_ical(&parsed2);
            let parsed3 = ical_to_event(&ical2).expect("sim ical2 parse");
            let ical3 = event_to_ical(&parsed3);

            assert_ical_fixpoint(&ical2, &ical3)?;
            assert_event_fixpoint(&parsed2, &parsed3)?;
        }
    }

    #[test]
    fn prop_fixpoint_unmodeled_and_refused_properties_isolation(
        summary in "[A-Z][a-z]{1,10}",
    ) {
        let raw_ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Vendor//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt-unmodeled\r\nSUMMARY:{summary}\r\nDTSTART:20260115T130000Z\r\nDURATION:PT1H\r\nX-VENDOR-CUSTOM-STATUS:ACTIVE\r\nX-MICROSOFT-CDO-BUSYSTATUS:BUSY\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nSUMMARY:Email reminder\r\nATTENDEE:mailto:user@example.com\r\nTRIGGER:-P1D\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:AUDIO\r\nATTACH;VALUE=URI:Basso\r\nTRIGGER:-PT30M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Display reminder\r\nTRIGGER:-PT15M\r\nUID:valid-alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );

        if let Ok(parsed1) = ical_to_event(&raw_ics) {
            let ical1 = event_to_ical(&parsed1);
            // Unsupported alarm types (ACTION:EMAIL, ACTION:AUDIO) and vendor properties must be dropped cleanly
            prop_assert!(!ical1.contains("ACTION:EMAIL"));
            prop_assert!(!ical1.contains("ACTION:AUDIO"));
            prop_assert!(!ical1.contains("X-VENDOR-CUSTOM-STATUS"));

            let parsed2 = ical_to_event(&ical1).expect("unmodeled ical1 parse");
            let ical2 = event_to_ical(&parsed2);
            let parsed3 = ical_to_event(&ical2).expect("unmodeled ical2 parse");
            let ical3 = event_to_ical(&parsed3);

            assert_ical_fixpoint(&ical2, &ical3)?;
            assert_event_fixpoint(&parsed2, &parsed3)?;
        }
    }

    #[test]
    fn prop_value_escaping_never_double_escapes_or_loses_characters(
        prefix in "[a-zA-Z0-9 ]{0,10}",
        escapes in prop::collection::vec(
            prop_oneof![
                Just("\n"),
                Just("\r\n"),
                Just(","),
                Just(";"),
                Just("\\"),
                Just("\\n"),
                Just("\\,"),
                Just("\\;"),
                Just("\\\\"),
            ],
            1..8,
        ),
        suffix in "[a-zA-Z0-9 ]{0,10}",
    ) {
        let text = format!("{prefix}{}{suffix}", escapes.join(""));
        let event = CalendarEvent {
            id: Some("E-PROP-ESC".into()),
            title: Some(text.clone()),
            description: Some(text.clone()),
            start: Some("2026-01-15T13:00:00".to_string()),
            ..CalendarEvent::default()
        };

        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("parse emitted escaped ical");
        prop_assert_eq!(parsed1.title.as_deref(), Some(text.as_str()));
        prop_assert_eq!(parsed1.description.as_deref(), Some(text.as_str()));

        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("parse second roundtrip ical");
        let ical3 = event_to_ical(&parsed2);
        prop_assert_eq!(ical2, ical3, "Escaped value must reach fixed point");
    }

    #[test]
    fn prop_non_ascii_unicode_event_roundtrips_without_corruption(
        title in prop_oneof![
            Just("Réunion d'équipe à Paris".to_string()),
            Just("Projektplanung in München 🍺".to_string()),
            Just("Обсуждение архитектуры 🇷🇺".to_string()),
            Just("東京での会議 🌸".to_string()),
            Just("مؤتمر التقنية في دبي".to_string()),
            Just("מפगש מתכננים".to_string()),
            Just("बैठक और योजना 🇮🇳".to_string()),
            Just("🚀 Launch Event 🌟".to_string()),
            "\\PC{1,30}",
        ],
        desc_text in prop_oneof![
            Just("Café & Croissants.\n∀x ∈ ℝ: x² ≥ 0 🌟".to_string()),
            Just("Привет, мир! 🌍".to_string()),
            Just("こんにちは 世界 🌸".to_string()),
            "\\PC{1,50}",
        ],
    ) {
        let event = CalendarEvent {
            id: Some("E-UNICODE".into()),
            title: Some(title.clone()),
            description: Some(desc_text.clone()),
            start: Some("2026-01-15T13:00:00".to_string()),
            ..CalendarEvent::default()
        };

        let ical1 = event_to_ical(&event);
        let parsed1 = ical_to_event(&ical1).expect("parse non-ascii unicode ical");
        prop_assert_eq!(parsed1.title.as_deref(), Some(title.as_str()));
        prop_assert_eq!(parsed1.description.as_deref(), Some(desc_text.as_str()));

        let ical2 = event_to_ical(&parsed1);
        let parsed2 = ical_to_event(&ical2).expect("parse second roundtrip");
        let ical3 = event_to_ical(&parsed2);
        prop_assert_eq!(ical2, ical3, "Unicode event must reach fixed point");
    }
}

/// Regression test for the minimal input `prop_ical_roundtrip_reaches_fixed_point_stability`
/// found: a `CATEGORIES` tag ending in a space right before a character `calcard`
/// escapes on emit (here `;`) used to lose that space on the next parse — `calcard`'s
/// tokenizer treated it as trailing whitespace to trim, not realising the following
/// backslash-escape would keep the token going. Fixed upstream between calcard 0.3.9
/// (bundled when this was found) and 0.3.11 (bumped for this fix); pinned here as a
/// deterministic case so a future downgrade or reintroduction is caught without
/// relying on `proptest` to regenerate the same random seed.
#[test]
fn a_space_before_an_escaped_character_in_a_category_survives_a_roundtrip() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nCATEGORIES:a ;\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let parsed1 = ical_to_event(ics).expect("a plain CATEGORIES value must parse");
    let ical1 = event_to_ical(&parsed1);
    let parsed2 = ical_to_event(&ical1).expect("re-parsing the emitted iCal must succeed");
    let ical2 = event_to_ical(&parsed2);

    assert_eq!(
        ical1, ical2,
        "re-emitted iCalendar must reach a fixed-point"
    );
    assert_eq!(
        parsed2.keywords, parsed1.keywords,
        "the space before the escaped semicolon must survive the round trip"
    );
}

/// Regression test for `docs/BACKLOG.md`'s "`jmap-ical` round trip is not a
/// fixed point for a whitespace-only `CATEGORIES` value": a `CATEGORIES`
/// value that is a single backslash-escaped space parses as the literal tag
/// `" "` (calcard keeps an escaped character verbatim), which this crate then
/// writes back unescaped as a bare `CATEGORIES: ` line — and calcard's own
/// parser trims that bare, unescaped whitespace to the empty string on the
/// next parse, so the tag disappears one round trip later than an event
/// starting with no tag at all.
#[test]
fn a_category_that_is_only_whitespace_reaches_fixed_point_on_the_first_emit() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nCATEGORIES:\\ \r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let parsed1 = ical_to_event(ics).expect("an escaped-space CATEGORIES value must parse");
    let ical1 = event_to_ical(&parsed1);
    let parsed2 = ical_to_event(&ical1).expect("re-parsing the emitted iCal must succeed");
    let ical2 = event_to_ical(&parsed2);

    assert_eq!(
        ical1, ical2,
        "re-emitted iCalendar must reach a fixed-point"
    );
}

/// Same root cause as the test above, but with real content beside the
/// whitespace: a trailing space survives the first parse (escaped) but is
/// trimmed by calcard on the second (unescaped, bare) — so a tag `"0 "`
/// settles to `"0"` one round trip later than a tag starting at `"0"`
/// outright. Found by `proptest` after the whitespace-only fix above landed;
/// pinned deterministically for the same reason the sibling regression tests
/// in this file are.
#[test]
fn a_category_with_trailing_whitespace_reaches_fixed_point_on_the_first_emit() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nCATEGORIES:\\0 \r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let parsed1 = ical_to_event(ics).expect("an escaped CATEGORIES value must parse");
    let ical1 = event_to_ical(&parsed1);
    let parsed2 = ical_to_event(&ical1).expect("re-parsing the emitted iCal must succeed");
    let ical2 = event_to_ical(&parsed2);

    assert_eq!(
        ical1, ical2,
        "re-emitted iCalendar must reach a fixed-point"
    );
}
