// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalendarEvent/get` naming more ids than `maxObjectsInGet` (RFC 8620 §2)
//! must split across requests, mirroring `Client::contact_get`'s own
//! chunking (`jmap-client/tests/contact_get_chunking.rs`), which was itself
//! added the hard way scale-testing item 94's `jmap-book-sync` slice against
//! real Stalwart. `event_get` had the identical gap: `CalSync::classify`
//! calls it with every created/updated id `CalendarEvent/changes` names,
//! unbounded by anything this crate controls, so a calendar of real size hits
//! the same `requestTooLarge` a 500-card address book did.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::error::method::REQUEST_TOO_LARGE;

fn server_with_calendar() -> (MockServer, Id, Id) {
    let server = MockServer::builder().objects_in_get(2).start();
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

/// A server enforcing `maxObjectsInGet` would refuse a `CalendarEvent/get`
/// naming every id at once with `requestTooLarge` — proven here by driving
/// the raw request past `event_get`, the same way
/// `a_server_enforcing_max_objects_in_get_refuses_a_contact_card_get_exceeding_it`
/// proves it for `ContactCard/get`. `event_get` itself must never let a
/// caller hit this.
#[test]
fn a_server_enforcing_max_objects_in_get_refuses_a_calendar_event_get_exceeding_it() {
    let (server, account_id, _calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = jmap_proto::request::Request::new([
        jmap_proto::session::CAPABILITY_CORE,
        jmap_proto::session::CAPABILITY_CALENDARS,
    ])
    .call(
        "CalendarEvent/get",
        &serde_json::json!({
            "accountId": account_id,
            "ids": ["id1", "id2", "id3"],
        }),
        "c1",
    )
    .unwrap();

    let response = client.api_call(&request).unwrap();
    let invocation = response.responses_for("c1").next().unwrap();
    assert!(invocation.is_error());
    let error: jmap_proto::error::MethodError = invocation.parse().unwrap();
    assert_eq!(error.error_type, REQUEST_TOO_LARGE);
}

/// `Client::event_get` naming more ids than `maxObjectsInGet` splits across
/// requests so every event is still fetched, same guarantee `contact_get`
/// gives.
#[test]
fn event_get_splits_across_max_objects_in_get() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let mut ids = Vec::new();
    for i in 0..3 {
        let event = CalendarEvent::simple(
            calendar.clone(),
            &format!("Event {i}"),
            "2026-09-20T09:00:00",
            "PT1H",
        );
        let created = client.event_create(&account_id, &event).unwrap();
        ids.push(created.id.expect("server assigned id"));
    }

    let before = server.api_requests();
    let response = client.event_get(&account_id, &ids).unwrap();

    assert_eq!(
        response.list.len(),
        3,
        "every event should still be fetched"
    );
    assert_eq!(
        server.api_requests() - before,
        2,
        "three ids with maxObjectsInGet=2 split across two requests"
    );
}

/// `event_get` with no ids must still reach the server for a real state, not
/// synthesize one locally just because the chunking loop saw nothing to
/// chunk — same guarantee `contact_get` gives `Client::contact_state`.
#[test]
fn event_get_with_no_ids_still_reaches_the_server_for_the_real_state() {
    let (server, account_id, calendar) = server_with_calendar();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let state_before = client.event_get(&account_id, &[]).unwrap().state;

    let event = CalendarEvent::simple(calendar, "Vera's Event", "2026-09-20T09:00:00", "PT1H");
    client.event_create(&account_id, &event).unwrap();

    let state_after = client.event_get(&account_id, &[]).unwrap().state;
    assert_ne!(
        state_before, state_after,
        "state must advance after a create, proving the empty-ids call reached the server"
    );
}
