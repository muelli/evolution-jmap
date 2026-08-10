// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the JMAP Calendars draft types (JSCalendar,
//! RFC 8984).

#![cfg(feature = "calendars")]

use jmap_proto::calendars::{CalendarEvent, NDay, RecurrenceRule};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn roundtrip<T>(value: &Value) -> Value
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(value.clone()).expect("deserialize");
    serde_json::to_value(&typed).expect("serialize")
}

#[test]
fn calendar_event_roundtrip() {
    let value = fixture("calendars/calendar_event.json");
    assert_eq!(roundtrip::<CalendarEvent>(&value), value);

    let event: CalendarEvent = serde_json::from_value(value).unwrap();
    assert_eq!(event.event_type.as_deref(), Some("Event"));
    assert_eq!(event.title.as_deref(), Some("Team sync"));
    assert_eq!(event.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));
    let rules = event.recurrence_rules.as_ref().unwrap();
    assert_eq!(rules[0].frequency, "weekly");
    assert_eq!(rules[0].by_day.as_deref(), Some(&[NDay::new("th")][..]));
    // An override's patch stays JSON: "this instance is off" and "this instance
    // was edited" are both PatchObjects, and only the caller knows which of
    // them it can represent.
    let overrides = event.recurrence_overrides.as_ref().unwrap();
    assert_eq!(
        overrides.get("2026-01-22T13:00:00"),
        Some(&serde_json::json!({"excluded": true}))
    );
    assert_eq!(
        overrides["2026-01-29T13:00:00"]["title"],
        serde_json::json!("Team sync (long)")
    );
    // Unmodeled JSCalendar properties (participants, sequence) survive.
    assert!(event.extra.contains_key("participants"));
    assert!(event.extra.contains_key("sequence"));
}

#[test]
fn recurrence_rule_roundtrip() {
    let value = fixture("calendars/recurrence_rule.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "monthly");
    assert_eq!(rule.interval, Some(2));
    assert_eq!(rule.count, Some(10));
    // `byDay` is modeled rather than parked in `extra`: an `RRULE` spells it,
    // so the mapping has to be able to see it — and to hand it back unchanged,
    // `@type` and all, when a save replaces the property whole.
    let days = rule.by_day.as_deref().expect("byDay");
    assert_eq!(
        days,
        [NDay {
            nth_of_period: Some(2),
            ..NDay::new("we")
        }]
    );
    assert_eq!(days[0].day_type.as_deref(), Some("NDay"));
    assert!(rule.extra.is_empty());
}
