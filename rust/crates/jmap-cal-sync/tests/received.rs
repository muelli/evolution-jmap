// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the scheduling rules for a component that Evolution received in a mail
//! or imported from a file ([`Itip::Received`]). Such a component is data from
//! another person. The server must send nothing in this account's name for it,
//! except the user's own answer to the organizer. The data must also not change
//! a meeting that this account organises.

use jmap_cal_sync::{CalSync, Itip, SyncError};
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;

const OLGA: &str = "mailto:olga@example.com";
const VERA: &str = "mailto:vera@example.com";
const IDA: &str = "mailto:ida@example.com";

const ANSWER: Itip = Itip::Received { reply: true };

struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar_id: Id,
}

impl Fixture {
    /// Returns an account that is Olga.
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let calendar_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_participant_identity("Olga", OLGA, true);
            account.seed_calendar("Work", true)
        };
        Self {
            server,
            account_id,
            calendar_id,
        }
    }

    fn sync(&self) -> CalSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        CalSync::new(client, self.account_id.clone(), self.calendar_id.clone())
            .with_server_scheduling(true)
    }

    /// Returns all messages that the server sent, as sorted (method, recipient) pairs.
    fn sent(&self) -> Vec<(String, String)> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let mut sent: Vec<_> = state
            .account(&self.account_id)
            .unwrap()
            .scheduling_outbox
            .iter()
            .map(|message| (message.method.clone(), message.recipient.clone()))
            .collect();
        sent.sort();
        sent
    }

    /// Returns the only stored event.
    fn stored(&self) -> CalendarEvent {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let events: Vec<_> = state
            .account(&self.account_id)
            .unwrap()
            .calendar_events
            .iter()
            .map(|(_, event)| event.clone())
            .collect();
        assert_eq!(events.len(), 1, "one event is stored: {events:?}");
        events.into_iter().next().unwrap()
    }

    fn count(&self) -> usize {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&self.account_id)
            .unwrap()
            .calendar_events
            .len()
    }
}

/// Returns the meeting `planning@localhost` with an optional organizer and the
/// given attendees, each with its participation status.
fn meeting(organizer: Option<&str>, attendees: &[(&str, &str)]) -> String {
    let organizer = organizer
        .map(|address| format!("ORGANIZER:{address}\r\n"))
        .unwrap_or_default();
    let attendees: String = attendees
        .iter()
        .map(|(address, status)| format!("ATTENDEE;PARTSTAT={status}:{address}\r\n"))
        .collect();
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:planning@localhost\r\n\
         SUMMARY:Planning\r\n\
         DTSTART:20260922T110000Z\r\n\
         DURATION:PT1H\r\n\
         {organizer}\
         {attendees}\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

fn addresses(event: &CalendarEvent) -> Vec<String> {
    let mut addresses: Vec<String> = event
        .participants
        .iter()
        .flatten()
        .filter_map(|(_, participant)| participant.get("calendarAddress")?.as_str())
        .map(str::to_owned)
        .collect();
    addresses.sort();
    addresses
}

#[test]
fn a_received_request_that_names_the_user_as_organizer_is_saved_without_scheduling() {
    let fixture = Fixture::start();
    let forged = meeting(
        Some(OLGA),
        &[
            (OLGA, "ACCEPTED"),
            (VERA, "NEEDS-ACTION"),
            (IDA, "NEEDS-ACTION"),
        ],
    );
    fixture
        .sync()
        .save_component_with(&forged, None, ANSWER)
        .expect("the component is filed");
    assert_eq!(fixture.count(), 1);
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// Without an organizer the server is the origin of the event, so the same rule applies.
#[test]
fn a_received_component_without_an_organizer_is_saved_without_scheduling() {
    let fixture = Fixture::start();
    let forged = meeting(None, &[(VERA, "NEEDS-ACTION"), (IDA, "NEEDS-ACTION")]);
    fixture
        .sync()
        .save_component_with(&forged, None, ANSWER)
        .expect("the component is filed");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// The usual Accept: the server sends the answer to the organizer and to nobody else.
#[test]
fn accepting_an_invitation_from_another_organizer_sends_only_the_reply() {
    let fixture = Fixture::start();
    let invitation = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (IDA, "NEEDS-ACTION")]);
    fixture
        .sync()
        .save_component_with(&invitation, None, ANSWER)
        .expect("the answer is filed");
    assert_eq!(fixture.sent(), vec![("REPLY".to_owned(), VERA.to_owned())]);
}

#[test]
fn accepting_without_an_answer_to_the_organizer_mails_nobody() {
    let fixture = Fixture::start();
    let invitation = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (IDA, "NEEDS-ACTION")]);
    fixture
        .sync()
        .save_component_with(&invitation, None, Itip::Received { reply: false })
        .expect("the answer is filed");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// A received component with the `UID` of a meeting that the user organises is
