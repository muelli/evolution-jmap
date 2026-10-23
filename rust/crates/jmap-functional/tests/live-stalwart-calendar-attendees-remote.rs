// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Attendees of a meeting that another client created show in Evolution.
//!
//! `jmap_ical::event::drawn_participants` changes the
//! `CalendarEvent.participants` JSON map into ORGANIZER/ATTENDEE lines with
//! `calendar_address`. That function prefers the JSCalendar-bis
//! `calendarAddress` and falls back to the obsolete RFC 8984 `sendTo` (see
//! its comment). This test checks the read direction.
//! `live-stalwart-calendar-attendees.rs` checks the write direction.
//!
//! As in `live-stalwart-calendar-changes.rs`, a second
//! [`jmap_client::Client`], independent of the [`Session`] under test,
//! creates the event directly on the real server. It uses the JSCalendar-bis
//! form that Stalwart stores and returns (`calendarAddress`, not `sendTo`).
//! Then a new `ECalClient` connects with an empty meta-backend cache. Its
//! first connect is `list_existing_sync`, which gets all events on the
//! server, with no explicit refresh.
//!
//! The client, `functional-cal-attendees-remote-client`
//! (`tests/functional/cal-attendees-remote-client.c`), writes nothing. It
//! connects, lists all objects, finds the one with the unique summary of
//! this run, and reports its ORGANIZER/ATTENDEE lines.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-calendar.rs` (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-attendees-remote -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

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

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read an event from an earlier run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-calendar.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `live-stalwart-calendar.rs`, with a different
/// `DisplayName=`.
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

/// JSCalendar-bis participants with `calendarAddress`, never the reserved
/// `sendTo`. Stalwart stores and returns this form without change. One owner
/// is also an attendee, thus `drawn_participants` gives it an ORGANIZER and
/// an ATTENDEE line. The two other participants are attendees, as in
/// `live-stalwart-calendar-attendees.rs`.
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

    // A second client that writes directly over JMAP, independent of the
    // session under test.
    // Always rebase, as the backend does behind the loopback proxy.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
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

    // EDS connects only now, with an empty meta-backend cache. Thus its first
    // connect is `list_existing_sync`, which gets the event from above.
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

    // `drawn_participants` reads `calendarAddress` from the participants map
    // of the server and makes an ORGANIZER line and ATTENDEE lines. EDS did
    // not write them.
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
