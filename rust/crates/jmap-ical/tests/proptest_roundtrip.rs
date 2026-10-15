// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests for the JSCalendar <-> iCalendar round trip using `proptest`.
//!
//! Mirrors claude's ROADMAP item 97 round-trip property testing for the sync layer.
//! Generates valid JSCalendar events within what `jmap-ical` claims to support
//! (`MAPPED_PROPERTIES`: start, duration, timeZone, recurrence rules and overrides,
//! alerts, locations, links, keywords, status, and scalars), asserting that:
//!     ical_to_event(event_to_ical(event)) == normalize(event)
//! modulo documented lossy fields, and that the second round trip reaches a fixed point:
//!     event_to_ical(parsed1) == event_to_ical(parsed2)
//!
//! Documented lossy fields and deviations (ICAL-MAPPING citations):
//! 1. `id`: ICAL-MAPPING §9.1 (JMAP store-owned identifier, dropped on export;
//!    `ical_to_event` derives `id` from iCalendar `UID` line).
//! 2. `created` / `updated`: ICAL-MAPPING §9.1 (Store-owned metadata timestamps;
//!    `DTSTAMP` is dropped as store-owned on import).
//! 3. `participants`: ICAL-MAPPING Section 17 & ROADMAP item 102 (Operator decision;
//!    attendees are not emitted from `ical_to_event`).
//! 4. `calendar_ids`: ICAL-MAPPING §17 & JMAP §10.9.1 (JMAP container membership,
//!    managed by EDS backend, not in RFC 5545 VEVENT).
//! 5. `is_origin` / `is_draft`: ICAL-MAPPING §17 & JMAP §10.9.1 (JMAP server state flags).
//! 6. `use_default_alerts`: ICAL-MAPPING §17.3 (VALARM has no default alarm syntax).
//! 7. `color`, `locale`, `localizations`: ICAL-MAPPING §13.15, §17 (Client/server metadata
//!    not modeled in RFC 5545 VEVENT).
//! 8. `Location.coordinates` / `Location.description`: ICAL-MAPPING §12.3 (RFC 5545 LOCATION
//!    is a single text property).
//! 9. `Link.size` / `Link.title` / `Link.cid`: ICAL-MAPPING §12.5 (RFC 5545 ATTACH carries
//!    only URI and FMTTYPE).

use std::collections::BTreeMap;

use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use proptest::prelude::*;
use serde_json::Value;

/// Normalizes an event by stripping documented lossy fields before asserting equality.
fn normalize_lossy_fields(mut event: CalendarEvent) -> CalendarEvent {
    // 1. Store/container-owned fields (ICAL-MAPPING §9.1, §17)
    event.calendar_ids = None;
    event.created = None;
    event.updated = None;
    event.participants = None;
    event.is_origin = None;
    event.is_draft = None;
    event.use_default_alerts = None;
    event.color = None;
    event.locale = None;
    event.localizations = None;
    event.time_zones = None;
    event.extra = Default::default();

    // ical_to_event stamps version as "2.0" and event_type as "Event"
    event.event_type = Some("Event".to_owned());
    event.version = Some("2.0".to_owned());

    // If input had no id but had uid, UID in ics was uid, so ical_to_event read id from UID.
    if event.id.is_none() && event.uid.is_some() {
        event.id = event.uid.as_ref().map(|u| Id::from(u.as_str()));
    }

    // RecurrenceRule normalizations (RFC 5545 §3.3.10 defaults)
    if let Some(rule) = &mut event.recurrence_rule {
        rule.rule_type = Some("RecurrenceRule".to_owned());
        if rule.interval == Some(1) {
            rule.interval = None;
        }
        if rule.first_day_of_week.as_deref() == Some("mo") {
            rule.first_day_of_week = None;
        }
    }

    event
}

/// Generates valid ISO-8601 local date-time strings without fractional seconds
/// (JSCalendar-bis §1.5.5 / RFC 5545 DATE-TIME).
fn arb_local_datetime() -> impl Strategy<Value = String> {
    (
        2020..2035u32,
        1..=12u32,
        1..=28u32,
        0..=23u32,
        0..=59u32,
        0..=59u32,
    )
        .prop_map(|(y, m, d, h, min, s)| format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}"))
}

