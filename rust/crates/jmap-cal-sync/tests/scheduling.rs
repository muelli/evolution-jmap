// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Who tells the guests. With the account's "server handles meeting
//! invitations" switch on, every create, edit and delete asks the server to
//! send the iTIP messages itself (`sendSchedulingMessages`,
//! draft-ietf-jmap-calendars §5.9.2), unless Evolution said not to for this
//! one change (`E_CAL_OPERATION_FLAG_DISABLE_ITIP_MESSAGE`). With it off,
//! nothing is ever asked and Evolution mails the guests as before. The mock
//! records what it would have sent in a per-account scheduling outbox.

use jmap_cal_sync::{CalSync, Itip, SyncError, Unscheduled};
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;

const OLGA: &str = "mailto:olga@example.com";
const VERA: &str = "mailto:vera@example.com";
const IDA: &str = "mailto:ida@example.com";

struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::on(MockServer::builder().start())
    }

    /// An account that is Olga, so a meeting she organises has this account
    /// as its origin and the server invites and cancels on her behalf.
    fn on(server: MockServer) -> Self {
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

    fn sync(&self, server_schedules: bool) -> CalSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        CalSync::new(client, self.account_id.clone(), self.calendar_id.clone())
            .with_server_scheduling(server_schedules)
    }

    /// Every message the server sent, as (method, recipient), sorted.
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

    fn stored(&self) -> usize {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&self.account_id)
            .unwrap()
            .calendar_events
            .len()
    }
}

