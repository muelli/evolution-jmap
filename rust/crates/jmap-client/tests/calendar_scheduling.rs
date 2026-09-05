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
use jmap_mock::{MockServer, RecordedSchedulingMessage};
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
        "sendTo": {"imip": address},
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

/// A weekly series Alice organises, with Bob on the whole thing.
fn series(calendar_id: &Id) -> CalendarEvent {
    let mut event = meeting(calendar_id);
    event.participants.as_mut().unwrap().remove("carol");
    event.recurrence_rule = Some(
        serde_json::from_value(json!({
            "@type": "RecurrenceRule",
            "frequency": "weekly",
        }))
        .unwrap(),
    );
    event
}

/// A top-level `update` patch that replaces one recurrence override whole.
///
/// This is the shape the draft's own Figure 6 insists on: an override's keys
/// are themselves pointer paths, so `recurrenceOverrides/<id>/participants/x`
/// would name a property of the override that does not exist. The client has
/// to send the containing object instead.
fn override_patch(recurrence_id: &str, override_: Value) -> Value {
    Value::Object(
        [(format!("recurrenceOverrides/{recurrence_id}"), override_)]
            .into_iter()
            .collect(),
    )
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

    /// The full records since `mark`, `ical` payload included, for tests
    /// that pin the message body rather than just who it went to.
    fn sent_full_since(&self, mark: usize) -> Vec<RecordedSchedulingMessage> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state.account(&self.account_id).unwrap().scheduling_outbox[mark..].to_vec()
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

/// The two occurrences these per-instance tests talk about.
const SECOND: &str = "2026-06-08T10:00:00";
const THIRD: &str = "2026-06-15T10:00:00";

#[test]
fn an_instance_only_participant_is_requested_for_just_that_occurrence() {
    let fixture = Fixture::new();
    let mut event = series(&fixture.calendar_id);
    event.recurrence_overrides = Some(
        [(
            SECOND.to_owned(),
            json!({"participants/carol": participant(CAROL, "needs-action")}),
        )]
        .into(),
    );
    fixture.create(&event, true);

    assert_eq!(
        fixture.sent_since(0),
        vec![
            ("REQUEST".to_owned(), BOB.to_owned(), None),
            (
                "REQUEST".to_owned(),
                CAROL.to_owned(),
                Some(SECOND.to_owned())
            ),
        ],
        "section 5.9.2.1: somebody invited to one occurrence hears about that \
         occurrence alone, not about the series"
    );
}

#[test]
fn adding_a_participant_to_one_instance_requests_only_that_occurrence() {
    let fixture = Fixture::new();
    let id = fixture.create(&series(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    fixture.update(
        &id,
        override_patch(
            SECOND,
            json!({"participants/carol": participant(CAROL, "needs-action")}),
        ),
        true,
    );

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            ("REQUEST".to_owned(), BOB.to_owned(), None),
            (
                "REQUEST".to_owned(),
                CAROL.to_owned(),
                Some(SECOND.to_owned())
            ),
        ],
        "the series changed, so Bob is re-invited to it and Carol to her one \
         occurrence"
    );
}

#[test]
fn removing_a_participant_from_one_instance_cancels_just_that_occurrence() {
    let fixture = Fixture::new();
    let mut event = series(&fixture.calendar_id);
    event
        .participants
        .as_mut()
        .unwrap()
        .insert("carol".to_owned(), participant(CAROL, "needs-action"));
    let id = fixture.create(&event, true);
    let mark = fixture.sent().len();

    fixture.update(
        &id,
        override_patch(SECOND, json!({"participants/carol": null})),
        true,
    );

    assert_eq!(
        fixture.sent_since(mark),
        vec![(
            "CANCEL".to_owned(),
            CAROL.to_owned(),
            Some(SECOND.to_owned())
        )],
        "section 5.9.2.2's first case, per instance: the message goes to the \
         dropped participant alone, and section 5.9.2.1's exception withholds \
         the REQUEST that would contradict it"
    );
}

#[test]
fn excluding_an_instance_reaches_that_instances_own_participants() {
    let fixture = Fixture::new();
    let mut event = series(&fixture.calendar_id);
    let carol_only = json!({"participants/carol": participant(CAROL, "needs-action")});
    event.recurrence_overrides = Some([(SECOND.to_owned(), carol_only.clone())].into());
    let id = fixture.create(&event, true);
    let mark = fixture.sent().len();

    let mut excluded = carol_only;
    excluded["excluded"] = json!(true);
    fixture.update(&id, override_patch(SECOND, excluded), true);

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            ("CANCEL".to_owned(), BOB.to_owned(), Some(SECOND.to_owned())),
            (
                "CANCEL".to_owned(),
                CAROL.to_owned(),
                Some(SECOND.to_owned())
            ),
        ],
        "the occurrence stops happening for whoever was in it, including the \
         participant only it named"
    );
}

