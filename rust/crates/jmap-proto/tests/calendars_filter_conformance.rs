// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEventQueryFilter, CalendarSetRequest};
use serde_json::json;

#[test]
fn calendar_event_query_filter_models_standard_fields() {
    let filter_json = json!({
        "timeZone": "Europe/Berlin",
        "participationStatus": "accepted",
        "inCalendarList": ["cal1", "cal2"],
        "excludedFromCalendar": "cal3"
    });

    let filter: Result<CalendarEventQueryFilter, _> = serde_json::from_value(filter_json);
    assert!(filter.is_ok());
    let filter = filter.unwrap();
    let roundtrip = serde_json::to_value(&filter).unwrap();
    assert_eq!(roundtrip["timeZone"], "Europe/Berlin");
    assert_eq!(roundtrip["participationStatus"], "accepted");
    assert_eq!(roundtrip["inCalendarList"], json!(["cal1", "cal2"]));
    assert_eq!(roundtrip["excludedFromCalendar"], "cal3");
}

#[test]
fn calendar_event_query_filter_accepts_in_calendars_alias() {
    let filter_json = json!({
        "inCalendars": ["cal1", "cal2"]
    });

    let filter: CalendarEventQueryFilter = serde_json::from_value(filter_json).unwrap();
    assert_eq!(
        filter.in_calendar_list,
        Some(vec![Id::from("cal1"), Id::from("cal2")])
    );
}

#[test]
fn calendar_event_query_filter_builder_methods() {
    let filter = CalendarEventQueryFilter::default()
        .with_time_zone("America/New_York")
        .with_participation_status("needs-action")
        .with_in_calendar_list([Id::from("cal-a"), Id::from("cal-b")])
        .with_excluded_from_calendar(Id::from("cal-c"));

    assert_eq!(filter.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(filter.participation_status.as_deref(), Some("needs-action"));
    assert_eq!(
        filter.in_calendar_list,
        Some(vec![Id::from("cal-a"), Id::from("cal-b")])
    );
    assert_eq!(
        filter.excluded_from_calendar.as_ref().map(Id::as_str),
        Some("cal-c")
    );
}

#[test]
fn calendar_set_request_models_on_destroy_remove_variants() {
    let req = CalendarSetRequest::new("acc1")
        .with_on_destroy_remove_contents(true)
        .with_on_destroy_remove_events(true);

    assert_eq!(req.on_destroy_remove_contents, Some(true));
    assert_eq!(req.on_destroy_remove_events, Some(true));

    let val = serde_json::to_value(&req).unwrap();
    assert_eq!(val["onDestroyRemoveContents"], true);
    assert_eq!(val["onDestroyRemoveEvents"], true);

    let parsed: CalendarSetRequest = serde_json::from_value(json!({
        "accountId": "acc2",
        "onDestroyRemoveContents": false,
        "onDestroyRemoveEvents": true
    }))
    .unwrap();
    assert_eq!(parsed.on_destroy_remove_contents, Some(false));
    assert_eq!(parsed.on_destroy_remove_events, Some(true));
}