/// Generates valid ISO-8601 duration strings supported by iCalendar DURATION (RFC 5545 §3.3.6).
fn arb_duration() -> impl Strategy<Value = String> {
    prop_oneof![
        (1..=14u32).prop_map(|d| format!("P{d}D")),
        (1..=23u32).prop_map(|h| format!("PT{h}H")),
        (1..=59u32).prop_map(|m| format!("PT{m}M")),
        (1..=12u32, 1..=59u32).prop_map(|(h, m)| format!("PT{h}H{m}M")),
        (1..=7u32).prop_map(|w| format!("P{w}W")),
    ]
}

/// Generates valid IANA time zone identifiers or UTC.
fn arb_timezone() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some("Etc/UTC".to_string())),
        Just(Some("Europe/Berlin".to_string())),
        Just(Some("Europe/London".to_string())),
        Just(Some("America/New_York".to_string())),
        Just(Some("America/Los_Angeles".to_string())),
        Just(Some("Asia/Tokyo".to_string())),
        Just(Some("Australia/Sydney".to_string())),
    ]
}

/// Generates safe text strings without carriage returns, NULs, or leading/trailing whitespace.
fn arb_clean_text() -> impl Strategy<Value = String> {
    "[A-Za-z0-9][A-Za-z0-9 .,!?:;'-]{0,60}[A-Za-z0-9]"
}

/// Generates valid scalar properties supported by jmap-ical.
fn arb_scalar_event() -> impl Strategy<Value = CalendarEvent> {
    (
        prop::option::of("[a-z0-9_]{3,16}").prop_map(|opt| opt.map(Id::from)),
        prop::option::of("[a-z0-9-]{8,36}"),
        prop::option::of(arb_clean_text()),
        prop::option::of(arb_clean_text()),
        arb_local_datetime(),
        arb_duration(),
        arb_timezone(),
        prop_oneof![
            Just(None),
            Just(Some("confirmed".to_string())),
            Just(Some("tentative".to_string())),
            Just(Some("cancelled".to_string())),
        ],
        prop_oneof![
            Just(None),
            Just(Some("busy".to_string())),
            Just(Some("free".to_string())),
        ],
        prop_oneof![
            Just(None),
            Just(Some(1i64)),
            Just(Some(5i64)),
            Just(Some(9i64)),
        ],
        prop_oneof![
            Just(None),
            Just(Some("public".to_string())),
            Just(Some("private".to_string())),
            Just(Some("secret".to_string())),
        ],
    )
        .prop_map(
            |(
                id,
                uid,
                title,
                description,
                start,
                duration,
                time_zone,
                status,
                free_busy_status,
                priority,
                privacy,
            )| {
                CalendarEvent {
                    id,
                    uid,
                    title,
                    description,
                    start: Some(start),
                    duration: Some(duration),
                    time_zone,
                    status,
                    free_busy_status,
                    priority,
                    privacy,
                    ..CalendarEvent::default()
                }
            },
        )
}

/// Generates valid weekdays without ordinals.
fn arb_plain_weekday() -> impl Strategy<Value = NDay> {
    prop_oneof![
        Just("mo"),
        Just("tu"),
        Just("we"),
        Just("th"),
        Just("fr"),
        Just("sa"),
        Just("su"),
    ]
    .prop_map(|d| NDay {
        day_type: Some("NDay".to_owned()),
        day: d.to_owned(),
        nth_of_period: None,
        extra: Default::default(),
    })
}

/// Generates valid NDay objects with optional ordinal for monthly/yearly rules.
fn arb_monthly_nday() -> impl Strategy<Value = NDay> {
    (
        prop_oneof![
            Just("mo"),
            Just("tu"),
            Just("we"),
            Just("th"),
            Just("fr"),
            Just("sa"),
            Just("su"),
        ],
        prop_oneof![
            Just(None),
            Just(Some(1i32)),
            Just(Some(2i32)),
            Just(Some(-1i32)),
        ],
    )
        .prop_map(|(d, nth)| NDay {
            day_type: Some("NDay".to_owned()),
            day: d.to_owned(),
            nth_of_period: nth,
            extra: Default::default(),
        })
}

