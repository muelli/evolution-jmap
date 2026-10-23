// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the scheduling switch of `CalSync` against a real JMAP server. With
//! the switch on, the server delivers a meeting saved through the sync layer to
//! a guest on the same server, exactly one time. The deletion of the meeting
//! reaches the guest as a cancellation. With the switch off, the guest gets
//! nothing. A dismissed reminder sends nothing to the guest. The mock-server
//! counterpart is `tests/scheduling.rs`.
//!
//! ## Running it
//!
//! The tests need two temporary accounts on the same server, so delivery stays
//! in the server (refer to `docs/manual-test-live-server.md`, steps 3 and 3a).
//! The write-test account is the organizer, and the recipient account is the
//! guest.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_scheduling -- --ignored
//! ```
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset. When a `_USER` variable is set,
//! a missing `_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::time::{Duration, Instant};

use jmap_cal_sync::{CalSync, Itip};
use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_MAIL};
use jmap_test_support::{connect_owner, connect_recipient_with_user, unique_suffix};

fn default_calendar(client: &Client, account_id: &Id) -> Id {
    client
        .calendars(account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the account needs a calendar")
        .id
        .expect("the server named the calendar")
}

/// Returns the copies of the event with this iCalendar `UID` in `account_id`.
fn guest_copies(guest: &Client, account_id: &Id, uid: &str) -> Vec<CalendarEvent> {
    let filter = CalendarEventQueryFilter {
        uid: Some(uid.to_owned()),
        ..CalendarEventQueryFilter::default()
    };
    let ids = guest.event_query(account_id, filter).unwrap().ids;
    guest.event_get(account_id, &ids).unwrap().list
}

/// Polls the guest account until `done` is true or the deadline passes. Returns
/// the last answer.
fn wait_for(
    guest: &Client,
    account_id: &Id,
    uid: &str,
    within: Duration,
    done: impl Fn(&[CalendarEvent]) -> bool,
) -> Vec<CalendarEvent> {
    let deadline = Instant::now() + within;
    loop {
        let copies = guest_copies(guest, account_id, uid);
        if done(&copies) || Instant::now() >= deadline {
            return copies;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn meeting(uid: &str, organizer: &str, guest: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         SUMMARY:{uid}\r\n\
         DTSTART;TZID=Europe/Berlin:20261022T130000\r\n\
         DURATION:PT1H\r\n\
         ORGANIZER;CN=Organizer:mailto:{organizer}\r\n\
         ATTENDEE;CN=Organizer;PARTSTAT=ACCEPTED:mailto:{organizer}\r\n\
         ATTENDEE;CN=Guest;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{guest}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn with_the_switch_on_the_server_delivers_the_meeting_and_its_cancellation_to_the_guest() {
    let Some((organizer, organizer_user)) = connect_owner() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest, guest_user)) = connect_recipient_with_user() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER not set; skipping");
        return;
    };
    let account_id = organizer.primary_account(CAPABILITY_CALENDARS).unwrap();
    let calendar_id = default_calendar(&organizer, &account_id);
    let guest_account = guest.primary_account(CAPABILITY_CALENDARS).unwrap();
    let sync = CalSync::new(organizer, account_id, calendar_id).with_server_scheduling(true);

    let uid = format!("agent-sched-on-{}@localhost", unique_suffix());
    let saved = sync
        .save_component(&meeting(&uid, &organizer_user, &guest_user), None)
        .expect("the real server stores and schedules the meeting");

    let delivered = wait_for(
        &guest,
        &guest_account,
        &uid,
        Duration::from_secs(30),
        |copies| !copies.is_empty(),
    );
    assert_eq!(
        delivered.len(),
        1,
        "the invitation lands in the guest's calendar exactly once: {delivered:?}"
    );

    sync.remove_component(&saved.uid)
        .expect("the real server deletes and schedules the cancellation");
    let after = wait_for(
        &guest,
        &guest_account,
        &uid,
        Duration::from_secs(30),
        |copies| {
            copies
                .iter()
                .all(|copy| copy.status.as_deref() == Some("cancelled"))
        },
    );
    eprintln!("the guest's copy after the cancellation: {after:?}");
    assert!(
        after
            .iter()
            .all(|copy| copy.status.as_deref() == Some("cancelled")),
        "the guest's copy is gone or marked cancelled: {after:?}"
    );
    for copy in after {
        if let Some(id) = copy.id {
            let _ = guest.event_destroy(&guest_account, &id);
        }
    }
}

#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn with_the_switch_off_the_guest_hears_nothing_from_the_server() {
    let Some((organizer, organizer_user)) = connect_owner() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest, guest_user)) = connect_recipient_with_user() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER not set; skipping");
        return;
    };
    let account_id = organizer.primary_account(CAPABILITY_CALENDARS).unwrap();
    let calendar_id = default_calendar(&organizer, &account_id);
    let guest_account = guest.primary_account(CAPABILITY_CALENDARS).unwrap();
    let sync = CalSync::new(organizer, account_id, calendar_id);

    let uid = format!("agent-sched-off-{}@localhost", unique_suffix());
    let saved = sync
        .save_component(&meeting(&uid, &organizer_user, &guest_user), None)
        .expect("the real server stores the meeting");
    let copies = wait_for(
        &guest,
        &guest_account,
        &uid,
        Duration::from_secs(10),
        |copies| !copies.is_empty(),
    );
    sync.remove_component(&saved.uid).unwrap();
    assert!(
        copies.is_empty(),
        "nothing was asked, so nothing reaches the guest: {copies:?}"
    );
}

