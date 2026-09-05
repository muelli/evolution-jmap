// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalendarEvent/set` with `sendSchedulingMessages`
//! (draft-ietf-jmap-calendars-28 section 5.9.2): which iTIP method goes to
//! whom when an event is created, changed or destroyed.
//!
//! The mock has no SMTP or iMIP path, so it records what a real server would
//! hand to its transport in a per-account scheduling outbox, the same way it
//! already records accepted `EmailSubmission`s. These tests read that outbox.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventSetRequest};
use jmap_proto::methods::{SetRequest, SetResponse};
use serde_json::{Value, json};

const ALICE: &str = "mailto:alice@example.com";
const BOB: &str = "mailto:bob@example.net";
const CAROL: &str = "mailto:carol@example.org";

/// One recorded message, flattened to what these tests care about.
type Sent = (String, String, Option<String>);

fn participant(address: &str, status: &str) -> Value {
    json!({
        "@type": "Participant",
        "calendarAddress": address,
        "participationStatus": status,
        "roles": {"attendee": true},
    })
}

fn meeting(calendar_id: &Id) -> CalendarEvent {
    CalendarEvent {
        calendar_ids: Some([(calendar_id.clone(), true)].into()),
        version: Some("2.0".to_owned()),
        start: Some("2026-06-01T10:00:00".to_owned()),
        title: Some("Design review".to_owned()),
        organizer_calendar_address: Some(ALICE.to_owned()),
        participants: Some(
            [
                ("alice".to_owned(), participant(ALICE, "accepted")),
                ("bob".to_owned(), participant(BOB, "needs-action")),
                ("carol".to_owned(), participant(CAROL, "needs-action")),
            ]
            .into(),
        ),
        ..CalendarEvent::default()
    }
}

/// An event this account did not organise: Bob invited Alice.
fn invitation(calendar_id: &Id) -> CalendarEvent {
    CalendarEvent {
        calendar_ids: Some([(calendar_id.clone(), true)].into()),
        version: Some("2.0".to_owned()),
        start: Some("2026-06-02T10:00:00".to_owned()),
        title: Some("Bob's meeting".to_owned()),
        organizer_calendar_address: Some(BOB.to_owned()),
        participants: Some(
            [
                ("bob".to_owned(), participant(BOB, "accepted")),
                ("alice".to_owned(), participant(ALICE, "needs-action")),
            ]
            .into(),
        ),
        ..CalendarEvent::default()
    }
}

struct Fixture {
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

    fn set(&self, set: SetRequest<CalendarEvent>, schedule: bool) -> SetResponse<CalendarEvent> {
        self.client
            .event_set(&CalendarEventSetRequest::new(set).send_scheduling_messages(schedule))
            .expect("the set call itself succeeds")
    }

    fn create(&self, event: &CalendarEvent, schedule: bool) -> Id {
        let response = self.set(
            SetRequest::new(self.account_id.clone()).create("new", event.clone()),
            schedule,
        );
        response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
            .and_then(|event| event.id.clone())
            .expect("the event was created")
    }

    fn update(&self, id: &Id, patch: Value, schedule: bool) {
        let response = self.set(
            SetRequest::new(self.account_id.clone()).update(id.clone(), patch),
            schedule,
        );
        assert!(
            response
                .updated
                .as_ref()
                .is_some_and(|updated| updated.contains_key(id)),
            "the update was applied: {:?}",
            response.not_updated
        );
    }

    fn destroy(&self, id: &Id, schedule: bool) {
        let response = self.set(
            SetRequest::new(self.account_id.clone()).destroy(id.clone()),
            schedule,
        );
        assert!(
            response
                .destroyed
                .as_ref()
                .is_some_and(|destroyed| destroyed.contains(id)),
            "the destroy was applied: {:?}",
            response.not_destroyed
        );
    }

    /// Everything recorded so far, as (method, recipient, recurrenceId).
    fn sent(&self) -> Vec<Sent> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&self.account_id)
            .unwrap()
            .scheduling_outbox
            .iter()
            .map(|message| {
                (
                    message.method.clone(),
                    message.recipient.clone(),
                    message.recurrence_id.clone(),
                )
            })
            .collect()
    }

    /// Everything recorded since `mark`, sorted so an assertion does not
    /// depend on the order the participants happen to be walked in.
    fn sent_since(&self, mark: usize) -> Vec<Sent> {
        let mut messages = self.sent().split_off(mark);
        messages.sort();
        messages
    }
}

#[test]
fn creating_an_event_requests_every_participant_but_this_account_itself() {
    let fixture = Fixture::new();
    fixture.create(&meeting(&fixture.calendar_id), true);

    assert_eq!(
        fixture.sent_since(0),
        vec![
            ("REQUEST".to_owned(), BOB.to_owned(), None),
            ("REQUEST".to_owned(), CAROL.to_owned(), None),
        ],
        "a REQUEST goes to each invitee, never back to this account"
    );
}

#[test]
fn nothing_is_sent_unless_the_client_asks_for_it() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), false);
    fixture.update(&id, json!({"title": "Design review (moved)"}), false);
    fixture.destroy(&id, false);

    assert!(
        fixture.sent().is_empty(),
        "sendSchedulingMessages defaults to false"
    );
}

#[test]
fn a_draft_is_never_scheduled() {
    let fixture = Fixture::new();
    let mut event = meeting(&fixture.calendar_id);
    event.is_draft = Some(true);
    fixture.create(&event, true);

    assert!(fixture.sent().is_empty(), "a draft sends no invitations");
}