/// Generates valid RecurrenceRule objects supported by iCalendar RRULE (RFC 5545 §3.3.10).
fn arb_valid_recurrence_rule() -> impl Strategy<Value = RecurrenceRule> {
    prop_oneof![
        // Daily rule
        (prop::option::of(2..=30u32), prop::option::of(1..=100u32),).prop_map(
            |(interval, count)| RecurrenceRule {
                rule_type: Some("RecurrenceRule".to_owned()),
                frequency: "daily".to_owned(),
                interval,
                count,
                ..RecurrenceRule::default()
            }
        ),
        // Weekly rule (BYDAY with plain weekdays only per RFC 5545 §3.3.10)
        (
            prop::option::of(2..=30u32),
            prop::option::of(1..=100u32),
            prop::option::of(prop::collection::vec(arb_plain_weekday(), 1..=3)),
            prop_oneof![Just(None), Just(Some("su".to_string()))],
        )
            .prop_map(
                |(interval, count, by_day, first_day_of_week)| RecurrenceRule {
                    rule_type: Some("RecurrenceRule".to_owned()),
                    frequency: "weekly".to_owned(),
                    interval,
                    count,
                    by_day,
                    first_day_of_week,
                    ..RecurrenceRule::default()
                }
            ),
        // Monthly rule (BYDAY with optional ordinal, or BYMONTHDAY)
        (
            prop::option::of(2..=30u32),
            prop::option::of(1..=100u32),
            prop_oneof![
                prop::collection::vec(arb_monthly_nday(), 1..=2).prop_map(Some),
                Just(None),
            ],
            prop_oneof![
                prop::collection::vec(1..=28i32, 1..=2).prop_map(Some),
                Just(None),
            ],
        )
            .prop_map(|(interval, count, by_day, by_month_day)| RecurrenceRule {
                rule_type: Some("RecurrenceRule".to_owned()),
                frequency: "monthly".to_owned(),
                interval,
                count,
                by_day,
                by_month_day,
                ..RecurrenceRule::default()
            }),
        // Yearly rule (BYMONTH, BYDAY with optional ordinal)
        (
            prop::option::of(2..=30u32),
            prop::option::of(1..=100u32),
            prop::option::of(prop::collection::vec(
                (1..=12u32).prop_map(|m| m.to_string()),
                1..=3,
            )),
            prop_oneof![
                prop::collection::vec(arb_monthly_nday(), 1..=2).prop_map(Some),
                Just(None),
            ],
        )
            .prop_map(|(interval, count, by_month, by_day)| RecurrenceRule {
                rule_type: Some("RecurrenceRule".to_owned()),
                frequency: "yearly".to_owned(),
                interval,
                count,
                by_month,
                by_day,
                ..RecurrenceRule::default()
            }),
    ]
}

/// Generates valid events with recurrence rules.
fn arb_recurring_event() -> impl Strategy<Value = CalendarEvent> {
    (arb_scalar_event(), arb_valid_recurrence_rule()).prop_map(|(mut ev, rrule)| {
        ev.recurrence_rule = Some(rrule);
        ev
    })
}

/// Generates valid Alert maps (RFC 8984 §4.5.2 / RFC 5545 VALARM DISPLAY).
fn arb_alerts() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        prop_oneof![Just("a1".to_string()), Just("a2".to_string())],
        (
            prop_oneof![
                Just("-PT15M".to_string()),
                Just("-PT30M".to_string()),
                Just("-PT1H".to_string()),
                Just("-P1D".to_string()),
            ],
            prop_oneof![Just(None), Just(Some("end".to_string()))],
        )
            .prop_map(|(offset, rel)| {
                let mut trigger = serde_json::json!({
                    "@type": "OffsetTrigger",
                    "offset": offset,
                });
                if let Some(r) = rel {
                    trigger["relativeTo"] = serde_json::json!(r);
                }
                serde_json::json!({
                    "@type": "Alert",
                    "action": "display",
                    "trigger": trigger,
                })
            }),
        1..=2,
    )
}

