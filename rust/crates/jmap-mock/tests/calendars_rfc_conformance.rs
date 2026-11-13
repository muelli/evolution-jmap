// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::State;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotificationQueryFilter, CalendarEventQueryFilter,
    ParticipantIdentity, participant_identity_set_error,
};
use jmap_proto::methods::{Comparator, QueryRequest, QueryResponse};
use serde_json::json;

#[test]
fn participant_identity_rejects_duplicate_calendar_address() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let id1 =
        ParticipantIdentity::new("First Identity").with_calendar_address("mailto:user@example.com");
    let created1 = client.participant_identity_create(&account_id, &id1);
    assert!(created1.is_ok());

    let id2 = ParticipantIdentity::new("Duplicate Identity")
        .with_calendar_address("mailto:user@example.com");
    let created2 = client.participant_identity_create(&account_id, &id2);
    match created2 {
        Err(Error::Set(set_error)) => {
            assert_eq!(
                set_error.error_type,
                participant_identity_set_error::CALENDAR_ADDRESS_IN_USE
            );
        }
        other => panic!("expected Error::Set(calendarAddressInUse), got {other:?}"),
    }
}

#[test]
fn participant_identity_update_rejects_duplicate_calendar_address() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let _id1 = client
        .participant_identity_create(
            &account_id,
            &ParticipantIdentity::new("First").with_calendar_address("mailto:one@example.com"),
        )
        .unwrap();

    let id2 = client
        .participant_identity_create(
            &account_id,
            &ParticipantIdentity::new("Second").with_calendar_address("mailto:two@example.com"),
        )
        .unwrap();

    // Patch second identity to use the first identity's address
    let update_res = client.participant_identity_update(
        &account_id,
        id2.id.as_ref().unwrap(),
        json!({
            "calendarAddress": "mailto:one@example.com"
        }),
    );

    match update_res {
        Err(Error::Set(set_error)) => {
            assert_eq!(
                set_error.error_type,
                participant_identity_set_error::CALENDAR_ADDRESS_IN_USE
            );
        }
        other => panic!("expected Error::Set(calendarAddressInUse), got {other:?}"),
    }
}

#[test]
fn calendar_event_query_supports_descending_start_sort() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let cal = client
        .calendar_create(&account_id, &Calendar::new("SortTest"))
        .unwrap();
    let cal_id = cal.id.unwrap();

    let mut event1 = CalendarEvent::new();
    event1.version = Some("2.0".to_owned());
    event1.start = Some("2026-11-01T10:00:00".to_owned());
    event1.calendar_ids = Some([(cal_id.clone(), true)].into_iter().collect());
    let e1 = client.event_create(&account_id, &event1).unwrap();

    let mut event2 = CalendarEvent::new();
    event2.version = Some("2.0".to_owned());
    event2.start = Some("2026-11-02T10:00:00".to_owned());
    event2.calendar_ids = Some([(cal_id.clone(), true)].into_iter().collect());
    let e2 = client.event_create(&account_id, &event2).unwrap();

    let sort = vec![Comparator::descending("start")];
    let request = QueryRequest::new(account_id.clone())
        .filter(CalendarEventQueryFilter::in_calendar(cal_id))
        .sort(sort);
    let arguments = client
        .single_call(
            &[
                jmap_proto::session::CAPABILITY_CORE,
                jmap_proto::session::CAPABILITY_CALENDARS,
            ],
            "CalendarEvent/query",
            &request,
        )
        .unwrap();
    let response: QueryResponse = serde_json::from_value(arguments).unwrap();

    assert_eq!(
        response.ids,
        vec![e2.id.unwrap(), e1.id.unwrap()],
        "CalendarEvent/query MUST sort by start descending when requested per draft-ietf-jmap-calendars-33 §5.5"
    );
}

