// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write side. The theme throughout is that saving a component must not
//! destroy what the component could not carry: the mapping keeps fourteen
//! properties of a JSCalendar event and drops the rest, so a save that
//! replaced properties wholesale would delete data the user never touched and
//! cannot even see.

mod common;

use common::Fixture;
use jmap_proto::calendars::NDay;
use serde_json::json;

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
fn editing_an_event_leaves_unmapped_properties_alone() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    // Properties no component we produce can carry. (`locations` was the
    // exemplar here until the place an event happens at became mapped,
    // `keywords` until the tags did, `freeBusyStatus` until the transparency
    // did, `priority` until the importance did and `useDefaultAlerts` until the
    // reminders did; the guest list and the scheduling revision are still
    // nowhere on a component.)
    fixture.patch(
        &id,
        json!({
            "participants": {"p1": {"@type": "Participant", "email": "vera@example.com"}},
            "sequence": 3,
        }),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("SUMMARY:Standup", "SUMMARY:Standup (short)");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.title.as_deref(), Some("Standup (short)"));
    assert_eq!(
        stored.extra.get("sequence"),
        Some(&json!(3)),
        "an unmapped property was overwritten"
    );
    assert!(stored.extra.contains_key("participants"));
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
    // the same reason a rule with a `byDay` leaves `recurrenceRules` untouched.
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
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "daily", "count": 10},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(
        icalendar.contains("RRULE:FREQ=DAILY;COUNT=10"),
        "{icalendar}"
    );
    let edited = icalendar.replace("COUNT=10", "COUNT=5");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].frequency, "daily");
    assert_eq!(rules[0].count, Some(5));
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
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "rscale": "chinese",
        }]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;COUNT=4");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].extra.get("rscale"),
        Some(&json!("chinese")),
        "a rule part the RRULE could not carry was dropped"
    );
    assert_eq!(
        rules[0].count, None,
        "narrowing a rule we cannot fully see is worse than ignoring the edit"
    );
}

#[test]
fn the_days_a_weekly_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "byDay": [{"@type": "NDay", "day": "mo"}],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(
        rules[0].by_day.as_deref(),
        Some(&[NDay::new("mo"), NDay::new("th")][..])
    );
}

#[test]
fn the_days_of_the_month_a_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "byMonthDay": [15],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_month_day.as_deref(), Some(&[-1][..]));
}

#[test]
fn the_months_a_yearly_rule_repeats_in_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Tax return", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byMonth": ["3"],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(
        rules[0].by_month.as_deref(),
        Some(&["3".to_owned(), "9".to_owned()][..])
    );
}

#[test]
fn the_days_of_the_year_a_rule_repeats_on_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "New Year", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byYearDay": [1],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_year_day.as_deref(), Some(&[1, -1][..]));
}

#[test]
fn a_day_of_the_year_the_rrule_should_not_carry_is_not_sent() {
    // `FREQ=MONTHLY;BYYEARDAY=100` is a rule RFC 5545 §3.3.10 does not admit — a
    // month is not a period a day of the year sits inside — and neither calcard
    // nor libical judges it, so the check is on the way out: `recurrenceRules` goes
    // to the server replaced whole, and one part it is entitled to reject would
    // cost every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "monthly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYYEARDAY=100")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_year_day, None);
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
    // check is on the way out: `recurrenceRules` goes to the server replaced
    // whole, and one part it is entitled to reject would cost every other edit in
    // the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Festival", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "yearly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=YEARLY", "RRULE:FREQ=YEARLY;BYMONTH=5L")
        .replace("SUMMARY:Festival", "SUMMARY:Spring festival");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_month, None);
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
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "interval": 2,
            "byDay": [{"@type": "NDay", "day": "tu"}],
            "firstDayOfWeek": "su",
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].first_day_of_week.as_deref(), Some("sa"));
}

