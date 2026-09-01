// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar CRUD against the mock server (draft-ietf-jmap-calendars).

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarEvent, CalendarEventQueryFilter, RecurrenceRule};
use serde_json::json;

fn server_with_calendar() -> (MockServer, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let calendar = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    (server, account_id, calendar)
}

#[test]
fn event_create() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendars = client.calendars(&account_id).unwrap();
    assert_eq!(calendars.len(), 1);
    assert_eq!(calendars[0].id.as_ref(), Some(&calendar));

    let event = CalendarEvent::simple(calendar, "Dentist", "2026-09-01T09:00:00", "PT30M");
    let created = client.event_create(&account_id, &event).unwrap();

    let id = created.id.expect("server assigned id");
    assert!(created.uid.is_some(), "server assigns a uid");
    assert_eq!(created.event_type.as_deref(), Some("Event"));

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(account.calendar_events.contains(&id));
}

#[test]
fn event_create_requires_calendar_and_start() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let in_unknown_calendar =
        CalendarEvent::simple("CAL999", "Ghost", "2026-09-01T09:00:00", "PT30M");
    match client.event_create(&account_id, &in_unknown_calendar) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected Set error, got {other:?}"),
    }

    let mut without_start = CalendarEvent::simple(calendar, "No start", "", "PT30M");
    without_start.start = None;
    match client.event_create(&account_id, &without_start) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn calendar_create() {
    let (server, account_id, _personal) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendar = Calendar {
        name: "Work".to_owned(),
        ..Calendar::default()
    };
    let created = client.calendar_create(&account_id, &calendar).unwrap();

    let id = created.id.expect("server assigned id");
    assert_eq!(created.name, "Work");

    // White box: it is really in the store.
    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(account.calendars.contains(&id));

    drop(state);
    let calendars = client.calendars(&account_id).unwrap();
    assert_eq!(calendars.len(), 2, "the seeded calendar plus the new one");
}

#[test]
fn calendar_create_requires_name() {
    let (server, account_id, _calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let unnamed = Calendar::default();
    match client.calendar_create(&account_id, &unnamed) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn calendar_create_rejects_client_supplied_id() {
    let (server, account_id, _calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let calendar = Calendar {
        id: Some(Id::new("CAL999")),
        name: "Sneaky".to_owned(),
        ..Calendar::default()
    };
    match client.calendar_create(&account_id, &calendar) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "invalidProperties"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn calendar_destroy() {
    let (server, account_id, _personal) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .calendar_create(
            &account_id,
            &Calendar {
                name: "Temporary".to_owned(),
                ..Calendar::default()
            },
        )
        .unwrap();
    let id = created.id.unwrap();

    client.calendar_destroy(&account_id, &id).unwrap();

    let state = server.state();
    let state = state.lock().unwrap();
    let account = state.account(&account_id).unwrap();
    assert!(!account.calendars.contains(&id));
    drop(state);

    // Destroying again fails with notFound.
    match client.calendar_destroy(&account_id, &id) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn event_get_by_id() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .event_create(
            &account_id,
            &CalendarEvent::simple(calendar, "Standup", "2026-09-02T10:00:00", "PT15M"),
        )
        .unwrap();
    let id = created.id.unwrap();

    let response = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap();
    assert_eq!(response.list.len(), 1);
    let event = &response.list[0];
    assert_eq!(event.title.as_deref(), Some("Standup"));
    assert_eq!(event.start.as_deref(), Some("2026-09-02T10:00:00"));
    assert_eq!(event.duration.as_deref(), Some("PT15M"));

    let missing = client.event_get(&account_id, &[Id::new("CE404")]).unwrap();
    assert!(missing.list.is_empty());
    assert_eq!(missing.not_found, vec![Id::new("CE404")]);
}

#[test]
fn calendar_update_color() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .calendar_update(&account_id, &calendar, json!({"color": "#00ff00"}))
        .unwrap();

    let updated = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&calendar))
        .unwrap();
    assert_eq!(updated.color.as_deref(), Some("#00ff00"));
}

#[test]
fn event_update_recurrence() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .event_create(
            &account_id,
            &CalendarEvent::simple(calendar, "Yoga", "2026-09-03T18:00:00", "PT1H"),
        )
        .unwrap();
    let id = created.id.unwrap();
    assert!(created.recurrence_rule.is_none());

    let weekly = RecurrenceRule::new("weekly");
    client
        .event_update(
            &account_id,
            &id,
            json!({"recurrenceRule": weekly, "title": "Yoga (weekly)"}),
        )
        .unwrap();

    let event = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap()
        .list
        .remove(0);
    assert_eq!(event.title.as_deref(), Some("Yoga (weekly)"));
    let rules = event.recurrence_rule.as_ref().unwrap();
    assert_eq!(rules.frequency, "weekly");
    // Untouched properties survive the patch.
    assert_eq!(event.start.as_deref(), Some("2026-09-03T18:00:00"));
}