/// Returns the participation status of the guest in the organizer copy of the meeting.
fn status_at_organizer(
    organizer: &Client,
    account_id: &Id,
    id: &str,
    guest_user: &str,
) -> Option<String> {
    let address = format!("mailto:{guest_user}");
    let event = organizer
        .event_get(account_id, &[Id::from(id)])
        .unwrap()
        .list
        .into_iter()
        .next()?;
    event
        .participants?
        .into_values()
        .find(|participant| {
            participant
                .get("calendarAddress")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(&address))
        })?
        .get("participationStatus")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

/// Polls the organizer copy until the guest status is `want` or the deadline
/// passes. Returns the last status.
fn wait_for_status(
    organizer: &Client,
    account_id: &Id,
    id: &str,
    guest_user: &str,
    want: &str,
) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = status_at_organizer(organizer, account_id, id, guest_user);
        if status.as_deref() == Some(want) || Instant::now() >= deadline {
            return status;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Both sides of a meeting that the organizer sent. Contains the organizer
/// sync, the stored meeting id, the guest account and sync, and the invitation
/// `UID`.
struct Sent {
    organizer: CalSync,
    meeting_id: String,
    guest_account: Id,
    guest_sync: CalSync,
    guest_user: String,
    organizer_user: String,
    uid: String,
}

fn send_meeting(tag: &str) -> Option<Sent> {
    let (organizer, organizer_user) = connect_owner()?;
    let (guest, guest_user) = connect_recipient_with_user()?;
    let account_id = organizer.primary_account(CAPABILITY_CALENDARS).unwrap();
    let calendar_id = default_calendar(&organizer, &account_id);
    let guest_account = guest.primary_account(CAPABILITY_CALENDARS).unwrap();
    let guest_calendar = default_calendar(&guest, &guest_account);
    let organizer = CalSync::new(organizer, account_id, calendar_id).with_server_scheduling(true);

    let uid = format!("agent-{tag}-{}@localhost", unique_suffix());
    let meeting_id = organizer
        .save_component(&meeting(&uid, &organizer_user, &guest_user), None)
        .expect("the real server stores and schedules the meeting")
        .uid;
    let guest_sync =
        CalSync::new(guest, guest_account.clone(), guest_calendar).with_server_scheduling(true);
    let delivered = wait_for(
        guest_sync.client(),
        &guest_account,
        &uid,
        Duration::from_secs(30),
        |copies| !copies.is_empty(),
    );
    assert_eq!(delivered.len(), 1, "the invitation arrives: {delivered:?}");
    Some(Sent {
        organizer,
        meeting_id,
        guest_account,
        guest_sync,
        guest_user,
        organizer_user,
        uid,
    })
}

impl Sent {
    /// Deletes the meeting on both sides, without telling anyone.
    fn clean_up(self) {
        let _ = self
            .organizer
            .remove_component_with(&self.meeting_id, jmap_cal_sync::Itip::Suppress);
        for copy in guest_copies(self.guest_sync.client(), &self.guest_account, &self.uid) {
            if let Some(id) = copy.id {
                let _ = self
                    .guest_sync
                    .client()
                    .event_destroy(&self.guest_account, &id);
            }
        }
    }
}

#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn a_guest_accepting_in_the_calendar_tells_the_organizer() {
    let Some(sent) = send_meeting("accept-cal") else {
        eprintln!("the live accounts are not set; skipping");
        return;
    };
    // The event editor of Evolution saves the guest copy, as this sync rendered
    // it, with a changed guest status.
    let copy = guest_copies(sent.guest_sync.client(), &sent.guest_account, &sent.uid)
        .into_iter()
        .next()
        .and_then(|copy| copy.id)
        .unwrap()
        .to_string();
    let rendered = sent.guest_sync.load_component(&copy).unwrap().icalendar;
    assert!(rendered.contains("PARTSTAT=NEEDS-ACTION"), "{rendered}");
    let accepted = rendered.replace("PARTSTAT=NEEDS-ACTION", "PARTSTAT=ACCEPTED");
    sent.guest_sync
        .save_component(&accepted, Some(&copy))
        .expect("the guest's accept is stored and scheduled");

    let status = wait_for_status(
        sent.organizer.client(),
        sent.organizer.account_id(),
        &sent.meeting_id,
        &sent.guest_user,
        "accepted",
    );
    let copies = guest_copies(sent.guest_sync.client(), &sent.guest_account, &sent.uid);
    sent.clean_up();
    assert_eq!(status.as_deref(), Some("accepted"));
    assert_eq!(copies.len(), 1, "the guest still has one copy: {copies:?}");
}

