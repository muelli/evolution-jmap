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

#[test]
fn recurrence_rule_days_of_the_month_roundtrip() {
    // `byMonthDay` is modeled for the same reason `byDay` is: an `RRULE` spells
    // it as `BYMONTHDAY`, so "the 15th and the last day of every month" is a
    // rule the mapping can both show and hand back unchanged. The negative
    // value is the one RFC 8984 §4.3.3 counts from the end of the month.
    let value = fixture("calendars/recurrence_rule_days_of_month.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "monthly");
    assert_eq!(rule.by_month_day.as_deref(), Some(&[15, -1][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_days_of_the_year_roundtrip() {
    // `byYearDay` is iCalendar's `BYYEARDAY` — "the first and the last day of the
    // year", the negative value counting back from 31 December as RFC 8984 §4.3.3
    // has it. A number, unlike `byMonth`: a day of the year has no leap-month
    // spelling to preserve.
    let value = fixture("calendars/recurrence_rule_days_of_year.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "yearly");
    assert_eq!(rule.by_year_day.as_deref(), Some(&[1, -1][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_weeks_of_the_year_roundtrip() {
    // `byWeekNo` is iCalendar's `BYWEEKNO` — "the first and the last week of the
    // year", the negative value counting back from the end of the year as RFC 8984
    // §4.3.3 has it. Which dates those weeks hold depends on `firstDayOfWeek`, so
    // the fixture states one: RFC 5545 §3.3.10 numbers weeks by ISO 8601, where a
    // week belongs to the year holding most of its days, counted from `WKST`.
    let value = fixture("calendars/recurrence_rule_weeks_of_year.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "yearly");
    assert_eq!(rule.by_week_no.as_deref(), Some(&[1, -1][..]));
    assert_eq!(rule.first_day_of_week.as_deref(), Some("su"));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_hours_roundtrip() {
    // `byHour` is iCalendar's `BYHOUR` — "twice a day, at 09:00 and at 17:00".
    // RFC 8984 §4.3.3 has it as `UnsignedInt[]`, so unlike the days of the month
    // and the weeks of the year there is no backwards count to preserve: RFC 5545
    // §3.3.10's `hour` gives no way to name an hour from the end of the day.
    let value = fixture("calendars/recurrence_rule_hours.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "daily");
    assert_eq!(rule.by_hour.as_deref(), Some(&[9, 17][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_set_position_roundtrip() {
    // `bySetPosition` is iCalendar's `BYSETPOS` — "the last Friday of the
    // month". It is the one part of RFC 8984 §4.3.3 that names no date of its
    // own: it selects out of the set the other `by*` properties expand to, so
    // the fixture states the `byDay` it selects from, which RFC 5545 §3.3.10
    // also requires.
    let value = fixture("calendars/recurrence_rule_set_position.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "monthly");
    assert_eq!(rule.by_set_position.as_deref(), Some(&[-1][..]));
    assert_eq!(rule.by_day.as_ref().unwrap()[0].day, "fr");
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_months_roundtrip() {
    // `byMonth` is the months of the year a rule repeats in — iCalendar's
    // `BYMONTH`. RFC 8984 §4.3.3 holds each as a *string*, not a number, so
    // that a leap month in a non-Gregorian calendar can be spelled `5L`; the
    // model keeps the string it was given rather than a number it would have to
    // spell back.
    let value = fixture("calendars/recurrence_rule_months.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "yearly");
    assert_eq!(
        rule.by_month.as_deref(),
        Some(&["3".to_owned(), "9".to_owned()][..])
    );
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_first_day_of_week_roundtrip() {
    // `firstDayOfWeek` is iCalendar's `WKST` — the day a week is counted from,
    // which RFC 5545 §3.3.10 says decides where a fortnightly series' second week
    // begins. "Every other Tuesday, weeks starting on Sunday" is a different set
    // of dates from the same rule counted from Monday, so the day has to be
    // modeled rather than parked in `extra`.
    let value = fixture("calendars/recurrence_rule_first_day_of_week.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "weekly");
    assert_eq!(rule.first_day_of_week.as_deref(), Some("su"));
    assert!(rule.extra.is_empty());
}
