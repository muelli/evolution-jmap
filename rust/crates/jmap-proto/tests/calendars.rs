// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the JMAP Calendars draft types (JSCalendar,
//! RFC 8984).

#![cfg(feature = "calendars")]

use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter, NDay, RecurrenceRule};
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
    let rule = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rule.frequency, "weekly");
    assert_eq!(rule.by_day.as_deref(), Some(&[NDay::new("th")][..]));
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
    // `locations` is modeled but left as JSON: a Location holds coordinates,
    // links and types the iCalendar mapping cannot draw, and the save path has
    // to see them in order to patch the one field it can and leave the rest.
    let locations = event.locations.as_ref().expect("locations");
    assert_eq!(
        locations.get("loc1"),
        Some(&serde_json::json!({
            "@type": "Location",
            "name": "Room 42",
            "coordinates": "geo:52.520008,13.404954",
        }))
    );
    // `keywords` is an RFC 8984 §1.4.3 Set — the keys are the tags and every
    // value is `true`. The values are held as JSON rather than as `bool` so that
    // one server answering something else for one event cannot fail the whole
    // `CalendarEvent/get` response and take the calendar with it.
    let keywords = event.keywords.as_ref().expect("keywords");
    assert_eq!(keywords.keys().collect::<Vec<_>>(), ["offsite", "planning"]);
    assert!(keywords.values().all(|set| set == &Value::Bool(true)));
    // `alerts` is modeled but left as JSON, for the reason `locations` is: an
    // Alert holds a trigger that is one of two object types and an
    // `acknowledged` timestamp a `VALARM` this mapping writes cannot carry, and
    // the save path has to see them in order to refuse to replace the property.
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.keys().collect::<Vec<_>>(), ["a1", "a2"]);
    assert_eq!(
        alerts["a1"]["trigger"],
        serde_json::json!({"@type": "OffsetTrigger", "offset": "-PT15M"})
    );
    assert_eq!(alerts["a2"]["acknowledged"], "2026-01-15T11:01:00Z");
    // `participants` is modeled but left as JSON, for the reason `locations` is
    // and then some: a Participant holds a `sendTo` map, a set of `roles`, a
    // `kind`, delegations and a scheduling agent, of which iCalendar spells the
    // part it shares on the parameters of an `ATTENDEE` line. The mapping draws
    // the guest list and never reads it back, so the shape matters only in that
    // it survives untouched.
    let participants = event.participants.as_ref().expect("participants");
    assert_eq!(
        participants.get("p1"),
        Some(&serde_json::json!({
            "@type": "Participant",
            "name": "Vera",
            "sendTo": {"imip": "mailto:vera@example.com"},
            "roles": {"attendee": true},
        }))
    );
    // An unmodeled JSCalendar property (sequence) survives.
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
fn recurrence_rule_minutes_and_seconds_roundtrip() {
    // `byMinute` and `bySecond` are iCalendar's `BYMINUTE` and `BYSECOND` — "on
    // the hour and the half hour, on the second". Unsigned like `byHour`, and
    // with the same absence of a backwards count; the ranges differ, RFC 5545
    // §3.3.10's `minutes` being 0 to 59 and its `seconds` 0 to 60, the sixtieth
    // second being the leap second UTC occasionally inserts.
    //
    // With these two the rule is modeled to the bottom of RFC 8984 §4.3.3 but
    // for `rscale` and `skip`, which name a non-Gregorian calendar.
    let value = fixture("calendars/recurrence_rule_minutes_and_seconds.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "hourly");
    assert_eq!(rule.by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rule.by_second.as_deref(), Some(&[0][..]));
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

#[test]
fn calendar_event_simple_sets_every_field() {
    let event = CalendarEvent::simple("C1", "Team sync", "2026-01-15T13:00:00", "PT1H");
    let calendar_ids: Vec<_> = event
        .calendar_ids
        .as_ref()
        .unwrap()
        .iter()
        .map(|(id, included)| (id.as_str(), *included))
        .collect();
    assert_eq!(calendar_ids, [("C1", true)]);
    assert_eq!(event.event_type.as_deref(), Some("Event"));
    assert_eq!(event.title.as_deref(), Some("Team sync"));
    assert_eq!(event.start.as_deref(), Some("2026-01-15T13:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Etc/UTC"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
}

#[test]
fn calendar_event_query_filter_in_calendar_sets_only_that_field() {
    let filter = CalendarEventQueryFilter::in_calendar("C1");
    assert_eq!(filter.in_calendar.as_ref().unwrap().as_str(), "C1");
    assert_eq!(filter.after, None);
    assert_eq!(filter.before, None);
}

#[test]
fn calendar_event_query_filter_time_range_sets_after_and_before() {
    let filter =
        CalendarEventQueryFilter::time_range("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z");
    assert_eq!(filter.after.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!(filter.before.as_deref(), Some("2026-02-01T00:00:00Z"));
    assert_eq!(filter.in_calendar, None);
}

#[test]
fn calendar_set_error_has_event_code() {
    assert_eq!(
        jmap_proto::calendars::calendar_set_error::HAS_EVENT,
        "calendarHasEvent"
    );
}

#[test]
fn calendar_properties_and_constants_cover_jmap_calendars_draft() {
    use jmap_proto::calendars::*;

    let cal: Calendar = serde_json::from_value(serde_json::json!({
        "name": "Work",
        "isVisible": true,
        "includeInAvailability": "allExceptDeclined"
    }))
    .unwrap();

    assert_eq!(cal.name, "Work");
    assert_eq!(cal.is_visible, Some(true));
    assert_eq!(
        cal.include_in_availability.as_deref(),
        Some("allExceptDeclined")
    );

    assert_eq!(include_in_availability::ALL, "all");
    assert_eq!(
        include_in_availability::ALL_EXCEPT_DECLINED,
        "allExceptDeclined"
    );
    assert_eq!(include_in_availability::NONE, "none");

    assert_eq!(event_status::CONFIRMED, "confirmed");
    assert_eq!(event_status::TENTATIVE, "tentative");
    assert_eq!(event_status::CANCELLED, "cancelled");

    assert_eq!(free_busy_status::FREE, "free");
    assert_eq!(free_busy_status::BUSY, "busy");

    assert_eq!(privacy::PUBLIC, "public");
    assert_eq!(privacy::PRIVATE, "private");
    assert_eq!(privacy::SECRET, "secret");

    assert_eq!(participant_role::OWNER, "owner");
    assert_eq!(participant_role::ADMIN, "admin");
    assert_eq!(participant_role::ATTENDEE, "attendee");
    assert_eq!(participant_role::OPTIONAL, "optional");
    assert_eq!(participant_role::INFORMATIONAL, "informational");

    assert_eq!(participation_status::NEEDS_ACTION, "needs-action");
    assert_eq!(participation_status::ACCEPTED, "accepted");
    assert_eq!(participation_status::DECLINED, "declined");
    assert_eq!(participation_status::TENTATIVE, "tentative");
    assert_eq!(participation_status::DELEGATED, "delegated");

    assert_eq!(participant_kind::INDIVIDUAL, "individual");
    assert_eq!(participant_kind::GROUP, "group");
    assert_eq!(participant_kind::RESOURCE, "resource");
    assert_eq!(participant_kind::LOCATION, "location");
}

#[test]
fn calendar_event_query_filter_properties_cover_draft_spec() {
    let filter: CalendarEventQueryFilter = serde_json::from_value(serde_json::json!({
        "inCalendar": "C1",
        "description": "Sprint planning",
        "location": "Room 101",
        "uid": "evt-1234"
    }))
    .unwrap();

    assert_eq!(filter.in_calendar.as_ref().unwrap().as_str(), "C1");
    assert_eq!(filter.description.as_deref(), Some("Sprint planning"));
    assert_eq!(filter.location.as_deref(), Some("Room 101"));
    assert_eq!(filter.uid.as_deref(), Some("evt-1234"));
}
