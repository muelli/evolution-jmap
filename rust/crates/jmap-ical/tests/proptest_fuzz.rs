// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structure-aware fuzzing of the JSCalendar ↔ iCalendar mapping using `proptest`.
//!
//! Asserts:
//! 1. `event_to_ical` never panics on arbitrary `CalendarEvent` instances.
//! 2. `ical_to_event` never panics on arbitrary strings or arbitrary iCalendar envelopes.
//! 3. Round-trip stability: Emitting an event, parsing it back, and re-emitting reaches a fixed point.

use std::collections::BTreeMap;

use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use proptest::prelude::*;
use serde_json::json;

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
            extra: BTreeMap::new(),
        }
    }
}

fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,8}",
        Just("loc1".to_string()),
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
        time_zone in prop::option::of(prop_oneof![
            Just("Etc/UTC".to_string()),
            Just("Europe/Berlin".to_string()),
            Just("America/New_York".to_string()),
            Just("Asia/Tokyo".to_string()),
            Just("/custom/zone_1".to_string()),
            Just("Floating".to_string()),
            "[A-Za-z0-9/_+-]{1,20}",
        ]),
        duration in prop::option::of(prop_oneof![
            Just("PT1H".to_string()),
            Just("PT30M".to_string()),
            Just("P1D".to_string()),
            Just("P1W".to_string()),
            Just("PT0S".to_string()),
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

prop_compose! {
    fn arb_event_maps()(
        locations in prop::option::of(prop::collection::btree_map(
            arb_key(),
            prop_oneof![
                Just(json!({"name": "Conference Room A"})),
                Just(json!({"name": "HQ", "description": "Main office"})),
                Just(json!({"name": ""})),
                Just(json!(123)),
            ],
            0..3,
        )),
        virtual_locations in prop::option::of(prop::collection::btree_map(
            arb_key(),
            prop_oneof![
                Just(json!({"uri": "https://meet.example.com/room", "name": "Video call"})),
                Just(json!({"uri": "tel:+1234567890"})),
                Just(json!({"name": "Audio bridge"})),
            ],
            0..3,
        )),
        links in prop::option::of(prop::collection::btree_map(
            arb_key(),
            prop_oneof![
                Just(json!({"href": "https://example.com/agenda.pdf", "contentType": "application/pdf"})),
                Just(json!({"href": "https://example.com/photo.png", "display": "badge"})),
                Just(json!({"href": "cid:doc123"})),
            ],
            0..3,
        )),
        keywords in prop::option::of(prop::collection::btree_map(
            "[a-zA-Z0-9_-]{1,10}",
            prop_oneof![Just(json!(true)), Just(json!(false)), Just(json!("tag")), Just(json!(1))],
            0..4,
        )),
        alerts in prop::option::of(prop::collection::btree_map(
            arb_key(),
            prop_oneof![
                Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}})),
                Just(json!({"trigger": {"@type": "OffsetTrigger", "offset": "PT0S", "relativeTo": "end"}})),
                Just(json!({"trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-15T12:45:00Z"}})),
                Just(json!({"action": "display", "description": "Reminder"})),
            ],
            0..3,
        )),
        participants in prop::option::of(prop::collection::btree_map(
            arb_key(),
            prop_oneof![
                Just(json!({"name": "Alice", "email": "alice@example.com", "roles": {"owner": true}})),
                Just(json!({"name": "Bob", "email": "bob@example.com", "participationStatus": "accepted"})),
                Just(json!({"sendTo": {"imip": "mailto:carol@example.com"}, "kind": "individual"})),
            ],
            0..3,
        )),
    ) -> (
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
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

prop_compose! {
    fn arb_event_recurrence()(
        recurrence_rules in prop::option::of(prop::collection::vec(arb_recurrence_rule(), 0..2)),
        recurrence_overrides in prop::option::of(prop::collection::btree_map(
            Just("2026-01-16T13:00:00".to_string()),
            prop_oneof![
                Just(json!({"excluded": true})),
                Just(json!({"title": "Special Session", "duration": "PT2H"})),
                Just(json!({})),
            ],
            0..2,
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
        Option<Vec<RecurrenceRule>>,
        Option<BTreeMap<String, serde_json::Value>>,
        Option<BTreeMap<String, serde_json::Value>>,
    ) {
        (recurrence_rules, recurrence_overrides, time_zones)
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
                (recurrence_rules, recurrence_overrides, time_zones),
            )| {
                CalendarEvent {
                    id: id.map(Into::into),
                    uid,
                    event_type: Some("Event".to_string()),
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
                    recurrence_rules,
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
            Just("UID".to_string()),
            Just("X-CUSTOM".to_string()),
            "[A-Z0-9-]{1,12}",
        ],
        params in prop::collection::vec(
            prop_oneof![
                Just(";TZID=Europe/Berlin".to_string()),
                Just(";TZID=Etc/UTC".to_string()),
                Just(";VALUE=DATE".to_string()),
                Just(";CN=\"Alice Example\"".to_string()),
                Just(";ROLE=REQ-PARTICIPANT".to_string()),
                Just(";PARTSTAT=ACCEPTED".to_string()),
                ";[A-Z-]+=[A-Za-z0-9-]+",
            ],
            0..3,
        ),
        value in "\\PC*",
    ) -> String {
        let param_str = params.join("");
        format!("{name}{param_str}:{value}")
    }
}

prop_compose! {
    fn arb_raw_ical()(
        lines in prop::collection::vec(arb_ical_property_line(), 0..10),
        trailing in prop::option::of("\\PC*"),
    ) -> String {
        let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\n");
        for line in lines {
            out.push_str(&line);
            out.push_str("\r\n");
        }
        out.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        if let Some(t) = trailing {
            out.push_str(&t);
        }
        out
    }
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
            prop_assert_eq!(ical2, ical3, "iCalendar emission must reach a fixed-point");
        }
    }

    #[test]
    fn prop_ical_roundtrip_reaches_fixed_point_stability(ical_text in arb_raw_ical()) {
        if let Ok(parsed1) = ical_to_event(&ical_text) {
            let ical1 = event_to_ical(&parsed1);
            let parsed2 = ical_to_event(&ical1).expect("re-parsing emitted iCal must succeed");
            let ical2 = event_to_ical(&parsed2);
            prop_assert_eq!(ical1, ical2, "re-emitted iCalendar must reach a fixed-point");
        }
    }
}