#[test]
fn cancelling_one_occurrence_still_invites_somebody_added_to_another() {
    let fixture = Fixture::new();
    let id = fixture.create(&series(&fixture.calendar_id), true);
    let mark = fixture.sent().len();

    let mut patch = override_patch(SECOND, json!({"excluded": true}));
    patch[format!("recurrenceOverrides/{THIRD}")] =
        json!({"participants/carol": participant(CAROL, "needs-action")});
    fixture.update(&id, patch, true);

    assert_eq!(
        fixture.sent_since(mark),
        vec![
            ("CANCEL".to_owned(), BOB.to_owned(), Some(SECOND.to_owned())),
            (
                "REQUEST".to_owned(),
                CAROL.to_owned(),
                Some(THIRD.to_owned())
            ),
        ],
        "section 5.9.2.1's exception withholds the series REQUEST, but it \
         cannot swallow the invitation of somebody newly named by an occurrence"
    );
}

#[test]
fn an_instance_only_participant_with_no_calendar_address_is_refused() {
    let fixture = Fixture::new();
    let mut event = series(&fixture.calendar_id);
    event.recurrence_overrides = Some(
        [(
            SECOND.to_owned(),
            json!({"participants/dave": {"@type": "Participant", "name": "Dave"}}),
        )]
        .into(),
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
        "a recipient with no calendar address is unreachable wherever it is named"
    );
}

#[test]
fn a_status_set_inside_an_override_replies_for_just_that_recurrence_id() {
    let fixture = Fixture::new();
    let mut event = invitation(&fixture.calendar_id);
    event.recurrence_rule = Some(
        serde_json::from_value(json!({
            "@type": "RecurrenceRule",
            "frequency": "weekly",
        }))
        .unwrap(),
    );
    let id = fixture.create(&event, true);
    assert!(
        fixture.sent().is_empty(),
        "needs-action throughout is not an answer worth sending"
    );

    fixture.update(
        &id,
        override_patch(
            SECOND,
            json!({"participants/alice/participationStatus": "declined"}),
        ),
        true,
    );

    assert_eq!(
        fixture.sent_since(0),
        vec![("REPLY".to_owned(), BOB.to_owned(), Some(SECOND.to_owned()))],
        "section 5.9.2.3: an answer given for one occurrence answers for that \
         occurrence alone"
    );
}

#[test]
fn a_status_pinned_by_an_override_does_not_answer_twice() {
    let fixture = Fixture::new();
    let mut event = invitation(&fixture.calendar_id);
    event.recurrence_rule = Some(
        serde_json::from_value(json!({
            "@type": "RecurrenceRule",
            "frequency": "weekly",
        }))
        .unwrap(),
    );
    event.recurrence_overrides = Some(
        [(
            SECOND.to_owned(),
            json!({"participants/alice/participationStatus": "accepted"}),
        )]
        .into(),
    );
    let id = fixture.create(&event, true);
    assert_eq!(
        fixture.sent_since(0),
        vec![("REPLY".to_owned(), BOB.to_owned(), Some(SECOND.to_owned()))],
        "the created event already answers for that one occurrence"
    );
    let mark = fixture.sent().len();

    fixture.update(
        &id,
        json!({"participants/alice/participationStatus": "accepted"}),
        true,
    );

    assert_eq!(
        fixture.sent_since(mark),
        vec![("REPLY".to_owned(), BOB.to_owned(), None)],
        "the series is answered once; the occurrence pinning the same status \
         did not change, so it says nothing again"
    );
}