/// Generates valid Location maps (RFC 8984 §4.2.5 / RFC 5545 LOCATION with X-JMAP-KEY).
fn arb_locations() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        Just("l1".to_string()),
        arb_clean_text().prop_map(|name| {
            serde_json::json!({
                "@type": "Location",
                "name": name,
            })
        }),
        1..=1,
    )
}

/// Generates valid VirtualLocation maps (RFC 8984 §4.2.6 / RFC 7986 CONFERENCE with X-JMAP-KEY).
fn arb_virtual_locations() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        prop_oneof![Just("v1".to_string()), Just("v2".to_string())],
        (
            prop_oneof![
                Just("https://meet.example.com/daily".to_string()),
                Just("https://zoom.example.com/j/12345".to_string()),
            ],
            prop::option::of(arb_clean_text()),
        )
            .prop_map(|(uri, name)| {
                let mut map = serde_json::json!({
                    "@type": "VirtualLocation",
                    "uri": uri,
                });
                if let Some(n) = name {
                    map["name"] = serde_json::json!(n);
                }
                map
            }),
        1..=2,
    )
}

/// Generates valid Link maps (JSCalendar-bis §1.5.11 / RFC 5545 ATTACH and IMAGE with X-JMAP-KEY).
fn arb_links() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        prop_oneof![Just("k1".to_string()), Just("k2".to_string())],
        prop_oneof![
            // Attachment link
            (
                prop_oneof![
                    Just("https://example.com/agenda.pdf".to_string()),
                    Just("https://example.com/notes.txt".to_string()),
                ],
                prop_oneof![
                    Just(None),
                    Just(Some("application/pdf".to_string())),
                    Just(Some("text/plain".to_string())),
                ],
            )
                .prop_map(|(href, content_type)| {
                    let mut link = serde_json::json!({
                        "@type": "Link",
                        "href": href,
                    });
                    if let Some(ct) = content_type {
                        link["contentType"] = serde_json::json!(ct);
                    }
                    link
                }),
            // Image link with display set
            (
                Just("https://example.com/badge.png".to_string()),
                prop_oneof![Just("badge"), Just("thumbnail"), Just("graphic"),],
            )
                .prop_map(|(href, disp)| {
                    serde_json::json!({
                        "@type": "Link",
                        "href": href,
                        "rel": "icon",
                        "display": { disp: true },
                    })
                }),
        ],
        1..=2,
    )
}

/// Generates valid Keywords maps (RFC 8984 §4.2.9 / RFC 5545 CATEGORIES).
fn arb_keywords() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        prop_oneof![
            Just("work".to_string()),
            Just("urgent".to_string()),
            Just("meeting".to_string()),
            Just("project".to_string()),
        ],
        Just(Value::Bool(true)),
        1..=3,
    )
}

/// Generates valid recurrence overrides (RFC 8984 §4.3.5 / RFC 5545 EXDATE and detached VEVENT).
/// Override patches specify properties that differ from the series baseline.
fn arb_recurrence_overrides() -> impl Strategy<Value = BTreeMap<String, Value>> {
    prop::collection::btree_map(
        prop_oneof![
            Just("2026-06-16T10:00:00".to_string()),
            Just("2026-06-23T10:00:00".to_string()),
        ],
        prop_oneof![
            // Excluded instance (EXDATE)
            Just(serde_json::json!({"excluded": true})),
            // Modified instance (RECURRENCE-ID detached component) differing from series baseline
            prop_oneof![
                Just(serde_json::json!({"status": "cancelled"})),
                Just(serde_json::json!({"title": "Shifted Standup"})),
                Just(serde_json::json!({"priority": 1})),
                Just(serde_json::json!({"title": "Shifted Standup", "status": "cancelled"})),
            ],
        ],
        1..=2,
    )
}

