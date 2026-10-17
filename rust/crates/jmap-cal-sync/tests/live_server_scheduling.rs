// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync`'s scheduling switch against a real JMAP server: with it on, a
//! meeting saved through the sync layer is delivered by the server to a guest
//! on the same server, exactly once, and its deletion reaches the guest as a
//! cancellation; with it off, the guest hears nothing. The mock-side
//! counterpart is `tests/scheduling.rs`.
//!
//! ## Running it
//!
//! Needs two throwaway accounts on the same server, so delivery stays
//! intra-server (see `docs/manual-test-live-server.md`, steps 3 and 3a): the
//! write-test account organises, the recipient account is the guest.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_scheduling -- --ignored
//! ```
//!
//! Skipped, not failed, when either account is not configured.

use std::env;
use std::time::{Duration, Instant};

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::session::CAPABILITY_CALENDARS;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn connect(user_var: &str, password_var: &str) -> Option<(Client, String)> {
    let user = env::var(user_var).ok()?;
    let password = env::var(password_var)
        .unwrap_or_else(|_| panic!("{user_var} is set but {password_var} is not"));
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .unwrap_or_else(|_| panic!("set JMAP_LIVE_SERVER_URL alongside {user_var}"));
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document");
    Some((client, user))
}

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

/// The guest's copies of the meeting with this iCalendar `UID`.
fn guest_copies(guest: &Client, account_id: &Id, uid: &str) -> Vec<CalendarEvent> {
    let filter = CalendarEventQueryFilter {
        uid: Some(uid.to_owned()),
        ..CalendarEventQueryFilter::default()
    };
    let ids = guest.event_query(account_id, filter).unwrap().ids;
    guest.event_get(account_id, &ids).unwrap().list
}

/// Polls the guest's account until `done` holds or the deadline passes, and
/// returns the last answer either way.
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
    let Some((organizer, organizer_user)) = connect(
        "JMAP_LIVE_SERVER_WRITE_USER",
        "JMAP_LIVE_SERVER_WRITE_PASSWORD",
    ) else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest, guest_user)) = connect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER",
        "JMAP_LIVE_SERVER_RECIPIENT_PASSWORD",
    ) else {
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
    let Some((organizer, organizer_user)) = connect(
        "JMAP_LIVE_SERVER_WRITE_USER",
        "JMAP_LIVE_SERVER_WRITE_PASSWORD",
    ) else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER not set; skipping");
        return;
    };
    let Some((guest, guest_user)) = connect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER",
        "JMAP_LIVE_SERVER_RECIPIENT_PASSWORD",
    ) else {
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

/// The guest's participation status as the organizer's copy of the meeting
/// records it.
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

/// Polls the organizer's copy until the guest's status is `want` or the
/// deadline passes, and returns the last status seen.
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

/// Both sides of a meeting the organizer has already sent: the organizer's
/// sync and the id of the stored meeting, the guest's account and sync, and the iCalendar `UID` the invitation carries.
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
    let (organizer, organizer_user) = connect(
        "JMAP_LIVE_SERVER_WRITE_USER",
        "JMAP_LIVE_SERVER_WRITE_PASSWORD",
    )?;
    let (guest, guest_user) = connect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER",
        "JMAP_LIVE_SERVER_RECIPIENT_PASSWORD",
    )?;
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
    // What Evolution's event editor saves: the guest's own copy, as this
    // sync rendered it, with the guest's status changed.
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
    // What Evolution's invitation view hands `ECalMetaBackend`'s
    // `receive_objects`: the invitation as mailed, its `UID` the meeting's
    // own, with the guest's status changed. The meta backend looks that `UID`
    // up in its cache, which is keyed by JMAP id, finds nothing, and creates.
    let accepted = meeting(&sent.uid, &sent.organizer_user, &sent.guest_user)
        .replace("PARTSTAT=NEEDS-ACTION", "PARTSTAT=ACCEPTED");
    let saved = sent.guest_sync.save_component(&accepted, None);

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
