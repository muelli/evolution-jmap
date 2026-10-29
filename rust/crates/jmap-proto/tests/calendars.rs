// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the JMAP Calendars draft types (JSCalendar,
//! RFC 8984).

#![cfg(feature = "calendars")]

use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventQueryFilter, NDay, RecurrenceRule,
};
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
    // An override patch uses JSON format to represent modification or deletion.
    let overrides = event.recurrence_overrides.as_ref().unwrap();
    assert_eq!(
        overrides.get("2026-01-22T13:00:00"),
        Some(&serde_json::json!({"excluded": true}))
    );
    assert_eq!(
        overrides["2026-01-29T13:00:00"]["title"],
        serde_json::json!("Team sync (long)")
    );
    // Locations data stays in JSON format to preserve unmapped fields.
    let locations = event.locations.as_ref().expect("locations");
    assert_eq!(
        locations.get("loc1"),
        Some(&serde_json::json!({
            "@type": "Location",
            "name": "Room 42",
            "coordinates": "geo:52.520008,13.404954",
        }))
    );
    // `keywords` is an RFC 8984 §4.2.9 Set: keys are tags, values are `true`.
    // Held as JSON to tolerate non-boolean values.
    let keywords = event.keywords.as_ref().expect("keywords");
    assert_eq!(keywords.keys().collect::<Vec<_>>(), ["offsite", "planning"]);
    assert!(keywords.values().all(|set| set == &Value::Bool(true)));
    // Alerts data stays in JSON format to preserve unmapped fields.
    let alerts = event.alerts.as_ref().expect("alerts");
    assert_eq!(alerts.keys().collect::<Vec<_>>(), ["a1", "a2"]);
    assert_eq!(
        alerts["a1"]["trigger"],
        serde_json::json!({"@type": "OffsetTrigger", "offset": "-PT15M"})
    );
    assert_eq!(alerts["a2"]["acknowledged"], "2026-01-15T11:01:00Z");
    // Participants data stays in JSON format to preserve unmapped fields.
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
fn calendar_my_rights_and_share_with_roundtrip() {
    let value = fixture("calendars/calendar_with_rights.json");
    assert_eq!(roundtrip::<Calendar>(&value), value);

    let calendar: Calendar = serde_json::from_value(value).unwrap();
    let rights = calendar.my_rights.expect("myRights");
    assert_eq!(rights.may_write_all, Some(false));
    assert_eq!(rights.may_write_own, Some(true));
    assert_eq!(rights.may_rsvp, Some(true));
    // Neither field alone is writable. Together (own items, not all), the
    // calendar as a whole counts as writable.
    assert!(rights.is_writable());

    let share_with = calendar.share_with.expect("shareWith");
    let shared_rights = &share_with[&Id::new("P1")];
    assert_eq!(shared_rights.may_write_all, Some(true));
    assert!(shared_rights.is_writable());
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
    // so the mapping must see it and roundtrip it unchanged.
    let days = rule.by_day.as_deref().expect("byDay");
    assert_eq!(days, [NDay::new("we").with_nth_of_period(2)]);
    assert_eq!(days[0].day_type.as_deref(), Some("NDay"));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_days_of_the_month_roundtrip() {
    // `byMonthDay` models recurrence day-of-month intervals (RFC 8984 §4.3.3).
    let value = fixture("calendars/recurrence_rule_days_of_month.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "monthly");
    assert_eq!(rule.by_month_day.as_deref(), Some(&[15, -1][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_days_of_the_year_roundtrip() {
    // `byYearDay` corresponds to iCalendar's `BYYEARDAY`: positive counts from
    // start of year, negative counts back from December 31 (RFC 8984 §4.3.3).
    let value = fixture("calendars/recurrence_rule_days_of_year.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "yearly");
    assert_eq!(rule.by_year_day.as_deref(), Some(&[1, -1][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_weeks_of_the_year_roundtrip() {
    // `byWeekNo` corresponds to iCalendar's `BYWEEKNO`: positive counts from
    // first week, negative counts back from last week (RFC 8984 §4.3.3).
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
    // `byHour` corresponds to iCalendar's `BYHOUR` (RFC 8984 §4.3.3 / RFC 5545 §3.3.10).
    let value = fixture("calendars/recurrence_rule_hours.json");
    assert_eq!(roundtrip::<RecurrenceRule>(&value), value);

    let rule: RecurrenceRule = serde_json::from_value(value).unwrap();
    assert_eq!(rule.frequency, "daily");
    assert_eq!(rule.by_hour.as_deref(), Some(&[9, 17][..]));
    assert!(rule.extra.is_empty());
}

#[test]
fn recurrence_rule_minutes_and_seconds_roundtrip() {
    // `byMinute` and `bySecond` correspond to iCalendar's `BYMINUTE` and `BYSECOND`
    // (RFC 8984 §4.3.3 / RFC 5545 §3.3.10).
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
    // `bySetPosition` corresponds to iCalendar's `BYSETPOS` (RFC 8984 §4.3.3).
    // Filters occurrences produced by the other `by*` rules.
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
    // `byMonth` corresponds to iCalendar's `BYMONTH` (RFC 8984 §4.3.3).
    // Stored as strings to support non-Gregorian leap months (e.g. "5L").
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
    // `firstDayOfWeek` corresponds to iCalendar's `WKST` (RFC 8984 §4.3.3 / RFC 5545 §3.3.10).
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

#[test]
fn calendar_and_event_advanced_properties_cover_draft_spec() {
    let cal: Calendar = serde_json::from_value(serde_json::json!({
        "name": "Team Calendar",
        "timeZone": "Europe/Berlin",
        "myRights": {
            "mayReadItems": true,
            "mayWriteAll": true,
            "mayWriteOwn": true,
            "mayDelete": false
        }
    }))
    .unwrap();

    assert_eq!(cal.time_zone.as_deref(), Some("Europe/Berlin"));
    let rights = cal.my_rights.as_ref().unwrap();
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_write_all, Some(true));
    assert_eq!(rights.may_write_own, Some(true));
    assert_eq!(rights.may_delete, Some(false));

    let event: CalendarEvent = serde_json::from_value(serde_json::json!({
        "title": "Board Meeting",
        "useDefaultAlerts": true,
        "sequence": 3,
        "locale": "en-US",
        "replyTo": {
            "imip": "mailto:board@example.com"
        }
    }))
    .unwrap();

    assert_eq!(
        event.use_default_alerts.or_else(|| event
            .extra
            .get("useDefaultAlerts")
            .and_then(|v| v.as_bool())),
        Some(true)
    );
    assert_eq!(event.extra.get("sequence"), Some(&serde_json::json!(3)));
    assert_eq!(
        event
            .locale
            .as_deref()
            .or_else(|| event.extra.get("locale").and_then(|v| v.as_str())),
        Some("en-US")
    );
    assert_eq!(
        event.extra.get("replyTo"),
        Some(&serde_json::json!({"imip": "mailto:board@example.com"}))
    );
}

#[test]
fn jscalendar_constants_cover_alert_action_and_relative_to() {
    use jmap_proto::calendars::*;
    assert_eq!(alert_action::DISPLAY, "display");
    assert_eq!(alert_action::EMAIL, "email");

    assert_eq!(relative_to::START, "start");
    assert_eq!(relative_to::END, "end");
}

#[test]
fn jscalendar_participant_location_alert_and_parse_roundtrip() {
    use jmap_proto::calendars::{
        Alert, CalendarEvent, CalendarEventParseRequest, CalendarEventParseResponse,
        CalendarPreferences, Location, OffsetTrigger, Participant, VirtualLocation,
        participant_attendance, participant_progress, schedule_agent,
    };
    use jmap_proto::state::UtcDate;
    use std::collections::BTreeMap;

    assert_eq!(schedule_agent::SERVER, "server");
    assert_eq!(schedule_agent::CLIENT, "client");
    assert_eq!(schedule_agent::NONE, "none");

    assert_eq!(participant_progress::NEEDS_ACTION, "needs-action");
    assert_eq!(participant_progress::IN_PROCESS, "in-process");
    assert_eq!(participant_progress::COMPLETED, "completed");
    assert_eq!(participant_progress::FAILED, "failed");

    assert_eq!(participant_attendance::REQUIRED, "required");
    assert_eq!(participant_attendance::OPTIONAL, "optional");
    assert_eq!(participant_attendance::INFORMATIONAL, "informational");

    let participant = Participant::new("Alice Example", "alice@example.com")
        .with_description("Project Lead")
        .with_calendar_address("mailto:alice@example.com")
        .with_kind("individual")
        .with_roles(BTreeMap::from([("owner".to_owned(), true)]))
        .with_participation_status("accepted")
        .with_attendance(participant_attendance::REQUIRED.to_owned())
        .expect_reply(false)
        .with_progress(participant_progress::IN_PROCESS.to_owned());

    let p_val = serde_json::to_value(&participant).unwrap();
    assert_eq!(p_val["@type"], "Participant");
    assert_eq!(p_val["name"], "Alice Example");
    assert_eq!(p_val["email"], "alice@example.com");
    assert_eq!(p_val["roles"]["owner"], true);
    assert_eq!(p_val["attendance"], "required");
    assert_eq!(p_val["progress"], "in-process");
    assert_eq!(p_val["calendarAddress"], "mailto:alice@example.com");
    assert!(p_val.get("sendTo").is_none());

    let p_round_tripped: Participant = serde_json::from_value(p_val).unwrap();
    assert_eq!(p_round_tripped, participant);

    let location = Location::new("Conference Room A")
        .with_coordinates("geo:51.5074,-0.1278")
        .with_location_types(BTreeMap::from([("room".to_owned(), true)]));
    let loc_val = serde_json::to_value(&location).unwrap();
    assert_eq!(loc_val["@type"], "Location");
    assert_eq!(loc_val["name"], "Conference Room A");
    assert_eq!(
        serde_json::from_value::<Location>(loc_val).unwrap(),
        location
    );

    let vloc = VirtualLocation::new("https://meet.example.com/bridge")
        .with_name("Video Bridge")
        .with_features(BTreeMap::from([("video".to_owned(), true)]));
    let vloc_val = serde_json::to_value(&vloc).unwrap();
    assert_eq!(vloc_val["@type"], "VirtualLocation");
    assert_eq!(vloc_val["uri"], "https://meet.example.com/bridge");
    assert_eq!(
        serde_json::from_value::<VirtualLocation>(vloc_val).unwrap(),
        vloc
    );

    let alert = Alert::new(
        "display",
        serde_json::json!({
            "@type": "OffsetTrigger",
            "offset": "-PT15M",
            "relativeTo": "start"
        }),
    )
    .with_acknowledged(UtcDate::new("2026-09-01T09:00:00Z"))
    .with_related_to("start");
    let a_val = serde_json::to_value(&alert).unwrap();
    assert_eq!(a_val["@type"], "Alert");
    assert_eq!(a_val["action"], "display");
    assert_eq!(a_val["trigger"]["offset"], "-PT15M");
    assert_eq!(serde_json::from_value::<Alert>(a_val).unwrap(), alert);

    let trigger = OffsetTrigger::new("-PT10M").relative_to("start");
    let t_val = serde_json::to_value(&trigger).unwrap();
    assert_eq!(t_val["@type"], "OffsetTrigger");
    assert_eq!(t_val["offset"], "-PT10M");
    assert_eq!(
        serde_json::from_value::<OffsetTrigger>(t_val).unwrap(),
        trigger
    );

    let prefs = CalendarPreferences::new()
        .with_id("singleton")
        .with_time_zone("UTC")
        .with_first_day_of_week("mo");
    let prefs_val = serde_json::to_value(&prefs).unwrap();
    assert_eq!(prefs_val["timeZone"], "UTC");
    assert_eq!(prefs_val["firstDayOfWeek"], "mo");
    assert_eq!(
        serde_json::from_value::<CalendarPreferences>(prefs_val).unwrap(),
        prefs
    );

    let parse_req = CalendarEventParseRequest::new("A1", ["b1"])
        .properties(["id".to_owned(), "title".to_owned()]);
    let pr_val = serde_json::to_value(&parse_req).unwrap();
    assert_eq!(pr_val["accountId"], "A1");
    assert_eq!(pr_val["blobIds"], serde_json::json!(["b1"]));
    assert_eq!(
        serde_json::from_value::<CalendarEventParseRequest>(pr_val).unwrap(),
        parse_req
    );

    let parse_resp = CalendarEventParseResponse::new("A1").with_parsed(BTreeMap::from([(
        "b1".into(),
        CalendarEvent::new().with_id("E1").with_title("Standup"),
    )]));
    let pr_resp_val = serde_json::to_value(&parse_resp).unwrap();
    assert_eq!(pr_resp_val["parsed"]["b1"]["title"], "Standup");
    assert_eq!(
        serde_json::from_value::<CalendarEventParseResponse>(pr_resp_val).unwrap(),
        parse_resp
    );
}

/// CalendarsCapability, AbsoluteTrigger, and EventRelation cover draft-ietf-jmap-calendars-33 §1.5.1 and RFC 8984 §1.4.10, §4.5.2.
#[test]
fn calendars_capabilities_absolute_trigger_and_event_relation_roundtrip_covers_draft_jmap_calendars()
 {
    use jmap_proto::calendars::{
        AbsoluteTrigger, CalendarsCapability, EventRelation, event_relation_type, priority,
    };

    use std::collections::BTreeMap;

    assert_eq!(event_relation_type::FIRST, "first");
    assert_eq!(event_relation_type::NEXT, "next");
    assert_eq!(event_relation_type::PARENT, "parent");
    assert_eq!(event_relation_type::CHILD, "child");

    assert_eq!(priority::UNDEFINED, 0);
    assert_eq!(priority::HIGH, 1);
    assert_eq!(priority::MEDIUM, 5);
    assert_eq!(priority::LOW, 9);

    let cap = CalendarsCapability::new()
        .with_max_participants_per_event(25_000_000)
        .with_max_calendars_per_event(10);
    let cap_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(cap_val["maxParticipantsPerEvent"], 25_000_000);
    assert_eq!(cap_val["maxCalendarsPerEvent"], 10);

    let round_cap: CalendarsCapability = serde_json::from_value(cap_val).unwrap();
    assert_eq!(round_cap, cap);

    let abs_trigger = AbsoluteTrigger::new(jmap_proto::UtcDate::new("2026-09-01T08:30:00Z"));
    let at_val = serde_json::to_value(&abs_trigger).unwrap();
    assert_eq!(at_val["@type"], "AbsoluteTrigger");
    assert_eq!(at_val["when"], "2026-09-01T08:30:00Z");

    let relation = EventRelation::new(BTreeMap::from([(
        event_relation_type::PARENT.to_owned(),
        true,
    )]));
    let rel_val = serde_json::to_value(&relation).unwrap();
    assert_eq!(rel_val["@type"], "Relation");
    assert_eq!(rel_val["relation"]["parent"], true);

    let round_rel: EventRelation = serde_json::from_value(rel_val).unwrap();
    assert_eq!(round_rel, relation);
}

#[test]
fn calendar_sharing_timezone_and_recurrence_rule_extensions_roundtrip() {
    use jmap_proto::calendars::{
        Calendar, CalendarEventQueryFilter, CalendarRights, RecurrenceRule, frequency,
        recurrence_skip, weekday,
    };
    use std::collections::BTreeMap;

    assert_eq!(frequency::SECONDLY, "secondly");
    assert_eq!(frequency::MINUTELY, "minutely");
    assert_eq!(frequency::HOURLY, "hourly");
    assert_eq!(frequency::DAILY, "daily");
    assert_eq!(frequency::WEEKLY, "weekly");
    assert_eq!(frequency::MONTHLY, "monthly");
    assert_eq!(frequency::YEARLY, "yearly");

    assert_eq!(recurrence_skip::OMIT, "omit");
    assert_eq!(recurrence_skip::BACKWARD, "backward");
    assert_eq!(recurrence_skip::FORWARD, "forward");

    assert_eq!(weekday::MO, "mo");
    assert_eq!(weekday::TU, "tu");
    assert_eq!(weekday::WE, "we");
    assert_eq!(weekday::TH, "th");
    assert_eq!(weekday::FR, "fr");
    assert_eq!(weekday::SA, "sa");
    assert_eq!(weekday::SU, "su");

    let rights = CalendarRights::default()
        .with_may_read_items(true)
        .with_may_write_all(true)
        .with_may_write_own(true)
        .with_may_rsvp(true)
        .with_may_share(false)
        .with_may_delete(false);

    let cal = {
        let mut c = Calendar::default();
        c.id = Some("cal1".into());
        c.name = "Shared Team Calendar".to_owned();
        c.time_zone = Some("Europe/Berlin".to_owned());
        c.share_with = Some(BTreeMap::from([("usr_bob".into(), rights.clone())]));
        c.my_rights = Some(rights.clone());
        c
    };

    let c_val = serde_json::to_value(&cal).unwrap();
    assert_eq!(c_val["timeZone"], "Europe/Berlin");
    assert_eq!(c_val["shareWith"]["usr_bob"]["mayReadItems"], true);
    assert_eq!(c_val["myRights"]["mayWriteAll"], true);

    let round_cal: Calendar = serde_json::from_value(c_val).unwrap();
    assert_eq!(round_cal, cal);

    let rrule = {
        let mut __built = RecurrenceRule::default();
        __built.rule_type = Some("RecurrenceRule".to_owned());
        __built.frequency = frequency::MONTHLY.to_owned();
        __built.rscale = Some("hebrew".to_owned());
        __built.skip = Some(recurrence_skip::FORWARD.to_owned());
        __built
    };

    let r_val = serde_json::to_value(&rrule).unwrap();
    assert_eq!(r_val["frequency"], "monthly");
    assert_eq!(r_val["rscale"], "hebrew");
    assert_eq!(r_val["skip"], "forward");

    let round_rrule: RecurrenceRule = serde_json::from_value(r_val).unwrap();
    assert_eq!(round_rrule, rrule);

    let filter = CalendarEventQueryFilter::in_calendar("cal1")
        .with_time_range("2026-09-01T00:00:00Z", "2026-09-30T23:59:59Z")
        .title("Planning")
        .description("Sprint planning")
        .location("Room 101")
        .uid("evt-uid-456")
        .text("quarterly");

    assert_eq!(filter.in_calendar.as_ref().unwrap().as_str(), "cal1");
    assert_eq!(filter.after.as_deref(), Some("2026-09-01T00:00:00Z"));
    assert_eq!(filter.before.as_deref(), Some("2026-09-30T23:59:59Z"));
    assert_eq!(filter.title.as_deref(), Some("Planning"));
    assert_eq!(filter.description.as_deref(), Some("Sprint planning"));
    assert_eq!(filter.location.as_deref(), Some("Room 101"));
    assert_eq!(filter.uid.as_deref(), Some("evt-uid-456"));
    assert_eq!(filter.text.as_deref(), Some("quarterly"));
}

#[test]
fn calendar_and_event_spec_properties_roundtrip() {
    use jmap_proto::calendars::{Calendar, CalendarEvent};

    let cal = {
        let mut c = Calendar::default();
        c.id = Some("cal_spec".into());
        c.name = "Personal Schedule".to_owned();
        c.may_delete = Some(true);
        c.color = Some("#336699".to_owned());
        c
    };
    let c_val = serde_json::to_value(&cal).unwrap();
    assert_eq!(c_val["mayDelete"], true);
    assert_eq!(c_val["color"], "#336699");
    assert_eq!(serde_json::from_value::<Calendar>(c_val).unwrap(), cal);

    let event = CalendarEvent::new()
        .with_id("evt_spec")
        .with_title("International Conference")
        .use_default_alerts(true)
        .with_color("#ff5500")
        .with_locale("en-GB");
    let e_val = serde_json::to_value(&event).unwrap();
    assert_eq!(e_val["useDefaultAlerts"], true);
    assert_eq!(e_val["color"], "#ff5500");
    assert_eq!(e_val["locale"], "en-GB");

    let round_event: CalendarEvent = serde_json::from_value(e_val).unwrap();
    assert_eq!(round_event, event);

    // Deserialization of legacy JSON with localizations is still supported:
    let legacy_json = serde_json::json!({
        "id": "evt_spec_legacy",
        "title": "International Conference",
        "localizations": {
            "de": {"title": "Internationale Konferenz"}
        }
    });
    let legacy_ev: CalendarEvent = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(
        legacy_ev.localizations.as_ref().unwrap()["de"]["title"],
        "Internationale Konferenz"
    );
}

#[test]
fn calendar_event_get_free_busy_and_set_error_roundtrip_covers_draft_spec() {
    use jmap_proto::UtcDate;
    use jmap_proto::calendars::{
        FreeBusyBlock, GetFreeBusyRequest, GetFreeBusyResponse, calendar_event_set_error,
        calendar_free_busy_status,
    };

    assert_eq!(calendar_event_set_error::BLOB_NOT_FOUND, "blobNotFound");
    assert_eq!(
        calendar_event_set_error::TOO_MANY_PARTICIPANTS,
        "tooManyParticipants"
    );
    assert_eq!(
        calendar_event_set_error::TOO_MANY_RECURRENCES,
        "tooManyRecurrences"
    );

    assert_eq!(calendar_free_busy_status::FREE, "free");
    assert_eq!(calendar_free_busy_status::BUSY, "busy");
    assert_eq!(calendar_free_busy_status::BUSY_TENTATIVE, "busy-tentative");
    assert_eq!(
        calendar_free_busy_status::BUSY_UNAVAILABLE,
        "busy-unavailable"
    );

    let req = GetFreeBusyRequest::new(
        "A1",
        UtcDate::new("2026-09-01T00:00:00Z"),
        UtcDate::new("2026-09-02T00:00:00Z"),
    )
    .calendar_ids(["cal1", "cal2"])
    .time_zone("Europe/London");

    let req_json = serde_json::to_value(&req).unwrap();
    assert_eq!(req_json["accountId"], "A1");
    assert_eq!(req_json["utcStart"], "2026-09-01T00:00:00Z");
    assert_eq!(req_json["utcEnd"], "2026-09-02T00:00:00Z");
    assert_eq!(req_json["calendarIds"], serde_json::json!(["cal1", "cal2"]));
    assert_eq!(req_json["timeZone"], "Europe/London");
    assert_eq!(
        serde_json::from_value::<GetFreeBusyRequest>(req_json).unwrap(),
        req
    );

    let resp = GetFreeBusyResponse::new(
        "A1",
        [
            FreeBusyBlock::new(
                UtcDate::new("2026-09-01T09:00:00Z"),
                UtcDate::new("2026-09-01T10:00:00Z"),
                "busy",
            )
            .with_calendar_id("cal1")
            .with_event_id("evt1"),
            FreeBusyBlock::new(
                UtcDate::new("2026-09-01T14:00:00Z"),
                UtcDate::new("2026-09-01T15:00:00Z"),
                "busy-tentative",
            ),
        ],
    );

    let resp_json = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_json["accountId"], "A1");
    assert_eq!(resp_json["list"][0]["busyStatus"], "busy");
    assert_eq!(resp_json["list"][0]["calendarId"], "cal1");
    assert_eq!(resp_json["list"][0]["eventId"], "evt1");
    assert_eq!(resp_json["list"][1]["busyStatus"], "busy-tentative");
    assert_eq!(
        serde_json::from_value::<GetFreeBusyResponse>(resp_json).unwrap(),
        resp
    );
}

#[test]
fn calendar_and_preferences_builders_roundtrip() {
    use jmap_proto::calendars::{Calendar, CalendarPreferences};

    let cal = Calendar::new("Team Events")
        .with_description("Shared team planning and milestones")
        .with_color("#336699")
        .with_sort_order(2)
        .is_default(true)
        .is_subscribed(true)
        .is_visible(true)
        .with_time_zone("Europe/Berlin");

    assert_eq!(cal.name, "Team Events");
    assert_eq!(
        cal.description.as_deref(),
        Some("Shared team planning and milestones")
    );
    assert_eq!(cal.color.as_deref(), Some("#336699"));
    assert_eq!(cal.sort_order, Some(2));
    assert_eq!(cal.is_default, Some(true));
    assert_eq!(cal.is_subscribed, Some(true));
    assert_eq!(cal.is_visible, Some(true));
    assert_eq!(cal.time_zone.as_deref(), Some("Europe/Berlin"));

    let cal_val = serde_json::to_value(&cal).unwrap();
    assert_eq!(cal_val["name"], "Team Events");
    assert_eq!(
        cal_val["description"],
        "Shared team planning and milestones"
    );
    assert_eq!(cal_val["color"], "#336699");
    assert_eq!(cal_val["sortOrder"], 2);
    assert_eq!(cal_val["isDefault"], true);
    assert_eq!(cal_val["isSubscribed"], true);
    assert_eq!(cal_val["isVisible"], true);
    assert_eq!(cal_val["timeZone"], "Europe/Berlin");
    assert_eq!(serde_json::from_value::<Calendar>(cal_val).unwrap(), cal);

    let prefs = CalendarPreferences::new()
        .with_id("singleton")
        .with_time_zone("America/New_York")
        .with_first_day_of_week("su");

    assert_eq!(prefs.id.as_ref().unwrap().as_str(), "singleton");
    assert_eq!(prefs.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(prefs.first_day_of_week.as_deref(), Some("su"));

    let prefs_val = serde_json::to_value(&prefs).unwrap();
    assert_eq!(prefs_val["id"], "singleton");
    assert_eq!(prefs_val["timeZone"], "America/New_York");
    assert_eq!(prefs_val["firstDayOfWeek"], "su");
    assert_eq!(
        serde_json::from_value::<CalendarPreferences>(prefs_val).unwrap(),
        prefs
    );
}

#[test]
fn calendar_event_parse_response_free_busy_response_and_entity_builders() {
    use jmap_proto::UtcDate;
    use jmap_proto::calendars::{
        AbsoluteTrigger, Alert, CalendarEvent, CalendarEventParseResponse, CalendarRights,
        EventRelation, FreeBusyBlock, GetFreeBusyResponse, Location, NDay, OffsetTrigger,
        Participant, RecurrenceRule, VirtualLocation,
    };
    use std::collections::BTreeMap;

    let parse_resp = CalendarEventParseResponse::new("acc1")
        .with_parsed(BTreeMap::from([("b1".into(), CalendarEvent::default())]))
        .with_not_parsable(["b2"])
        .with_not_found(["b3"]);

    assert_eq!(parse_resp.account_id.as_str(), "acc1");
    assert_eq!(parse_resp.parsed.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_parsable.as_ref().unwrap().len(), 1);
    assert_eq!(parse_resp.not_found.as_ref().unwrap().len(), 1);

    let fb_resp = GetFreeBusyResponse::new(
        "acc1",
        [FreeBusyBlock::new(
            UtcDate::new("2026-09-01T10:00:00Z"),
            UtcDate::new("2026-09-01T11:00:00Z"),
            "busy",
        )
        .with_calendar_id("cal1")
        .with_event_id("evt1")],
    );
    assert_eq!(fb_resp.account_id.as_str(), "acc1");
    assert_eq!(fb_resp.list.len(), 1);

    let rights_all = CalendarRights::all();
    assert_eq!(rights_all.may_read_items, Some(true));
    assert_eq!(rights_all.may_write_all, Some(true));
    assert_eq!(rights_all.may_write_own, Some(true));
    assert_eq!(rights_all.may_delete, Some(true));
    assert!(rights_all.is_writable());

    let rights_ro = CalendarRights::read_only();
    assert_eq!(rights_ro.may_read_items, Some(true));
    assert_eq!(rights_ro.may_write_all, None);
    assert!(!rights_ro.is_writable());

    let loc = Location::new("Meeting Room A")
        .with_description("Room 101")
        .with_time_zone("Europe/London")
        .with_coordinates("geo:51.5074,-0.1278");
    assert_eq!(loc.name.as_deref(), Some("Meeting Room A"));
    assert_eq!(loc.description.as_deref(), Some("Room 101"));
    assert_eq!(loc.time_zone.as_deref(), Some("Europe/London"));

    let vloc = VirtualLocation::new("https://meet.example.com/room")
        .with_name("Main Bridge")
        .with_description("Jitsi meeting")
        .with_features(BTreeMap::from([("video".to_string(), true)]));
    assert_eq!(vloc.uri, "https://meet.example.com/room");
    assert_eq!(vloc.name.as_deref(), Some("Main Bridge"));

    let alert = Alert::new(
        "display",
        serde_json::json!({"@type": "OffsetTrigger", "offset": "-PT15M"}),
    )
    .with_acknowledged(UtcDate::new("2026-09-01T09:45:00Z"))
    .with_related_to("start");
    assert_eq!(alert.action.as_deref(), Some("display"));
    assert_eq!(alert.related_to.as_deref(), Some("start"));

    let offset_trig = OffsetTrigger::new("-PT30M").relative_to("start");
    assert_eq!(offset_trig.offset, "-PT30M");
    assert_eq!(offset_trig.relative_to.as_deref(), Some("start"));

    let abs_trig = AbsoluteTrigger::new(UtcDate::new("2026-09-01T09:00:00Z"));
    assert_eq!(abs_trig.when.as_str(), "2026-09-01T09:00:00Z");

    let relation = EventRelation::new(BTreeMap::from([("parent".to_string(), true)]));
    assert!(relation.relation.as_ref().unwrap().contains_key("parent"));

    let part = Participant::new("Bob Smith", "bob@example.com")
        .with_roles(BTreeMap::from([("attendee".to_string(), true)]))
        .with_participation_status("accepted")
        .with_attendance("required")
        .expect_reply(true)
        .with_schedule_agent("server");
    assert_eq!(part.name.as_deref(), Some("Bob Smith"));
    assert_eq!(part.email.as_deref(), Some("bob@example.com"));
    assert_eq!(part.participation_status.as_deref(), Some("accepted"));

    let rrule = RecurrenceRule::new("weekly")
        .with_interval(2)
        .with_count(10)
        .with_until("2026-12-31T23:59:59Z")
        .with_by_day([NDay::new("mo").with_nth_of_period(1), NDay::new("fr")])
        .with_by_month(["1", "6"])
        .with_rscale("gregorian")
        .with_skip("omit");
    assert_eq!(rrule.frequency, "weekly");
    assert_eq!(rrule.interval, Some(2));
    assert_eq!(rrule.count, Some(10));
    assert_eq!(rrule.by_day.as_ref().unwrap().len(), 2);
    assert_eq!(rrule.rscale.as_deref(), Some("gregorian"));
    assert_eq!(rrule.skip.as_deref(), Some("omit"));
}

#[test]
fn calendar_event_and_capability_builders() {
    use jmap_proto::Id;
    use jmap_proto::calendars::{CalendarEvent, CalendarsCapability, RecurrenceRule};

    let cap = CalendarsCapability::new()
        .with_max_participants_per_event(50_000_000)
        .with_max_calendars_per_event(10);
    assert_eq!(cap.max_participants_per_event, Some(50_000_000));
    assert_eq!(cap.max_calendars_per_event, Some(10));

    let event = CalendarEvent::default()
        .with_id("evt_100")
        .with_calendar_id("cal_work")
        .with_uid("urn:uuid:event-9876")
        .with_title("Team Planning")
        .with_description("Quarterly planning session")
        .with_start("2026-09-01T09:00:00")
        .with_time_zone("Europe/Berlin")
        .with_duration("PT2H")
        .with_status("confirmed")
        .with_free_busy_status("busy")
        .with_priority(1)
        .with_privacy("private")
        .show_without_time(false)
        .use_default_alerts(true)
        .with_color("#336699")
        .with_locale("en")
        .with_recurrence_rule(RecurrenceRule::new("weekly"));

    assert_eq!(event.id.as_ref().unwrap().as_str(), "evt_100");
    assert!(
        event
            .calendar_ids
            .as_ref()
            .unwrap()
            .contains_key(&Id::new("cal_work"))
    );
    assert_eq!(event.uid.as_deref(), Some("urn:uuid:event-9876"));
    assert_eq!(event.title.as_deref(), Some("Team Planning"));
    assert_eq!(
        event.description.as_deref(),
        Some("Quarterly planning session")
    );
    assert_eq!(event.start.as_deref(), Some("2026-09-01T09:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.duration.as_deref(), Some("PT2H"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(1));
    assert_eq!(event.privacy.as_deref(), Some("private"));
    assert_eq!(event.show_without_time, Some(false));
    assert_eq!(event.use_default_alerts, Some(true));
    assert_eq!(event.color.as_deref(), Some("#336699"));
    assert_eq!(event.locale.as_deref(), Some("en"));
    assert!(event.recurrence_rule.is_some());
}

#[test]
fn calendar_group_roundtrip_and_builders() {
    use jmap_proto::calendars::CalendarGroup;
    use jmap_proto::{Id, UtcDate};
    use std::collections::BTreeMap;

    let group = CalendarGroup::new("Project Milestones")
        .with_id(Id::new("grp_1"))
        .with_uid("urn:uuid:group-1234")
        .with_description("Collection of sprint milestone events")
        .with_time_zone("America/New_York")
        .with_updated(UtcDate::new("2026-09-01T12:00:00Z"))
        .with_entries(BTreeMap::from([(
            "evt_1".to_string(),
            serde_json::json!({"@type": "Event", "title": "Milestone 1"}),
        )]))
        .with_keywords(BTreeMap::from([(
            "sprint".to_string(),
            serde_json::Value::Bool(true),
        )]))
        .with_categories(BTreeMap::from([("engineering".to_string(), true)]))
        .with_color("#228833")
        .with_source("https://calendar.example.com/groups/grp_1")
        .with_links(BTreeMap::from([(
            "l1".to_string(),
            serde_json::json!({"href": "https://wiki.example.com/sprint"}),
        )]));

    let json = serde_json::to_value(&group).unwrap();
    assert_eq!(json["@type"], "Group");
    assert_eq!(json["id"], "grp_1");
    assert_eq!(json["uid"], "urn:uuid:group-1234");
    assert_eq!(json["title"], "Project Milestones");
    assert_eq!(json["description"], "Collection of sprint milestone events");
    assert_eq!(json["timeZone"], "America/New_York");
    assert_eq!(json["updated"], "2026-09-01T12:00:00Z");
    assert_eq!(json["entries"]["evt_1"]["title"], "Milestone 1");
    assert_eq!(json["keywords"]["sprint"], true);
    assert_eq!(json["categories"]["engineering"], true);
    assert_eq!(json["color"], "#228833");
    assert_eq!(json["source"], "https://calendar.example.com/groups/grp_1");
    assert_eq!(
        json["links"]["l1"]["href"],
        "https://wiki.example.com/sprint"
    );

    let deserialized: CalendarGroup = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, group);
}

#[test]
fn participant_participation_comment_roundtrip() {
    use jmap_proto::calendars::Participant;

    let p = Participant::new("Alice", "alice@example.com")
        .with_participation_comment("Will arrive 10 minutes late");

    // In JSCalendar-bis, participationComment is reserved and omitted on serialization:
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["name"], "Alice");
    assert_eq!(json["email"], "alice@example.com");
    assert!(json.get("participationComment").is_none());

    // Legacy JSON with participationComment is read on deserialization:
    let legacy_json = serde_json::json!({
        "@type": "Participant",
        "name": "Alice",
        "email": "alice@example.com",
        "participationComment": "Will arrive 10 minutes late"
    });
    let deserialized: Participant = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(
        deserialized.participation_comment.as_deref(),
        Some("Will arrive 10 minutes late")
    );
}

#[test]
fn calendar_preferences_capability_accessor() {
    use jmap_proto::State;
    use jmap_proto::calendars::CalendarPreferencesCapability;
    use jmap_proto::session::{CAPABILITY_CALENDAR_PREFERENCES, Session};

    let mut session = Session::new(
        "alice@example.com",
        "https://jmap.example.com/api/",
        "https://jmap.example.com/download/",
        "https://jmap.example.com/upload/",
        State::new("s1"),
    );

    assert!(session.calendar_preferences_capability().is_none());

    session = session.with_capability(
        CAPABILITY_CALENDAR_PREFERENCES,
        serde_json::json!({"customPrefExtension": true}),
    );

    let cap = session
        .calendar_preferences_capability()
        .expect("capability present");
    assert_eq!(
        cap,
        CalendarPreferencesCapability::default()
            .with_extra(serde_json::json!({"customPrefExtension": true}))
    );
}

#[test]
fn calendar_event_notification_roundtrip_and_builders() {
    use jmap_proto::calendars::{
        CalendarEvent, CalendarEventNotification, CalendarEventNotificationQueryFilter,
        calendar_event_notification_type,
    };
    use jmap_proto::principals::Principal;
    use jmap_proto::state::UtcDate;

    assert_eq!(calendar_event_notification_type::CREATED, "created");
    assert_eq!(calendar_event_notification_type::UPDATED, "updated");
    assert_eq!(calendar_event_notification_type::DESTROYED, "destroyed");
    assert_eq!(calendar_event_notification_type::REPLY, "reply");

    let notif =
        CalendarEventNotification::new(UtcDate::new("2026-09-01T15:00:00Z"), Id::new("evt_123"))
            .with_id(Id::new("notif_1"))
            .with_changed_by(Principal::new("Bob Builder").with_email("bob@example.com"))
            .with_comment("Moved meeting by 30 mins")
            .with_notification_type(calendar_event_notification_type::UPDATED)
            .with_recurrence_id("2026-09-02T10:00:00")
            .with_event(CalendarEvent::new().with_title("Sprint Retro"));

    let json = serde_json::to_value(&notif).unwrap();
    assert_eq!(json["@type"], "CalendarEventNotification");
    assert_eq!(json["id"], "notif_1");
    assert_eq!(json["created"], "2026-09-01T15:00:00Z");
    assert_eq!(json["eventId"], "evt_123");
    assert_eq!(json["comment"], "Moved meeting by 30 mins");
    assert_eq!(json["type"], "updated");
    assert_eq!(json["recurrenceId"], "2026-09-02T10:00:00");
    assert_eq!(json["event"]["title"], "Sprint Retro");
    assert_eq!(json["changedBy"]["name"], "Bob Builder");

    let deserialized: CalendarEventNotification = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, notif);

    let filter = CalendarEventNotificationQueryFilter::new()
        .with_after(UtcDate::new("2026-09-01T00:00:00Z"))
        .with_before(UtcDate::new("2026-09-02T00:00:00Z"))
        .with_types(vec!["created".to_owned(), "updated".to_owned()]);

    let filter_json = serde_json::to_value(&filter).unwrap();
    assert_eq!(filter_json["after"], "2026-09-01T00:00:00Z");
    assert_eq!(filter_json["before"], "2026-09-02T00:00:00Z");
    assert_eq!(filter_json["types"][0], "created");
    assert_eq!(filter_json["types"][1], "updated");

    let filter_deserialized: CalendarEventNotificationQueryFilter =
        serde_json::from_value(filter_json).unwrap();
    assert_eq!(filter_deserialized, filter);
}

#[test]
fn calendar_event_send_request_and_response_roundtrip() {
    use jmap_proto::calendars::{
        CalendarEvent, CalendarEventSendRequest, CalendarEventSendResponse, ParticipantProblem,
        SendCalendarEvent, SendCalendarEventResult, calendar_send_error, participant_problem_kind,
    };
    use jmap_proto::error::SetError;
    use std::collections::BTreeMap;

    assert_eq!(
        participant_problem_kind::CANNOT_SEND_TO_SELF,
        "cannotSendToSelf"
    );
    assert_eq!(
        participant_problem_kind::CALENDAR_NOT_FOUND,
        "calendarNotFound"
    );
    assert_eq!(
        participant_problem_kind::PARTICIPANT_NOT_FOUND,
        "participantNotFound"
    );
    assert_eq!(participant_problem_kind::INVALID_EMAIL, "invalidEmail");
    assert_eq!(
        participant_problem_kind::CANNOT_SEND_TO_RESOURCE,
        "cannotSendToResource"
    );
    assert_eq!(participant_problem_kind::NOT_AUTHORIZED, "notAuthorized");

    assert_eq!(calendar_send_error::FORBIDDEN_FROM, "forbiddenFrom");
    assert_eq!(
        calendar_send_error::PARTICIPANT_NOT_FOUND,
        "participantNotFound"
    );
    assert_eq!(
        calendar_send_error::INVALID_PARTICIPANTS,
        "invalidParticipants"
    );
    assert_eq!(
        calendar_send_error::CANNOT_SEND_FOR_CALENDAR,
        "cannotSendForCalendar"
    );
    assert_eq!(calendar_send_error::EVENT_NOT_FOUND, "eventNotFound");

    let req = CalendarEventSendRequest::new(Id::new("acc_1"))
        .with_identity_id(Id::new("ident_1"))
        .with_send(BTreeMap::from([(
            Id::new("k1"),
            SendCalendarEvent::new()
                .with_recipient("mailto:alice@example.com")
                .with_send_to(BTreeMap::from([(
                    "imip".to_owned(),
                    "mailto:alice@example.com".to_owned(),
                )]))
                .with_calendar_event(CalendarEvent::new().with_title("Planning"))
                .with_include_old_properties(true),
        )]))
        .with_on_success_update_calendar_event(BTreeMap::from([(
            Id::new("evt_1"),
            serde_json::json!({"status": "confirmed"}),
        )]))
        .with_on_success_destroy_calendar_event_ids(vec![Id::new("evt_old")]);

    let req_json = serde_json::to_value(&req).unwrap();
    assert_eq!(req_json["accountId"], "acc_1");
    assert_eq!(req_json["identityId"], "ident_1");
    assert_eq!(
        req_json["send"]["k1"]["recipient"],
        "mailto:alice@example.com"
    );
    assert_eq!(
        req_json["send"]["k1"]["sendTo"]["imip"],
        "mailto:alice@example.com"
    );
    assert_eq!(req_json["send"]["k1"]["calendarEvent"]["title"], "Planning");
    assert_eq!(req_json["send"]["k1"]["includeOldProperties"], true);
    assert_eq!(
        req_json["onSuccessUpdateCalendarEvent"]["evt_1"]["status"],
        "confirmed"
    );
    assert_eq!(req_json["onSuccessDestroyCalendarEventIds"][0], "evt_old");

    let req_deserialized: CalendarEventSendRequest = serde_json::from_value(req_json).unwrap();
    assert_eq!(req_deserialized, req);

    let prob =
        ParticipantProblem::new("invalidEmail").with_description("The domain does not exist");
    let prob_json = serde_json::to_value(&prob).unwrap();
    assert_eq!(prob_json["type"], "invalidEmail");
    assert_eq!(prob_json["description"], "The domain does not exist");

    let resp = CalendarEventSendResponse::new(Id::new("acc_1"))
        .with_sent(BTreeMap::from([(
            Id::new("k1"),
            SendCalendarEventResult::new()
                .with_send_status("sent")
                .with_participant_problems(BTreeMap::from([(
                    "mailto:bad@example.com".to_owned(),
                    prob,
                )])),
        )]))
        .with_not_sent(BTreeMap::from([(
            Id::new("k2"),
            SetError::new("forbiddenFrom").with_description("Cannot send on behalf of this user"),
        )]));

    let resp_json = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_json["accountId"], "acc_1");
    assert_eq!(resp_json["sent"]["k1"]["sendStatus"], "sent");
    assert_eq!(
        resp_json["sent"]["k1"]["participantProblems"]["mailto:bad@example.com"]["type"],
        "invalidEmail"
    );
    assert_eq!(resp_json["notSent"]["k2"]["type"], "forbiddenFrom");

    let resp_deserialized: CalendarEventSendResponse = serde_json::from_value(resp_json).unwrap();
    assert_eq!(resp_deserialized, resp);
}

#[test]
fn participant_reply_roundtrip_and_builders() {
    use jmap_proto::calendars::{ParticipantReply, participant_participation_status};
    use std::collections::BTreeMap;

    assert_eq!(
        participant_participation_status::NEEDS_ACTION,
        "needs-action"
    );
    assert_eq!(participant_participation_status::ACCEPTED, "accepted");
    assert_eq!(participant_participation_status::DECLINED, "declined");
    assert_eq!(participant_participation_status::TENTATIVE, "tentative");
    assert_eq!(participant_participation_status::DELEGATED, "delegated");

    let reply = ParticipantReply::new(
        Id::new("evt_100"),
        participant_participation_status::ACCEPTED,
    )
    .with_recurrence_id("2026-09-05T10:00:00")
    .with_comment("I will attend remotely")
    .with_send_to(BTreeMap::from([(
        "imip".to_owned(),
        "mailto:organizer@example.com".to_owned(),
    )]));

    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["calendarEventId"], "evt_100");
    assert_eq!(json["participationStatus"], "accepted");
    assert_eq!(json["recurrenceId"], "2026-09-05T10:00:00");
    assert_eq!(json["comment"], "I will attend remotely");
    assert_eq!(json["sendTo"]["imip"], "mailto:organizer@example.com");

    let deserialized: ParticipantReply = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, reply);
}

#[test]
fn participant_identity_roundtrip_and_builders() {
    use jmap_proto::calendars::{ParticipantIdentity, participant_identity_set_error};

    assert_eq!(
        participant_identity_set_error::CANNOT_DESTROY_DEFAULT,
        "cannotDestroyDefault"
    );

    let identity = ParticipantIdentity::new("Alice Work")
        .with_id("pi_work")
        .with_schedule_id("mailto:alice.work@example.com")
        .with_send_to_method("imip", "mailto:alice.work@example.com")
        .with_send_to_method("web", "https://example.com/schedule/alice")
        .is_default(true);

    assert_eq!(identity.name, "Alice Work");
    assert_eq!(identity.id.as_ref().unwrap().as_str(), "pi_work");
    assert_eq!(
        identity.schedule_id.as_deref(),
        Some("mailto:alice.work@example.com")
    );
    assert_eq!(identity.is_default, Some(true));
    let send_to = identity.send_to.as_ref().unwrap();
    assert_eq!(send_to["imip"], "mailto:alice.work@example.com");
    assert_eq!(send_to["web"], "https://example.com/schedule/alice");

    let json = serde_json::to_value(&identity).unwrap();
    assert_eq!(json["name"], "Alice Work");
    assert_eq!(json["id"], "pi_work");
    assert_eq!(json["scheduleId"], "mailto:alice.work@example.com");
    assert_eq!(json["isDefault"], true);
    assert_eq!(json["sendTo"]["imip"], "mailto:alice.work@example.com");

    let deserialized: ParticipantIdentity = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, identity);
}

#[test]
fn participant_deserializes_rfc8984_send_to_into_calendar_address() {
    use jmap_proto::calendars::Participant;
    use serde_json::json;

    let wire_rfc8984 = json!({
        "@type": "Participant",
        "name": "Bob Smith",
        "email": "bob@example.com",
        "sendTo": {
            "imip": "mailto:bob@example.com"
        }
    });

    let participant: Participant = serde_json::from_value(wire_rfc8984).unwrap();
    assert_eq!(
        participant.calendar_address.as_deref(),
        Some("mailto:bob@example.com")
    );
    assert_eq!(
        participant
            .send_to
            .as_ref()
            .and_then(|st| st.get("imip"))
            .map(String::as_str),
        Some("mailto:bob@example.com")
    );
}

#[test]
fn participant_deserializes_jscalendar_bis_calendar_address() {
    use jmap_proto::calendars::Participant;
    use serde_json::json;

    let wire_bis = json!({
        "@type": "Participant",
        "name": "Alice Vance",
        "email": "alice@example.com",
        "calendarAddress": "mailto:alice@example.com"
    });

    let participant: Participant = serde_json::from_value(wire_bis).unwrap();
    assert_eq!(
        participant.calendar_address.as_deref(),
        Some("mailto:alice@example.com")
    );
}

#[test]
fn participant_serialization_emits_calendar_address_and_never_send_to() {
    use jmap_proto::calendars::Participant;

    let p = Participant::new("Alice", "alice@example.com")
        .with_calendar_address("mailto:alice@example.com");
    let val = serde_json::to_value(&p).unwrap();
    assert_eq!(val["calendarAddress"], "mailto:alice@example.com");
    assert!(val.get("sendTo").is_none());

    let mut p_legacy = Participant::new("Bob", "bob@example.com");
    p_legacy.send_to = Some([("imip".to_owned(), "mailto:bob@example.com".to_owned())].into());
    let val_legacy = serde_json::to_value(&p_legacy).unwrap();
    assert_eq!(val_legacy["calendarAddress"], "mailto:bob@example.com");
    assert!(val_legacy.get("sendTo").is_none());
}

#[test]
fn participant_role_required_constant_exists() {
    use jmap_proto::calendars::participant_role;
    assert_eq!(participant_role::REQUIRED, "required");
    assert_eq!(participant_role::ATTENDEE, "attendee");
}

#[test]
fn calendar_event_top_level_bis_fields_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::calendars::CalendarEvent;

    let event = CalendarEvent::new()
        .with_end_time_zone("Europe/Berlin")
        .with_main_location_id("loc-1")
        .with_schedule_sequence(3)
        .with_schedule_updated(UtcDate::new("2026-10-14T03:00:00Z"))
        .may_invite_self(true)
        .may_invite_others(false);

    assert_eq!(event.end_time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(event.main_location_id.as_ref().unwrap().as_str(), "loc-1");
    assert_eq!(event.schedule_sequence, Some(3));
    assert_eq!(
        event.schedule_updated.as_ref().unwrap().as_str(),
        "2026-10-14T03:00:00Z"
    );
    assert_eq!(event.may_invite_self, Some(true));
    assert_eq!(event.may_invite_others, Some(false));

    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["endTimeZone"], "Europe/Berlin");
    assert_eq!(val["mainLocationId"], "loc-1");
    assert_eq!(val["scheduleSequence"], 3);
    assert_eq!(val["scheduleUpdated"], "2026-10-14T03:00:00Z");
    assert_eq!(val["mayInviteSelf"], true);
    assert_eq!(val["mayInviteOthers"], false);

    let round_tripped: CalendarEvent = serde_json::from_value(val).unwrap();
    assert_eq!(round_tripped, event);
}

/// The calendars capability object of draft-ietf-jmap-calendars-33 §1.5.1 parses
/// with all six fields, and an object with no fields parses too.
#[test]
fn calendars_capability_parses_draft_33_fields() {
    use jmap_proto::calendars::CalendarsCapability;

    let cap: CalendarsCapability = serde_json::from_value(serde_json::json!({
        "maxCalendarsPerEvent": 1,
        "minDateTime": "1970-01-01T00:00:00",
        "maxDateTime": "2100-01-01T00:00:00",
        "maxExpandedQueryDuration": "P366D",
        "maxParticipantsPerEvent": 50,
        "mayCreateCalendar": true
    }))
    .unwrap();
    assert_eq!(cap.max_calendars_per_event, Some(1));
    assert_eq!(cap.min_date_time.as_deref(), Some("1970-01-01T00:00:00"));
    assert_eq!(cap.max_date_time.as_deref(), Some("2100-01-01T00:00:00"));
    assert_eq!(cap.max_expanded_query_duration.as_deref(), Some("P366D"));
    assert_eq!(cap.max_participants_per_event, Some(50));
    assert_eq!(cap.may_create_calendar, Some(true));
    assert!(cap.extra.is_empty());

    let back = serde_json::to_value(&cap).unwrap();
    assert_eq!(back["maxExpandedQueryDuration"], "P366D");
    assert_eq!(back["mayCreateCalendar"], true);

    let empty: CalendarsCapability = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, CalendarsCapability::new());
    assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));

    // A null maxCalendarsPerEvent means "no limit" and must parse.
    let null_limit: CalendarsCapability =
        serde_json::from_value(serde_json::json!({"maxCalendarsPerEvent": null})).unwrap();
    assert_eq!(null_limit.max_calendars_per_event, None);
}