/// an edit of that meeting. It must not add guests, because the next edit by
/// the user would invite them.
#[test]
fn a_received_component_cannot_add_guests_to_the_users_own_meeting() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let own = meeting(Some(OLGA), &[(OLGA, "ACCEPTED"), (VERA, "NEEDS-ACTION")]);
    sync.save_component(&own, None).expect("Olga's meeting");
    let before = fixture.sent();
    let forged = meeting(
        Some(OLGA),
        &[
            (OLGA, "ACCEPTED"),
            (VERA, "NEEDS-ACTION"),
            (IDA, "NEEDS-ACTION"),
        ],
    );
    let failure = sync
        .save_component_with(&forged, None, Itip::Received { reply: false })
        .expect_err("the received component is refused");
    assert!(matches!(failure, SyncError::OwnMeeting), "{failure:?}");
    assert_eq!(addresses(&fixture.stored()), vec![OLGA, VERA]);
    assert_eq!(fixture.sent(), before);
}

#[test]
fn a_received_component_cannot_change_the_organizer_of_the_users_own_meeting() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let own = meeting(Some(OLGA), &[(OLGA, "ACCEPTED"), (VERA, "NEEDS-ACTION")]);
    sync.save_component(&own, None).expect("Olga's meeting");
    let forged = meeting(Some(IDA), &[(OLGA, "ACCEPTED"), (VERA, "NEEDS-ACTION")]);
    let failure = sync
        .save_component_with(&forged, None, ANSWER)
        .expect_err("the received component is refused");
    assert!(matches!(failure, SyncError::OwnMeeting), "{failure:?}");
    assert_eq!(
        fixture.stored().organizer_calendar_address.as_deref(),
        Some(OLGA)
    );
}

/// For a meeting of another organizer, a received component updates the user's
/// copy, but the organizer stays the one that the server holds.
#[test]
fn a_received_component_keeps_the_organizer_of_a_held_invitation() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let invitation = meeting(Some(VERA), &[(OLGA, "NEEDS-ACTION"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&invitation, None, Itip::Suppress)
        .expect("the delivered invitation");
    let repointed = meeting(Some(OLGA), &[(OLGA, "ACCEPTED"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&repointed, None, ANSWER)
        .expect("the answer is filed");
    let stored = fixture.stored();
    assert_eq!(stored.organizer_calendar_address.as_deref(), Some(VERA));
    assert!(
        fixture.sent().iter().all(|(method, _)| method == "REPLY"),
        "{:?}",
        fixture.sent()
    );
}

/// A received cancellation must not delete a meeting that the user organises,
/// because the server would then cancel it for every guest.
#[test]
fn a_received_cancellation_cannot_delete_the_users_own_meeting() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let own = meeting(Some(OLGA), &[(OLGA, "ACCEPTED"), (VERA, "NEEDS-ACTION")]);
    let saved = sync.save_component(&own, None).expect("Olga's meeting");
    let before = fixture.sent();
    let failure = sync
        .remove_component_with(&saved.uid, ANSWER)
        .expect_err("the received cancellation is refused");
    assert!(matches!(failure, SyncError::OwnMeeting), "{failure:?}");
    assert_eq!(fixture.count(), 1);
    assert_eq!(fixture.sent(), before);
}

#[test]
fn a_received_cancellation_deletes_the_users_copy_of_another_organizers_meeting() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let invitation = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (VERA, "ACCEPTED")]);
    let saved = sync
        .save_component_with(&invitation, None, Itip::Suppress)
        .expect("the delivered invitation");
    sync.remove_component_with(&saved.uid, Itip::Received { reply: false })
        .expect("the cancellation is applied");
    assert_eq!(fixture.count(), 0);
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// An answer to a held invitation goes out as an edit, and only to the organizer.
#[test]
fn accepting_a_held_invitation_sends_only_the_reply() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let invitation = meeting(Some(VERA), &[(OLGA, "NEEDS-ACTION"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&invitation, None, Itip::Suppress)
        .expect("the delivered invitation");
    let accepted = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&accepted, None, ANSWER)
        .expect("the answer is filed");
    assert_eq!(fixture.sent(), vec![("REPLY".to_owned(), VERA.to_owned())]);
}

#[test]
fn accepting_a_held_invitation_without_an_answer_mails_nobody() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let invitation = meeting(Some(VERA), &[(OLGA, "NEEDS-ACTION"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&invitation, None, Itip::Suppress)
        .expect("the delivered invitation");
    let accepted = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (VERA, "ACCEPTED")]);
    sync.save_component_with(&accepted, None, Itip::Received { reply: false })
        .expect("the answer is filed");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// A cancellation with an answer lets the server tell the organizer.
#[test]
fn a_received_cancellation_with_an_answer_tells_only_the_organizer() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let invitation = meeting(Some(VERA), &[(OLGA, "ACCEPTED"), (VERA, "ACCEPTED")]);
    let saved = sync
        .save_component_with(&invitation, None, Itip::Suppress)
        .expect("the delivered invitation");
    sync.remove_component_with(&saved.uid, ANSWER)
        .expect("the cancellation is applied");
    assert_eq!(fixture.count(), 0);
    assert_eq!(fixture.sent(), vec![("REPLY".to_owned(), VERA.to_owned())]);
}
