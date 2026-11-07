// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::calendars::{Calendar, CalendarEvent, RecurrenceRule};
use jmap_proto::error::set::INVALID_PROPERTIES;
use serde_json::json;

fn setup_calendar(client: &Client, account_id: &jmap_proto::Id) -> (jmap_proto::Id, CalendarEvent) {
    let cal = client
        .calendar_create(account_id, &Calendar::new("TestCalendar"))
        .unwrap();
    let cal_id = cal.id.unwrap();

    let mut event = CalendarEvent::new();
    event.version = Some("2.0".to_owned());
    event.title = Some("Valid Event".to_owned());
    event.start = Some("2026-11-02T10:00:00".to_owned());
    event.duration = Some("PT1H".to_owned());
    event.calendar_ids = Some([(cal_id.clone(), true)].into_iter().collect());
    (cal_id, event)
}

#[test]
fn calendar_event_show_without_time_rejects_non_null_timezone() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.start = Some("2026-11-02T00:00:00".to_owned());
    event.duration = Some("P1D".to_owned());
    event.show_without_time = Some(true);
    event.time_zone = Some("America/New_York".to_owned());

    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_show_without_time_rejects_non_midnight_start() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.start = Some("2026-11-02T14:30:00".to_owned());
    event.duration = Some("P1D".to_owned());
    event.show_without_time = Some(true);
    event.time_zone = None;

    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_show_without_time_rejects_non_zero_time_duration() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.start = Some("2026-11-02T00:00:00".to_owned());
    event.duration = Some("PT2H".to_owned());
    event.show_without_time = Some(true);
    event.time_zone = None;

    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_show_without_time_accepts_valid_all_day_event() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.start = Some("2026-11-02T00:00:00".to_owned());
    event.duration = Some("P1D".to_owned());
    event.show_without_time = Some(true);
    event.time_zone = None;

    let created = client.event_create(&account_id, &event);
    assert!(created.is_ok(), "valid all-day event must be accepted");
}

#[test]
fn calendar_event_rejects_invalid_recurrence_frequency() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.recurrence_rule = Some(RecurrenceRule::new("biweekly"));

    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_rejects_recurrence_interval_zero() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.recurrence_rule = Some(RecurrenceRule::new("daily").with_interval(0));

    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_accepts_valid_recurrence_rule() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.recurrence_rule = Some(RecurrenceRule::new("weekly").with_interval(2));

    let created = client.event_create(&account_id, &event);
    assert!(created.is_ok(), "valid recurrence rule must be accepted");
}

#[test]
fn calendar_event_rejects_out_of_range_priority() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.priority = Some(42);
    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }

    event.priority = Some(-1);
    let err2 = client.event_create(&account_id, &event).unwrap_err();
    match err2 {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_accepts_valid_priority() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.priority = Some(0);
    let created = client.event_create(&account_id, &event).unwrap();
    let event_id = created.id.unwrap();

    let patch = json!({ "priority": 9 });
    let update_res = client.event_update(&account_id, &event_id, patch);
    assert!(update_res.is_ok(), "priority 9 must be accepted");
}

#[test]
fn calendar_event_rejects_invalid_free_busy_status() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.free_busy_status = Some("opaque".to_owned());
    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_accepts_valid_free_busy_status() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.free_busy_status = Some("free".to_owned());
    let created = client.event_create(&account_id, &event).unwrap();
    let event_id = created.id.unwrap();

    let patch = json!({ "freeBusyStatus": "busy" });
    let update_res = client.event_update(&account_id, &event_id, patch);
    assert!(update_res.is_ok(), "freeBusyStatus busy must be accepted");
}

#[test]
fn calendar_event_rejects_invalid_privacy() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.privacy = Some("confidential".to_owned());
    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_accepts_valid_privacy() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.privacy = Some("public".to_owned());
    let created = client.event_create(&account_id, &event).unwrap();
    let event_id = created.id.unwrap();

    let patch = json!({ "privacy": "private" });
    let update_res = client.event_update(&account_id, &event_id, patch);
    assert!(update_res.is_ok(), "privacy private must be accepted");

    let patch2 = json!({ "privacy": "secret" });
    let update_res2 = client.event_update(&account_id, &event_id, patch2);
    assert!(update_res2.is_ok(), "privacy secret must be accepted");
}

#[test]
fn calendar_event_rejects_invalid_status() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.status = Some("in-progress".to_owned());
    let err = client.event_create(&account_id, &event).unwrap_err();
    match err {
        Error::Set(set_error) => {
            assert_eq!(set_error.error_type, INVALID_PROPERTIES);
        }
        other => panic!("expected Error::Set(invalidProperties), got {other:?}"),
    }
}

#[test]
fn calendar_event_accepts_valid_status() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();
    let (_cal_id, mut event) = setup_calendar(&client, &account_id);

    event.status = Some("confirmed".to_owned());
    let created = client.event_create(&account_id, &event).unwrap();
    let event_id = created.id.unwrap();

    let patch = json!({ "status": "tentative" });
    let update_res = client.event_update(&account_id, &event_id, patch);
    assert!(update_res.is_ok(), "status tentative must be accepted");

    let patch2 = json!({ "status": "cancelled" });
    let update_res2 = client.event_update(&account_id, &event_id, patch2);
    assert!(update_res2.is_ok(), "status cancelled must be accepted");
}