#[test]
fn calendar_event_query_matches_extended_filter_fields() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let cal_a = client
        .calendar_create(&account_id, &Calendar::new("CalA"))
        .unwrap();
    let cal_a_id = cal_a.id.unwrap();

    let cal_b = client
        .calendar_create(&account_id, &Calendar::new("CalB"))
        .unwrap();
    let cal_b_id = cal_b.id.unwrap();

    let mut event = CalendarEvent::new();
    event.version = Some("2.0".to_owned());
    event.start = Some("2026-11-05T12:00:00".to_owned());
    event.calendar_ids = Some([(cal_a_id.clone(), true)].into_iter().collect());
    event.participants = Some(
        [(
            "p1".to_string(),
            json!({
                "name": "Alice",
                "email": "alice@example.com",
                "participationStatus": "accepted"
            }),
        )]
        .into_iter()
        .collect(),
    );
    let created_event = client.event_create(&account_id, &event).unwrap();
    let event_id = created_event.id.unwrap();

    // Match in_calendar_list
    let filter_list = CalendarEventQueryFilter::default()
        .with_in_calendar_list([cal_a_id.clone(), cal_b_id.clone()]);
    let req = QueryRequest::new(account_id.clone()).filter(filter_list);
    let val = client
        .single_call(
            &[
                jmap_proto::session::CAPABILITY_CORE,
                jmap_proto::session::CAPABILITY_CALENDARS,
            ],
            "CalendarEvent/query",
            &req,
        )
        .unwrap();
    let res: QueryResponse = serde_json::from_value(val).unwrap();
    assert_eq!(res.ids, vec![event_id.clone()]);

    // Excluded from calendar A should not match
    let filter_excluded =
        CalendarEventQueryFilter::default().with_excluded_from_calendar(cal_a_id.clone());
    let req_exc = QueryRequest::new(account_id.clone()).filter(filter_excluded);
    let val_exc = client
        .single_call(
            &[
                jmap_proto::session::CAPABILITY_CORE,
                jmap_proto::session::CAPABILITY_CALENDARS,
            ],
            "CalendarEvent/query",
            &req_exc,
        )
        .unwrap();
    let res_exc: QueryResponse = serde_json::from_value(val_exc).unwrap();
    assert!(res_exc.ids.is_empty());

    // Match participation_status
    let filter_part = CalendarEventQueryFilter::default().with_participation_status("accepted");
    let req_part = QueryRequest::new(account_id.clone()).filter(filter_part);
    let val_part = client
        .single_call(
            &[
                jmap_proto::session::CAPABILITY_CORE,
                jmap_proto::session::CAPABILITY_CALENDARS,
            ],
            "CalendarEvent/query",
            &req_part,
        )
        .unwrap();
    let res_part: QueryResponse = serde_json::from_value(val_part).unwrap();
    assert_eq!(res_part.ids, vec![event_id]);
}

#[test]
fn calendar_event_notification_changes_routes_in_mock() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let changes = client.calendar_event_notification_changes(&account_id, &State::new("0"));
    assert!(
        changes.is_ok(),
        "CalendarEventNotification/changes MUST be routed per draft-ietf-jmap-calendars-33 §7.3"
    );
}

#[test]
fn calendar_event_notification_query_after_matches_exact_created_timestamp() {
    use jmap_proto::Id;

    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let work = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Work", false)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    alice
        .calendar_update(
            &account_id,
            &work,
            json!({"shareWith": {bob.as_str(): {"mayReadItems": true}}}),
        )
        .unwrap();

    let mut event = CalendarEvent::new();
    event.version = Some("2.0".to_owned());
    event.start = Some("2026-11-02T10:00:00".to_owned());
    event.calendar_ids = Some([(work, true)].into_iter().collect());
    alice.event_create(&account_id, &event).unwrap();

    let notifications = bob_client
        .calendar_event_notifications(&account_id)
        .unwrap();
    assert!(
        !notifications.is_empty(),
        "Creating an event in shared calendar should notify bob"
    );
    let notif = notifications.first().unwrap();
    let filter = CalendarEventNotificationQueryFilter::new().with_after(notif.created.clone());
    let ids = bob_client
        .calendar_event_notification_query(&account_id, filter)
        .unwrap();
    assert!(
        ids.contains(notif.id.as_ref().unwrap()),
        "Notification created at exact `after` timestamp MUST match filter per draft-ietf-jmap-calendars-33 §7.4.1"
    );
}
