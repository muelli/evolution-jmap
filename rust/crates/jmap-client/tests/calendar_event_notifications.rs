// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalendarEventNotification/get|query|set` (draft-ietf-jmap-calendars §8):
//! a create/update/destroy of an event on a calendar shared with someone
//! else produces a notification for them, never for the actor making the
//! change. The object is entirely server-created: only destroy is a valid
//! `/set` mutation, create and update are always rejected.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE};
use serde_json::json;

fn new_event(calendar_id: &Id, title: &str) -> CalendarEvent {
    CalendarEvent {
        calendar_ids: Some([(calendar_id.clone(), true)].into()),
        version: Some("2.0".to_owned()),
        start: Some("2026-06-01T10:00:00".to_owned()),
        title: Some(title.to_owned()),
        ..CalendarEvent::default()
    }
}

#[test]
fn shared_calendar_event_lifecycle_notifies_the_other_principal_not_the_actor() {
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

    let created = alice
        .event_create(&account_id, &new_event(&work, "Standup"))
        .unwrap();
    let event_id = created.id.clone().unwrap();

    let notifications = bob_client
        .calendar_event_notifications(&account_id)
        .unwrap();
    assert_eq!(notifications.len(), 1, "the create notified bob");
    assert_eq!(notifications[0].event_id, event_id);
    assert_eq!(
        notifications[0].notification_type.as_deref(),
        Some("created")
    );
    assert_eq!(
        notifications[0]
            .event
            .as_ref()
            .and_then(|event| event.title.as_deref()),
        Some("Standup")
    );

    assert!(
        alice
            .calendar_event_notifications(&account_id)
            .unwrap()
            .is_empty(),
        "the actor is never their own recipient"
    );

    alice
        .event_update(&account_id, &event_id, json!({"title": "Standup (moved)"}))
        .unwrap();

    let notifications = bob_client
        .calendar_event_notifications(&account_id)
        .unwrap();
    assert_eq!(
        notifications.len(),
        2,
        "the update produced a second notification"
    );
    assert_eq!(
        notifications[1].notification_type.as_deref(),
        Some("updated")
    );
    assert_eq!(
        notifications[1]
            .event
            .as_ref()
            .and_then(|event| event.title.as_deref()),
        Some("Standup (moved)")
    );

    alice.event_destroy(&account_id, &event_id).unwrap();

    let notifications = bob_client
        .calendar_event_notifications(&account_id)
        .unwrap();
    assert_eq!(
        notifications.len(),
        3,
        "the destroy produced a third notification"
    );
    assert_eq!(
        notifications[2].notification_type.as_deref(),
        Some("destroyed")
    );
    assert_eq!(notifications[2].event_id, event_id);
    assert!(
        notifications[2].event.is_none(),
        "the event is gone by the time it's destroyed"
    );
}

#[test]
fn unshared_calendar_event_change_notifies_nobody() {
    let bob = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", bob.clone())
        .start();
    let account_id = server.account_id();
    let private = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Private", false)
    };

    let alice = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    alice
        .event_create(&account_id, &new_event(&private, "Solo"))
        .unwrap();

    assert!(
        bob_client
            .calendar_event_notifications(&account_id)
            .unwrap()
            .is_empty(),
        "no one shares this calendar, so no notification goes out"
    );
}

#[test]
fn destroy_dismisses_a_notification_and_create_update_are_forbidden() {
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
    alice
        .event_create(&account_id, &new_event(&work, "Standup"))
        .unwrap();

    let notifications = bob_client
        .calendar_event_notifications(&account_id)
        .unwrap();
    let notification_id = notifications[0].id.clone().unwrap();

    bob_client
        .calendar_event_notification_destroy(&account_id, &notification_id)
        .unwrap();
    assert!(
        bob_client
            .calendar_event_notifications(&account_id)
            .unwrap()
            .is_empty(),
        "destroy dismisses it"
    );

    // Create and update are always rejected: the object is entirely
    // server-created (draft §8, verified against real Stalwart's
    // `calendar_event_notification/set.rs`), so there is no `Client`
    // convenience method for either — this drives the wire call directly.
    let response = bob_client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CALENDARS],
            "CalendarEventNotification/set",
            &json!({
                "accountId": account_id,
                "create": {"new": {"eventId": "E1", "created": "2026-01-01T00:00:00Z"}},
            }),
        )
        .unwrap();
    assert_eq!(
        response["notCreated"]["new"]["type"],
        json!("forbidden"),
        "create is always rejected"
    );
}