#[test]
fn a_day_no_week_starts_on_is_not_sent() {
    // A `firstDayOfWeek` outside RFC 8984 §4.3.3's closed vocabulary is one no
    // `WKST` can say, and libical refuses a component carrying `WKST=XX` outright
    // — so the rule is shown without it, and a save must not write the property
    // back: `recurrenceRules` goes to the server replaced whole, so the day would
    // be dropped from the server's own rule by a save that never touched the
    // recurrence.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "weekly",
            "firstDayOfWeek": "xx",
        }]}),
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
    let rules = stored.recurrence_rules.unwrap();
    assert_eq!(
        rules[0].first_day_of_week.as_deref(),
        Some("xx"),
        "the day the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules[0].count, None,
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
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byWeekNo": [1],
            "firstDayOfWeek": "su",
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_week_no.as_deref(), Some(&[1, -1][..]));
    assert_eq!(
        rules[0].first_day_of_week.as_deref(),
        Some("su"),
        "the day the weeks are counted from goes with them"
    );
}

#[test]
fn a_week_of_the_year_the_rrule_should_not_carry_is_not_sent() {
    // `FREQ=MONTHLY;BYWEEKNO=20` is a rule RFC 5545 §3.3.10 does not admit — it
    // admits `BYWEEKNO` beside `YEARLY` and nothing else — and neither calcard nor
    // libical judges it, so the check is on the way out: `recurrenceRules` goes to
    // the server replaced whole, and one part it is entitled to reject would cost
    // every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Rent", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "monthly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYWEEKNO=20")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_week_no, None);
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
    // without it, and a save must not write the property back: `recurrenceRules`
    // goes to the server replaced whole, so the week would be dropped from the
    // server's own rule by a save that only narrowed it.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Stocktake", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "byWeekNo": [54],
        }]}),
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
    let rules = stored.recurrence_rules.unwrap();
    assert_eq!(
        rules[0].by_week_no.as_deref(),
        Some(&[54][..]),
        "the week the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules[0].count, None,
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
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "monthly",
            "byDay": [{"@type": "NDay", "day": "fr"}],
            "bySetPosition": [-1],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_set_position.as_deref(), Some(&[1, -1][..]));
    assert_eq!(
        rules[0].by_day.as_ref().unwrap()[0].day,
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
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "monthly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MONTHLY", "RRULE:FREQ=MONTHLY;BYSETPOS=2")
        .replace("SUMMARY:Rent", "SUMMARY:Rent, due");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_set_position, None);
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
    // and `recurrenceRules` goes back replaced whole, so a save that only
    // narrowed the rule would delete the position from the server's own copy.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Stocktake", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "yearly",
            "bySetPosition": [-1],
        }]}),
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
    let rules = stored.recurrence_rules.unwrap();
    assert_eq!(
        rules[0].by_set_position.as_deref(),
        Some(&[-1][..]),
        "the position the server holds is left alone rather than cleared"
    );
    assert_eq!(
        rules[0].count, None,
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
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "daily",
            "byHour": [9],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_hour.as_deref(), Some(&[9, 14][..]));
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
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "daily"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=DAILY", "RRULE:FREQ=DAILY;BYHOUR=24")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_hour, None);
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
    // reason, since `recurrenceRules` is replaced whole.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "daily",
            "byHour": [9, 14],
        }]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace(
        "RRULE:FREQ=DAILY;BYHOUR=9,14",
        "RRULE:FREQ=DAILY;BYHOUR=9,14;COUNT=4",
    );
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_hour.as_deref(), Some(&[9, 14][..]));
    assert_eq!(rules[0].count, Some(4), "the edit itself still has to land");
}

#[test]
fn the_minutes_and_seconds_a_rule_repeats_at_reach_the_server() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sensor poll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "hourly",
            "byMinute": [0],
            "bySecond": [0],
        }]}),
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

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules[0].by_second.as_deref(), Some(&[0][..]));
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
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "minutely"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=MINUTELY", "RRULE:FREQ=MINUTELY;BYSECOND=60")
        .replace("SUMMARY:Sensor poll", "SUMMARY:Poll on the second");
    sync.save_component(&edited, Some(id.as_str())).unwrap();
    assert_eq!(
        fixture.event(&id).recurrence_rules.unwrap()[0]
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
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_minute, None);
    assert_eq!(
        stored.title.as_deref(),
        Some("Sensor poll"),
        "the edit the save could carry still has to land"
    );
}