#[test]
fn event_destroy() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .event_create(
            &account_id,
            &CalendarEvent::simple(calendar, "Doomed", "2026-09-04T09:00:00", "PT1H"),
        )
        .unwrap();
    let id = created.id.unwrap();

    client.event_destroy(&account_id, &id).unwrap();

    let response = client
        .event_get(&account_id, std::slice::from_ref(&id))
        .unwrap();
    assert!(response.list.is_empty());
    assert_eq!(response.not_found, vec![id.clone()]);

    match client.event_destroy(&account_id, &id) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Set error, got {other:?}"),
    }
}

#[test]
fn event_query_time_range() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let january = client
        .event_create(
            &account_id,
            &CalendarEvent::simple(calendar.clone(), "January", "2026-01-15T13:00:00", "PT1H"),
        )
        .unwrap();
    let february = client
        .event_create(
            &account_id,
            &CalendarEvent::simple(calendar.clone(), "February", "2026-02-20T09:00:00", "PT1H"),
        )
        .unwrap();

    let ids = client
        .event_query(
            &account_id,
            CalendarEventQueryFilter::time_range("2026-02-01T00:00:00Z", "2026-03-01T00:00:00Z"),
        )
        .unwrap()
        .ids;
    assert_eq!(ids, vec![february.id.clone().unwrap()]);

    // Whole-year range returns both, sorted by start.
    let ids = client
        .event_query(
            &account_id,
            CalendarEventQueryFilter::time_range("2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z"),
        )
        .unwrap()
        .ids;
    assert_eq!(ids, vec![january.id.unwrap(), february.id.unwrap()]);

    // in_calendar filter composes with everything else.
    let ids = client
        .event_query(&account_id, CalendarEventQueryFilter::in_calendar(calendar))
        .unwrap()
        .ids;
    assert_eq!(ids.len(), 2);
}

/// Track E Phase C step 2: a `shareWith` grant on a `Calendar` restricts and
/// grants cross-account visibility the same way it already does for
/// `AddressBook` and `Mailbox` (`jmap-client/tests/contacts.rs`,
/// `jmap-client/tests/mail_folders.rs`), matching the live Stalwart probe's
/// findings (Track E Phase C step 1, recorded in the work queue).
#[test]
fn calendar_shared_with_a_principal_is_visible_only_to_them() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let (_personal, work) = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let personal = account.seed_calendar("Personal", true);
        let work = account.seed_calendar("Work", false);
        (personal, work)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    match bob_client.calendars(&account_id) {
        Err(Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, "forbidden");
        }
        other => panic!("expected forbidden before any grant, got {other:?}"),
    }

    alice
        .calendar_update(
            &account_id,
            &work,
            json!({"shareWith": {bob.as_str(): {"mayReadItems": true, "mayReadFreeBusy": true}}}),
        )
        .unwrap();

    let bob_calendars = bob_client.calendars(&account_id).unwrap();
    assert_eq!(
        bob_calendars.len(),
        1,
        "bob only sees the shared calendar, not Personal"
    );
    assert_eq!(bob_calendars[0].id.as_ref(), Some(&work));
    let rights = bob_calendars[0]
        .my_rights
        .as_ref()
        .expect("myRights is computed from the grant");
    assert_eq!(rights.may_read_items, Some(true));
    assert_eq!(rights.may_read_free_busy, Some(true));

    assert_eq!(
        alice.calendars(&account_id).unwrap().len(),
        2,
        "the owner's own view still sees both calendars"
    );
}
