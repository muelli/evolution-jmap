// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `isOrigin` on `CalendarEvent/get` (draft-ietf-jmap-calendars-28 section
//! 5.6): a server-set property telling the client whether this account is
//! the event's organiser (section 10.9.5), stamped fresh on every fetch
//! rather than stored, since a `ParticipantIdentity/set` can change which
//! address counts as "this account" between two fetches of the same event.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventSetRequest};
use jmap_proto::methods::SetRequest;

const ALICE: &str = "mailto:alice@example.com";
const BOB: &str = "mailto:bob@example.net";

struct Fixture {
    // Kept alive for the fixture's whole lifetime: `MockServer::start`'s
    // background thread stops the moment its handle is dropped.
    #[allow(dead_code)]
    server: MockServer,
    client: Client,
    account_id: Id,
    calendar_id: Id,
}

impl Fixture {
    fn new() -> Self {
        let server = MockServer::builder().bearer_token("alice-token").start();
        let account_id = server.account_id();
        let calendar_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_participant_identity("Alice", ALICE, true);
            account.seed_calendar("Work", true)
        };
        let client = Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
        Self {
            server,
            client,
            account_id,
            calendar_id,
        }
    }

    fn create(&self, organizer_calendar_address: Option<&str>) -> Id {
        let event = CalendarEvent {
            calendar_ids: Some([(self.calendar_id.clone(), true)].into()),
            version: Some("2.0".to_owned()),
            start: Some("2026-06-01T10:00:00".to_owned()),
            title: Some("Meeting".to_owned()),
            organizer_calendar_address: organizer_calendar_address.map(str::to_owned),
            ..CalendarEvent::default()
        };
        let request = CalendarEventSetRequest::new(
            SetRequest::new(self.account_id.clone()).create("new", event),
        );
        let response = self
            .client
            .event_set(&request)
            .expect("the set call itself succeeds");
        response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
            .and_then(|event| event.id.clone())
            .expect("the event was created")
    }

    fn is_origin(&self, id: &Id) -> Option<bool> {
        let response = self
            .client
            .event_get(&self.account_id, std::slice::from_ref(id))
            .expect("the get call succeeds");
        response
            .list
            .into_iter()
            .find(|event| event.id.as_ref() == Some(id))
            .expect("the event was found")
            .is_origin
    }
}

#[test]
fn this_account_organising_is_the_origin() {
    let fixture = Fixture::new();
    let id = fixture.create(Some(ALICE));
    assert_eq!(fixture.is_origin(&id), Some(true));
}

#[test]
fn someone_else_organising_is_not_the_origin() {
    let fixture = Fixture::new();
    let id = fixture.create(Some(BOB));
    assert_eq!(fixture.is_origin(&id), Some(false));
}

#[test]
fn no_named_organizer_defaults_to_this_account_being_the_origin() {
    let fixture = Fixture::new();
    let id = fixture.create(None);
    assert_eq!(fixture.is_origin(&id), Some(true));
}