/// Generates composite events combining all supported properties.
fn arb_composite_event() -> impl Strategy<Value = CalendarEvent> {
    (
        arb_scalar_event(),
        prop::option::of(arb_valid_recurrence_rule()),
        prop::option::of(arb_alerts()),
        prop::option::of(arb_locations()),
        prop::option::of(arb_virtual_locations()),
        prop::option::of(arb_links()),
        prop::option::of(arb_keywords()),
    )
        .prop_map(|(mut ev, rrule, alerts, locs, vlocs, links, keywords)| {
            ev.recurrence_rule = rrule;
            ev.alerts = alerts;
            ev.locations = locs;
            ev.virtual_locations = vlocs;
            ev.links = links;
            ev.keywords = keywords;
            ev
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 1: Core scalars (start, duration, timeZone, status, freeBusyStatus,
    /// priority, privacy, title, description) round-trip losslessly modulo documented lossy fields.
    #[test]
    fn prop_roundtrip_start_duration_timezone_and_scalars(event in arb_scalar_event()) {
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("iCalendar serialization must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event must equal normalized original");

        // Second round trip must reach a fixed point
        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");

        prop_assert_eq!(&parsed, &parsed2, "roundtrip must reach fixed point on second pass");
        prop_assert_eq!(&ics, &ics2, "iCalendar emission must reach fixed point");
    }

    /// Property 2: Recurrence rules (frequency, interval, count, byDay, byMonthDay,
    /// byMonth, firstDayOfWeek) round-trip losslessly modulo RFC 5545 default omissions.
    #[test]
    fn prop_roundtrip_recurrence_rule(event in arb_recurring_event()) {
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("recurring event iCal must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed recurring event must equal normalized original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");

        prop_assert_eq!(&parsed, &parsed2, "recurring event roundtrip must reach fixed point");
        prop_assert_eq!(&ics, &ics2, "recurring event iCal emission must reach fixed point");
    }

    /// Property 3: Alerts with OffsetTrigger and action: display round-trip losslessly.
    #[test]
    fn prop_roundtrip_alerts(
        mut event in arb_scalar_event(),
        alerts in arb_alerts(),
    ) {
        event.alerts = Some(alerts);
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("event with alerts must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event with alerts must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }

    /// Property 4: Physical locations (with name) and virtual locations (with URI and name) round-trip.
    #[test]
    fn prop_roundtrip_locations(
        mut event in arb_scalar_event(),
        locations in arb_locations(),
        virtual_locations in arb_virtual_locations(),
    ) {
        event.locations = Some(locations);
        event.virtual_locations = Some(virtual_locations);
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("event with locations must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event with locations must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }

    /// Property 5: External links (attachments with contentType, image links with display set) round-trip.
    #[test]
    fn prop_roundtrip_links(
        mut event in arb_scalar_event(),
        links in arb_links(),
    ) {
        event.links = Some(links);
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("event with links must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event with links must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }

    /// Property 6: Keywords (categories) round-trip losslessly.
    #[test]
    fn prop_roundtrip_keywords(
        mut event in arb_scalar_event(),
        keywords in arb_keywords(),
    ) {
        event.keywords = Some(keywords);
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("event with keywords must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event with keywords must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }

    /// Property 7: Recurrence overrides (exclusions via EXDATE, patched overrides via detached VEVENT).
    #[test]
    fn prop_roundtrip_recurrence_overrides(
        mut event in arb_scalar_event(),
        rrule in arb_valid_recurrence_rule(),
        overrides in arb_recurrence_overrides(),
    ) {
        event.start = Some("2026-06-09T10:00:00".to_string());
        event.duration = Some("PT1H".to_string());
        event.status = Some("confirmed".to_string());
        event.title = Some("Series Title".to_string());
        event.priority = Some(5);
        event.recurrence_rule = Some(rrule);
        event.recurrence_overrides = Some(overrides);

        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("event with overrides must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed event with overrides must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }

    /// Property 8: Composite events combining all supported properties round-trip losslessly.
    #[test]
    fn prop_roundtrip_composite_event(event in arb_composite_event()) {
        let expected = normalize_lossy_fields(event.clone());
        let ics = event_to_ical(&event);
        let parsed = ical_to_event(&ics).expect("composite event must parse cleanly");

        prop_assert_eq!(&parsed, &expected, "parsed composite event must equal original");

        let ics2 = event_to_ical(&parsed);
        let parsed2 = ical_to_event(&ics2).expect("second roundtrip must parse cleanly");
        prop_assert_eq!(&parsed, &parsed2);
        prop_assert_eq!(&ics, &ics2);
    }
}
