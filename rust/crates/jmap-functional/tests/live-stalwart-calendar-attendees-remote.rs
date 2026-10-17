// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Do attendees on a meeting another client created show up in
//! Evolution?
//!
//! `jmap_ical::event::drawn_participants` reads a `CalendarEvent.participants`
//! JSON map and turns it into ORGANIZER/ATTENDEE lines via `calendar_address`,
//! which prefers JSCalendar-bis `calendarAddress` and falls back to the
//! obsolete RFC 8984 `sendTo` (see that function's own comment). This is the
//! pull side only: `ical_to_event` never *writes* `participants` (confirmed
//! live by part (1), `live-stalwart-calendar-attendees.rs`), but nothing
//! stops another client, or a raw JMAP write, from putting one there for this
//! product to read.
//!
//! This test is the mirror of `live-stalwart-calendar-changes.rs`: a genuine
//! second [`jmap_client::Client`], independent of the EDS-backed [`Session`]
//! under test, creates the event directly on the real server, in the
//! JSCalendar-bis shape `STALWART-RFC-FINDINGS.md` finding 23 confirmed
//! Stalwart actually stores and returns (`calendarAddress`, not `sendTo`).
//! Only then does a fresh `ECalClient` connect -- a brand new meta-backend
//! cache, so its first connect is `list_existing_sync`, which pulls in
//! everything already on the server, including the event this test just
//! seeded, with no explicit refresh needed.
//!
//! The twin client, `functional-cal-attendees-remote-client`
//! (`tests/functional/cal-attendees-remote-client.c`), writes nothing at
//! all: it only connects, lists every object, finds the one with this run's
//! unique summary, and reports whether its ORGANIZER/ATTENDEE lines are
//! there.
//!
//! ## Running it
//!
//! Same environment as `live-stalwart-calendar.rs` -- see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-attendees-remote -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::collections::BTreeMap;
use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;
use serde_json::{Value, json};

const START: &str = "2026-02-21T10:00:00";
const DURATION: &str = "PT1H";
const ORGANIZER_ADDRESS: &str = "mailto:organizer@example.com";
const ATTENDEE_1_ADDRESS: &str = "mailto:attendee1@example.com";
const ATTENDEE_2_ADDRESS: &str = "mailto:attendee2@example.com";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's event for this
/// one's -- mirrors every other live-server test in this crate.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip -- mirrors
/// `live-stalwart-calendar.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// `live-stalwart-calendar.rs`'s own keyfile, unchanged.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart remote attendees test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={user}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// The JSCalendar-bis participant shape `STALWART-RFC-FINDINGS.md` finding 23
/// confirmed Stalwart stores and returns intact: `calendarAddress`, never the
/// reserved `sendTo`. One owner (who is also an attendee, so it gets both an
/// ORGANIZER and an ATTENDEE line per `drawn_participants`' own rule) and two
/// plain attendees, mirroring part (1)'s own organizer-plus-two-attendees
/// shape so the two tests are directly comparable.
fn participants() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "org".to_owned(),
            json!({
                "@type": "Participant",
                "name": "Organizer",
                "calendarAddress": ORGANIZER_ADDRESS,
                "roles": {"owner": true, "attendee": true},
                "participationStatus": "accepted",
            }),
        ),
        (
            "att1".to_owned(),
            json!({
                "@type": "Participant",
                "name": "Attendee One",
                "calendarAddress": ATTENDEE_1_ADDRESS,
                "roles": {"attendee": true},
                "participationStatus": "needs-action",
                "expectReply": true,
            }),
        ),
        (
            "att2".to_owned(),
            json!({
                "@type": "Participant",
                "name": "Attendee Two",
                "calendarAddress": ATTENDEE_2_ADDRESS,
                "roles": {"attendee": true},
                "participationStatus": "needs-action",
                "expectReply": true,
            }),
        ),
    ])
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_fresh_connect_shows_attendees_a_remote_client_put_straight_on_the_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server remote-attendees leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_ATTENDEES_REMOTE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    // A genuine second client, independent of the EDS-backed session under
    // test, writing directly over JMAP -- the same role it plays in
    // `live-stalwart-calendar-changes.rs`.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let raw_client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the raw second client");
    let account_id = raw_client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = raw_client
        .calendars(&account_id)
        .expect("Calendar/get failed for the raw second client")
        .into_iter()
        .find(|calendar| calendar.is_default == Some(true))
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the default calendar");

    let summary = format!(
        "agent-fncal-attendees-remote-{:x}",
        unique_suffix() & 0xffff_ffff_ffff
    );
    let mut event = CalendarEvent::simple(calendar_id, &summary, START, DURATION);
    event.participants = Some(participants());
    let created = raw_client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create failed for the raw second client");
    let event_id = created.id.clone().expect("the server named the event");

    // Only now does the EDS side connect at all: a brand new meta-backend
    // cache, so its first connect is list_existing_sync, which pulls in
    // everything already on the server -- including the event just seeded
    // above -- with no second sync needed.
    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-attendees-remote"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let output = session.run(&client_binary, &["jmap-functional", &summary]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
    assert_eq!(
        readonly, "0",
        "EDS opened the real-server calendar read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let found = seen.get("found").copied().unwrap_or("0");
    assert_eq!(
        found, "1",
        "the fresh connect's list_existing_sync never surfaced the event the \
         raw second client seeded directly on the server\n{report}"
    );

    // The confirmation itself: `drawn_participants`' pull-side
    // `calendarAddress` read turns the server's participants map into an
    // ORGANIZER line and two ATTENDEE lines, despite EDS never having
    // written either itself.
    let organizer = seen
        .get("read-back-organizer")
        .unwrap_or_else(|| panic!("no 'read-back-organizer' observation\n{report}"));
    assert_eq!(
        *organizer, ORGANIZER_ADDRESS,
        "the pull-side calendarAddress read should have produced an ORGANIZER \
         line for the event's owner\n{report}"
    );

    let attendees = seen
        .get("read-back-attendees")
        .unwrap_or_else(|| panic!("no 'read-back-attendees' observation\n{report}"));
    for address in [ORGANIZER_ADDRESS, ATTENDEE_1_ADDRESS, ATTENDEE_2_ADDRESS] {
        assert!(
            attendees.contains(address),
            "expected {address} among the ATTENDEE lines, got {attendees:?}\n{report}"
        );
    }

    raw_client
        .event_destroy(&account_id, &event_id)
        .expect("CalendarEvent/set destroy failed for the raw second client");
}