/// The `ical` payload itself (draft-ietf-jmap-calendars-28 §5.9.2, RFC 5546
/// §3.2), not just the decision to send.
mod payload {
    use super::*;

    fn owner_participant(address: &str) -> Value {
        json!({
            "@type": "Participant",
            "sendTo": {"imip": address},
            "participationStatus": "accepted",
            "roles": {"owner": true},
        })
    }

    #[test]
    fn creating_a_simple_event_produces_a_full_request_body() {
        let fixture = Fixture::new();
        let mut event = meeting(&fixture.calendar_id);
        event.participants = Some(
            [
                ("alice".to_owned(), owner_participant(ALICE)),
                ("bob".to_owned(), participant(BOB, "needs-action")),
            ]
            .into(),
        );

        fixture.create(&event, true);

        let sent = fixture.sent_full_since(0);
        assert_eq!(sent.len(), 1, "one REQUEST, to Bob alone: {sent:?}");
        assert_eq!(sent[0].method, "REQUEST");
        assert_eq!(sent[0].recipient, BOB);
        assert_eq!(sent[0].recurrence_id, None);
        assert_eq!(
            sent[0].ical,
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
             METHOD:REQUEST\r\n\
             BEGIN:VEVENT\r\n\
             UID:CE1\r\n\
             X-JMAP-UID:urn:example:event:CE1\r\n\
             SUMMARY:Design review\r\n\
             DTSTART:20260601T100000\r\n\
             ORGANIZER:mailto:alice@example.com\r\n\
             ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION:mailto:bob@example.net\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
    }

    #[test]
    fn destroying_an_event_produces_a_cancel_body() {
        let fixture = Fixture::new();
        let mut event = meeting(&fixture.calendar_id);
        event.participants = Some(
            [
                ("alice".to_owned(), owner_participant(ALICE)),
                ("bob".to_owned(), participant(BOB, "accepted")),
            ]
            .into(),
        );
        let id = fixture.create(&event, false);

        fixture.destroy(&id, true);

        let sent = fixture.sent_full_since(0);
        assert_eq!(sent.len(), 1, "one CANCEL, to Bob alone: {sent:?}");
        assert_eq!(sent[0].method, "CANCEL");
        assert_eq!(sent[0].recipient, BOB);
        assert_eq!(sent[0].recurrence_id, None);
        assert_eq!(
            sent[0].ical,
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
             METHOD:CANCEL\r\n\
             BEGIN:VEVENT\r\n\
             UID:CE1\r\n\
             X-JMAP-UID:urn:example:event:CE1\r\n\
             SUMMARY:Design review\r\n\
             DTSTART:20260601T100000\r\n\
             ORGANIZER:mailto:alice@example.com\r\n\
             ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:bob@example.net\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
    }

    #[test]
    fn an_instance_only_participant_s_request_body_is_scoped_to_that_occurrence() {
        let fixture = Fixture::new();
        let mut event = series(&fixture.calendar_id);
        event.recurrence_overrides = Some(
            [(
                SECOND.to_owned(),
                json!({"participants/carol": participant(CAROL, "needs-action")}),
            )]
            .into(),
        );

        fixture.create(&event, true);

        let sent = fixture.sent_full_since(0);
        let to_carol = sent
            .iter()
            .find(|message| message.recipient == CAROL)
            .expect("Carol was sent something");
        assert_eq!(to_carol.recurrence_id.as_deref(), Some(SECOND));
        // RFC 5545 §3.1 folds a physical line over 75 octets, which an
        // address this long may cross; undo that before checking substrings.
        let ical = to_carol.ical.replace("\r\n ", "");
        assert!(
            ical.contains("RECURRENCE-ID"),
            "a per-instance message names which occurrence it is about: {ical}"
        );
        assert!(
            ical.contains("mailto:carol@example.org"),
            "and carries Carol's own invitation: {ical}"
        );
        assert!(
            ical.contains("mailto:bob@example.net"),
            "the occurrence's participant set is the series' plus Carol, not \
             Carol alone: {ical}"
        );
    }
}