#[test]
fn removing_a_participant_cancels_to_them_and_requests_the_rest() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    fixture.update(&id, json!({"participants/carol": null}), true);

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            ("CANCEL".to_owned(), CAROL.to_owned(), None),
            ("REQUEST".to_owned(), BOB.to_owned(), None),
        ],
        "the dropped participant is cancelled, the rest re-requested"
    );
}

#[test]
fn destroying_an_event_cancels_to_every_participant() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    fixture.destroy(&id, true);

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            ("CANCEL".to_owned(), BOB.to_owned(), None),
            ("CANCEL".to_owned(), CAROL.to_owned(), None),
        ]
    );
}

#[test]
fn a_change_to_per_user_properties_alone_sends_nothing() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    fixture.update(
        &id,
        json!({"color": "blue", "freeBusyStatus": "free", "keywords": {"offsite": true}}),
        true,
    );

    assert!(
        fixture.sent_since(mark).is_empty(),
        "section 5.4's per-user properties are nobody else's business"
    );
}

#[test]
fn excluding_an_instance_cancels_just_that_occurrence() {
    let fixture = Fixture::new();
    let mut event = meeting(&fixture.calendar_id);
    event.recurrence_rule = Some(
        serde_json::from_value(json!({
            "@type": "RecurrenceRule",
            "frequency": "weekly",
        }))
        .unwrap(),
    );
    let id = fixture.create(&event, true);
    let mark = fixture.sent().len();

    fixture.update(
        &id,
        json!({"recurrenceOverrides/2026-06-08T10:00:00": {"excluded": true}}),
        true,
    );

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            (
                "CANCEL".to_owned(),
                BOB.to_owned(),
                Some("2026-06-08T10:00:00".to_owned())
            ),
            (
                "CANCEL".to_owned(),
                CAROL.to_owned(),
                Some("2026-06-08T10:00:00".to_owned())
            ),
        ],
        "an added exclusion cancels the instance instead of re-requesting the series"
    );
}

#[test]
fn answering_someone_elses_invitation_replies_to_the_organizer() {
    let fixture = Fixture::new();
    let id = fixture.create(&invitation(&fixture.calendar_id), true);
    assert!(
        fixture.sent().is_empty(),
        "needs-action on a foreign event is not an answer worth sending"
    );

    fixture.update(
        &id,
        json!({"participants/alice/participationStatus": "accepted"}),
        true,
    );

    assert_eq!(
        fixture.sent_since(0),
        vec![("REPLY".to_owned(), BOB.to_owned(), None)],
        "this account is not the origin, so it answers the organizer"
    );
}

#[test]
fn updating_to_add_an_unreachable_participant_is_refused() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    let response = fixture.set(
        SetRequest::new(fixture.account_id.clone()).update(
            id.clone(),
            json!({"participants/dave": {"@type": "Participant", "name": "Dave"}}),
        ),
        true,
    );

    let error = response
        .not_updated
        .as_ref()
        .and_then(|map| map.get(&id))
        .expect("the update was refused");
    assert_eq!(error.error_type, "noSupportedScheduleMethods");
    assert!(
        fixture.sent_since(mark).is_empty(),
        "a refused update sends no messages"
    );
}

#[test]
fn destroying_an_event_with_an_unreachable_participant_is_refused() {
    let fixture = Fixture::new();
    let mut event = meeting(&fixture.calendar_id);
    event.participants.as_mut().unwrap().insert(
        "dave".to_owned(),
        json!({"@type": "Participant", "name": "Dave"}),
    );
    // Unscheduled create: the reachability check only runs when a call
    // actually asks for scheduling messages.
    let id = fixture.create(&event, false);

    let response = fixture.set(
        SetRequest::new(fixture.account_id.clone()).destroy(id.clone()),
        true,
    );

    let error = response
        .not_destroyed
        .as_ref()
        .and_then(|map| map.get(&id))
        .expect("the destroy was refused");
    assert_eq!(error.error_type, "noSupportedScheduleMethods");
    assert!(fixture.sent().is_empty(), "a refused destroy sends nothing");
}

#[test]
fn an_update_that_drops_the_version_is_refused() {
    let fixture = Fixture::new();
    let id = fixture.create(&meeting(&fixture.calendar_id), false);

    let response = fixture.set(
        SetRequest::new(fixture.account_id.clone()).update(id.clone(), json!({"version": "1.0"})),
        false,
    );

    let error = response
        .not_updated
        .as_ref()
        .and_then(|map| map.get(&id))
        .expect("the update was refused");
    assert_eq!(error.error_type, "invalidProperties");
    assert_eq!(
        error.properties.as_deref(),
        Some(["version".to_owned()].as_slice())
    );
}

#[test]
fn a_participant_with_no_calendar_address_is_refused() {
    let fixture = Fixture::new();
    let mut event = meeting(&fixture.calendar_id);
    event.participants.as_mut().unwrap().insert(
        "dave".to_owned(),
        json!({"@type": "Participant", "name": "Dave"}),
    );

    let response = fixture.set(
        SetRequest::new(fixture.account_id.clone()).create("new", event),
        true,
    );

    let error = response
        .not_created
        .as_ref()
        .and_then(|map| map.get("new"))
        .expect("the create was refused");
    assert_eq!(error.error_type, "noSupportedScheduleMethods");
    assert!(
        fixture.sent().is_empty(),
        "a refused create sends no invitations"
    );
}