#[test]
fn minutes_the_server_holds_are_not_cleared_by_a_save_that_narrowed_the_rule() {
    // The direction that loses data: `recurrenceRules` is replaced whole, so a
    // save that touched the rule for another reason has to carry the minutes and
    // seconds the server already held back out with it.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Sensor poll", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [{
            "@type": "RecurrenceRule",
            "frequency": "hourly",
            "byMinute": [0, 30],
            "bySecond": [15],
        }]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar.replace("BYMINUTE=0,30", "BYMINUTE=0,30;COUNT=4");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let rules = fixture.event(&id).recurrence_rules.unwrap();
    assert_eq!(rules[0].by_minute.as_deref(), Some(&[0, 30][..]));
    assert_eq!(rules[0].by_second.as_deref(), Some(&[15][..]));
    assert_eq!(rules[0].count, Some(4), "the edit itself still has to land");
}

#[test]
fn a_day_of_the_month_the_rrule_should_not_carry_is_not_sent() {
    // `FREQ=WEEKLY;BYMONTHDAY=15` is a rule RFC 5545 §3.3.10 does not admit, and
    // calcard hands it back rather than judging it — so, as with an ordinal
    // weekday, the check is on the way out: `recurrenceRules` goes to the server
    // replaced whole, and one part it may reject would cost every other edit in
    // the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "weekly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;BYMONTHDAY=15")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_month_day, None);
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
    // own `timeZone`: `recurrenceRules` goes to the server replaced whole, and
    // one part it is entitled to reject would cost every other edit in the save.
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.patch(
        &id,
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "weekly"},
        ]}),
    );
    let sync = fixture.sync();

    let icalendar = sync.load_component(id.as_str()).unwrap().icalendar;
    let edited = icalendar
        .replace("RRULE:FREQ=WEEKLY", "RRULE:FREQ=WEEKLY;BYDAY=2MO")
        .replace("SUMMARY:Standup", "SUMMARY:Daily standup");
    sync.save_component(&edited, Some(id.as_str())).unwrap();

    let stored = fixture.event(&id);
    assert_eq!(stored.recurrence_rules.unwrap()[0].by_day, None);
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
        json!({"recurrenceRules": [
            {"@type": "RecurrenceRule", "frequency": "daily"},
        ]}),
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
    assert_eq!(stored.recurrence_rules.unwrap()[0].frequency, "daily");
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
    assert_eq!(stored.recurrence_rules.unwrap()[0].frequency, "daily");
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
            "recurrenceRules": [{
                "@type": "RecurrenceRule",
                "frequency": "weekly",
                "bySetPosition": [-1],
            }],
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
    for tzid in ["W. Europe Standard Time", LIBICAL_TZID] {
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
    let icalendar = NEW_EVENT.replace("TZID=Europe/Berlin", "TZID=W. Europe Standard Time");

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
             DTSTART;TZID=W. Europe Standard Time:20260116T100000\r\n\
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
fn a_tag_the_component_could_not_show_leaves_the_whole_set_alone() {
    // RFC 8984 §1.4.3 has every value of a Set be `true`; this server said
    // otherwise, so the tag never reached the CATEGORIES line — and a set shown
    // in part is not the user's to have edited. The property goes back replaced
    // whole, so writing it would delete the entry the user never saw.
    let fixture = Fixture::start();
    let keywords = json!({"offsite": true, "odd": "yes"});
    let (id, icalendar) = tagged(&fixture, keywords.clone());
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
        Some(serde_json::from_value(keywords).unwrap()),
        "a property shown in part must not be written back"
    );
    assert_eq!(
        stored.title.as_deref(),
        Some("Standup (short)"),
        "the edit the user made must still arrive"
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
    fixture.patch(&id, json!({"recurrenceRules": [{"frequency": "weekly"}]}));
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
            "recurrenceRules": [{"frequency": "weekly"}],
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
    fixture.patch(&id, json!({"recurrenceRules": [{"frequency": "weekly"}]}));
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
    fixture.patch(&id, json!({"recurrenceRules": [{"frequency": "weekly"}]}));
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
    fixture.patch(&id, json!({"recurrenceRules": [{"frequency": "weekly"}]}));
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