#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn a_guest_accepting_from_the_invitation_mail_updates_its_one_copy_and_tells_the_organizer() {
    let Some(sent) = send_meeting("accept-mail") else {
        eprintln!("the live accounts are not set; skipping");
        return;
    };
    // The invitation view of Evolution gives the mailed invitation, with a
    // changed guest status, to `ECalMetaBackend` `receive_objects`. Its `UID`
    // is the meeting `UID`. The meta backend cache uses JMAP ids as keys, so
    // the lookup of that `UID` fails, and the backend creates.
    let accepted = meeting(&sent.uid, &sent.organizer_user, &sent.guest_user)
        .replace("PARTSTAT=NEEDS-ACTION", "PARTSTAT=ACCEPTED");
    let saved =
        sent.guest_sync
            .save_component_with(&accepted, None, Itip::Received { reply: true });

    let status = wait_for_status(
        sent.organizer.client(),
        sent.organizer.account_id(),
        &sent.meeting_id,
        &sent.guest_user,
        "accepted",
    );
    let copies = guest_copies(sent.guest_sync.client(), &sent.guest_account, &sent.uid);
    sent.clean_up();
    let saved = saved.expect("the guest's accept is stored and scheduled");
    assert_eq!(copies.len(), 1, "the guest still has one copy: {copies:?}");
    assert_eq!(
        copies[0].id.as_ref().map(ToString::to_string),
        Some(saved.uid),
        "the accept was filed into the copy the server delivered"
    );
    assert_eq!(status.as_deref(), Some("accepted"));
}

/// [`meeting`] with one display reminder.
fn meeting_with_reminder(uid: &str, organizer: &str, guest: &str) -> String {
    meeting(uid, organizer, guest).replace(
        "END:VEVENT\r\n",
        &format!(
            "BEGIN:VALARM\r\n\
             UID:{uid}-alarm\r\n\
             ACTION:DISPLAY\r\n\
             DESCRIPTION:{uid}\r\n\
             TRIGGER:-PT15M\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\n"
        ),
    )
}

/// Returns the subjects of the guest mails that contain `uid`.
fn guest_mail(guest: &Client, uid: &str) -> Vec<String> {
    let account_id = guest.primary_account(CAPABILITY_MAIL).unwrap();
    let filter = EmailQueryFilter {
        text: Some(uid.to_owned()),
        ..EmailQueryFilter::default()
    };
    let ids = guest
        .email_query(&account_id, filter, None, None, 0)
        .unwrap()
        .ids;
    guest
        .email_get(&account_id, &ids, Some(&["subject"][..]))
        .unwrap()
        .into_iter()
        .filter_map(|email| email.subject)
        .collect()
}