/// A meeting Olga organises, with the guests given.
fn meeting(guests: &[&str]) -> String {
    let mut attendees = format!("ATTENDEE;CN=Olga;PARTSTAT=ACCEPTED:{OLGA}\r\n");
    for guest in guests {
        attendees.push_str(&format!("ATTENDEE;PARTSTAT=NEEDS-ACTION:{guest}\r\n"));
    }
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:planning@localhost\r\n\
         SUMMARY:Planning\r\n\
         DTSTART:20260922T110000Z\r\n\
         DURATION:PT1H\r\n\
         ORGANIZER;CN=Olga:{OLGA}\r\n\
         {attendees}\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

fn request(to: &str) -> (String, String) {
    ("REQUEST".to_owned(), to.to_owned())
}

fn cancel(to: &str) -> (String, String) {
    ("CANCEL".to_owned(), to.to_owned())
}

#[test]
fn with_the_switch_on_creating_a_meeting_has_the_server_invite_the_guests() {
    let fixture = Fixture::start();
    fixture
        .sync(true)
        .save_component(&meeting(&[VERA, IDA]), None)
        .expect("the meeting is created");
    assert_eq!(fixture.sent(), vec![request(IDA), request(VERA)]);
}

#[test]
fn with_the_switch_on_dropping_a_guest_has_the_server_cancel_to_them() {
    let fixture = Fixture::start();
    let sync = fixture.sync(true);
    let saved = sync
        .save_component(&meeting(&[VERA, IDA]), None)
        .expect("the meeting is created");
    let invited = fixture.sent().len();
    sync.save_component(&meeting(&[VERA]), Some(&saved.uid))
        .expect("the edit is saved");
    let sent = fixture.sent();
    assert!(
        sent.contains(&cancel(IDA)),
        "the dropped guest is told: {sent:?}"
    );
    assert!(sent.len() > invited, "the edit sent something: {sent:?}");
}

#[test]
fn with_the_switch_on_deleting_a_meeting_has_the_server_cancel_to_every_guest() {
    let fixture = Fixture::start();
    let sync = fixture.sync(true);
    let saved = sync
        .save_component(&meeting(&[VERA, IDA]), None)
        .expect("the meeting is created");
    sync.remove_component(&saved.uid)
        .expect("the meeting is deleted");
    assert_eq!(
        fixture.sent(),
        vec![cancel(IDA), cancel(VERA), request(IDA), request(VERA)]
    );
}

#[test]
fn with_the_switch_on_a_change_evolution_said_not_to_announce_mails_nobody() {
    let fixture = Fixture::start();
    let sync = fixture.sync(true);
    let saved = sync
        .save_component_with(&meeting(&[VERA, IDA]), None, Itip::Suppress)
        .expect("the meeting is created");
    sync.save_component_with(&meeting(&[VERA]), Some(&saved.uid), Itip::Suppress)
        .expect("the edit is saved");
    sync.remove_component_with(&saved.uid, Itip::Suppress)
        .expect("the meeting is deleted");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

#[test]
fn with_the_switch_off_the_server_is_never_asked_to_mail_anyone() {
    let fixture = Fixture::start();
    let sync = fixture.sync(false);
    let saved = sync
        .save_component(&meeting(&[VERA, IDA]), None)
        .expect("the meeting is created");
    sync.save_component(&meeting(&[VERA]), Some(&saved.uid))
        .expect("the edit is saved");
    sync.remove_component(&saved.uid)
        .expect("the meeting is deleted");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

#[test]
fn the_switch_is_off_unless_turned_on() {
    let fixture = Fixture::start();
    let client = Client::connect(fixture.server.origin(), Credentials::none()).unwrap();
    let sync = CalSync::new(
        client,
        fixture.account_id.clone(),
        fixture.calendar_id.clone(),
    );
    assert!(!sync.server_schedules());
    sync.save_component(&meeting(&[VERA]), None)
        .expect("the meeting is created");
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
    assert!(fixture.sync(true).server_schedules());
}

/// §5.9.2: a guest the server has no way to reach (the mock, like Stalwart,
/// delivers only to `mailto:`) makes it refuse the change,
/// and the refusal reaches the caller as an error rather than a meeting filed
/// with nobody told.
#[test]
fn a_guest_the_server_cannot_reach_fails_the_create_and_nothing_is_stored() {
    let fixture = Fixture::start();
    let unreachable = meeting(&["urn:uuid:5f0c9a4e-0000-4000-8000-000000000001"]);
    let failure = fixture
        .sync(true)
        .save_component(&unreachable, None)
        .expect_err("the server refuses to schedule a guest it cannot reach");
    assert!(
        matches!(
            &failure,
            SyncError::Unscheduled(Unscheduled::Unreachable { addresses })
                if addresses == &["urn:uuid:5f0c9a4e-0000-4000-8000-000000000001"]
        ),
        "the refusal names the guest no mail can reach: {failure:?}"
    );
    assert_eq!(fixture.stored(), 0);
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// An edit that adds such a guest is refused the same way, naming only the
/// guest the server cannot reach, and the stored meeting is left as it was.
#[test]
fn adding_a_guest_the_server_cannot_reach_fails_the_edit_and_names_only_them() {
    let fixture = Fixture::start();
    let sync = fixture.sync(true);
    let stored = sync.save_component(&meeting(&[VERA]), None).unwrap();
    let unreachable = "urn:uuid:5f0c9a4e-0000-4000-8000-000000000002";
    let failure = sync
        .save_component(&meeting(&[VERA, unreachable]), Some(&stored.uid))
        .expect_err("the server refuses to schedule a guest it cannot reach");
    assert_eq!(
        failure_addresses(&failure),
        Some(vec![unreachable.to_owned()]),
        "{failure:?}"
    );
    assert_eq!(fixture.sent(), vec![request(VERA)]);
}

/// A delete names nobody: the sync layer does not load the event to remove
/// it, so it has no guest list to name from. The refusal still fails the
/// delete rather than removing the meeting with its guests left expecting it.
#[test]
fn a_refused_delete_is_a_scheduling_refusal_too() {
    let fixture = Fixture::start();
    let unreachable = "urn:uuid:5f0c9a4e-0000-4000-8000-000000000003";
    let stored = fixture
        .sync(false)
        .save_component(&meeting(&[unreachable]), None)
        .unwrap();
    let failure = fixture
        .sync(true)
        .remove_component(&stored.uid)
        .expect_err("the server cannot send the cancellation");
    assert_eq!(failure_addresses(&failure), Some(Vec::new()), "{failure:?}");
    assert_eq!(fixture.stored(), 1, "the meeting is still there");
}

/// A server that does not implement `sendSchedulingMessages` and says so
/// (`invalidArguments`, RFC 8620 section 3.9) fails every change made while
/// the switch is on, as [`Unscheduled::NotImplemented`]: the user has to turn
/// the switch off, and only they can.
#[test]
fn a_server_that_does_not_schedule_fails_each_change_while_the_switch_is_on() {
    let fixture = Fixture::on(MockServer::builder().without_scheduling().start());
    let failure = fixture
        .sync(true)
        .save_component(&meeting(&[VERA]), None)
        .expect_err("the server refuses the argument");
    assert!(
        matches!(failure, SyncError::Unscheduled(Unscheduled::NotImplemented)),
        "{failure:?}"
    );
    assert_eq!(fixture.stored(), 0);

    let stored = fixture
        .sync(false)
        .save_component(&meeting(&[VERA]), None)
        .expect("with the switch off the server is never asked");
    for failure in [
        fixture
            .sync(true)
            .save_component(&meeting(&[VERA, IDA]), Some(&stored.uid))
            .expect_err("an edit is refused too"),
        fixture
            .sync(true)
            .remove_component(&stored.uid)
            .expect_err("and so is a delete"),
    ] {
        assert!(
            matches!(failure, SyncError::Unscheduled(Unscheduled::NotImplemented)),
            "{failure:?}"
        );
    }
    assert_eq!(fixture.stored(), 1);
    assert_eq!(fixture.sent(), Vec::<(String, String)>::new());
}

/// The same server, with the switch off: nothing is asked, so nothing is
/// refused, and the guests are still synced as data for Evolution to mail.
#[test]
fn a_server_that_does_not_schedule_is_fine_with_the_switch_off() {
    let fixture = Fixture::on(MockServer::builder().without_scheduling().start());
    let sync = fixture.sync(false);
    let stored = sync.save_component(&meeting(&[VERA]), None).unwrap();
    assert!(stored.icalendar.contains(VERA), "{}", stored.icalendar);
    sync.save_component(&meeting(&[VERA, IDA]), Some(&stored.uid))
        .unwrap();
    sync.remove_component(&stored.uid).unwrap();
    assert_eq!(fixture.stored(), 0);
}

fn failure_addresses(failure: &SyncError) -> Option<Vec<String>> {
    match failure {
        SyncError::Unscheduled(Unscheduled::Unreachable { addresses }) => Some(addresses.clone()),
        _ => None,
    }
}