/// The test saves a dismissed reminder with [`Itip::Suppress`], as
/// `discard_alarm_sync` of the calendar backend does. The guest gets no second
/// mail.
#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn dismissing_a_meetings_reminder_sends_the_guest_nothing() {
    let Some((organizer, organizer_user)) = connect_owner() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest, guest_user)) = connect_recipient_with_user() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER not set; skipping");
        return;
    };
    let account_id = organizer.primary_account(CAPABILITY_CALENDARS).unwrap();
    let calendar_id = default_calendar(&organizer, &account_id);
    let sync = CalSync::new(organizer, account_id, calendar_id).with_server_scheduling(true);

    let uid = format!("agent-dismiss-{}", unique_suffix());
    let saved = sync
        .save_component(
            &meeting_with_reminder(&uid, &organizer_user, &guest_user),
            None,
        )
        .expect("the real server stores and schedules the meeting");
    let invited = Instant::now() + Duration::from_secs(60);
    let mut mail = guest_mail(&guest, &uid);
    while mail.is_empty() && Instant::now() < invited {
        std::thread::sleep(Duration::from_secs(1));
        mail = guest_mail(&guest, &uid);
    }
    assert_eq!(mail.len(), 1, "the guest is invited exactly once: {mail:?}");

    // As EDS does: saves the server rendering, with the VALARM acknowledged.
    assert!(
        saved.icalendar.contains("END:VALARM"),
        "{}",
        saved.icalendar
    );
    let dismissed = saved
        .icalendar
        .replace("END:VALARM", "ACKNOWLEDGED:20261021T120000Z\r\nEND:VALARM");
    sync.save_component_with(&dismissed, Some(&saved.uid), Itip::Suppress)
        .expect("the real server stores the dismissal");
    let stored = sync
        .client()
        .event_get(sync.account_id(), &[Id::from(saved.uid.as_str())])
        .unwrap()
        .list;
    let acknowledged = stored
        .iter()
        .flat_map(|event| event.alerts.iter().flatten())
        .filter_map(|(_, alert)| alert.get("acknowledged"))
        .count();
    assert_eq!(
        acknowledged, 1,
        "the dismissal reached the server: {stored:?}"
    );
    std::thread::sleep(Duration::from_secs(30));
    let after = guest_mail(&guest, &uid);
    let _ = sync.remove_component_with(&saved.uid, Itip::Suppress);
    assert_eq!(after, mail, "the dismissal mailed the guest");
}

/// A received component that names the user as the organizer is data from
/// another person. The server must not send invitations for it in the user's
/// name. The recipient account is the third address here.
#[test]
#[ignore = "needs a real JMAP server and two accounts; see docs/manual-test-live-server.md"]
fn a_received_request_that_names_the_user_as_organizer_invites_nobody() {
    let Some((victim, victim_user)) = connect_owner() else {
        eprintln!("the live accounts are not set; skipping");
        return;
    };
    let (stranger, stranger_user) =
        connect_recipient_with_user().expect("the recipient account is set");
    let account_id = victim.primary_account(CAPABILITY_CALENDARS).unwrap();
    let calendar_id = default_calendar(&victim, &account_id);
    let stranger_account = stranger.primary_account(CAPABILITY_CALENDARS).unwrap();
    let victim = CalSync::new(victim, account_id, calendar_id).with_server_scheduling(true);

    let uid = format!("agent-received-{}@localhost", unique_suffix());
    let forged = meeting(&uid, &victim_user, &stranger_user);
    let saved = victim
        .save_component_with(&forged, None, Itip::Received { reply: true })
        .expect("the received component is filed");
    let copies = wait_for(
        &stranger,
        &stranger_account,
        &uid,
        Duration::from_secs(15),
        |copies| !copies.is_empty(),
    );
    let mail = guest_mail(&stranger, &uid);
    let _ = victim.remove_component_with(&saved.uid, Itip::Suppress);
    assert!(
        copies.is_empty(),
        "the stranger got the meeting: {copies:?}"
    );
    assert!(mail.is_empty(), "the stranger got mail: {mail:?}");
}
