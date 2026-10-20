// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A meeting that a local `ECalClient` creates keeps its organizer and
//! attendees, in the EDS copy and on the server.
//!
//! The guests go to the server as jscalendarbis `participants`
//! (`calendarAddress`, never `sendTo`), with `organizerCalendarAddress`.
//! `jmap-cal-sync::CalSync::save_component` reads the event again from the
//! server after a create (see the comment of that function). Thus a dropped
//! `ORGANIZER`/`ATTENDEE` also disappears from the local copy.
//!
//! This test uses a real `ECalClient` against real Stalwart
//! (`functional-cal-attendees-client`,
//! `tests/functional/cal-attendees-client.c`). It also asks the server over
//! JMAP what it stored, so the result does not depend on the code under test.
//!
//! All addresses are on `example.com` (RFC 2606). Thus if the server sends
//! mail to the guests, nobody gets it.
//!
//! The method is the same as in `live-stalwart-calendar.rs`:
//! `spawn_loopback_proxy`, `JMAP_FUNCTIONAL_STORE_PASSWORD` and
//! `JMAP_LIVE_SERVER_REBASE_URLS`.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-calendar.rs` (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-attendees -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEventQueryFilter;
use jmap_proto::session::CAPABILITY_CALENDARS;

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
         DisplayName=JMAP functional live-Stalwart attendees test\n\
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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn attendees_survive_a_local_create_against_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server attendees leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_ATTENDEES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    // A direct connection, not through EDS. It reads the copy that Stalwart
    // stored, not the EDS version of it.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let direct = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the write-test account");
    let account_id = direct
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = direct
        .calendars(&account_id)
        .expect("Calendar/get failed against the real server")
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-attendees"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let summary = format!(
        "agent-fncal-attendees-{:x}",
        unique_suffix() & 0xffff_ffff_ffff
    );
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

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added event\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added an event with no UID\n{report}"
    );

    // The local copy: the EDS read-back after the create shows what the
    // reload of `CalSync::save_component` put in the cache.
    assert_eq!(
        seen.get("read-back-organizer"),
        Some(&"mailto:organizer@example.com"),
        "the event EDS handed back lost its organizer\n{report}"
    );
    let mut attendees: Vec<&str> = seen
        .get("read-back-attendees")
        .map(|joined| joined.split(',').collect())
        .unwrap_or_default();
    attendees.sort_unstable();
    assert_eq!(
        attendees,
        [
            "mailto:attendee1@example.com",
            "mailto:attendee2@example.com"
        ],
        "the event EDS handed back lost its attendees\n{report}"
    );

    // The server copy: ask Stalwart directly what it stored for this event.
    let query = direct
        .event_query(
            &account_id,
            CalendarEventQueryFilter::in_calendar(calendar_id),
        )
        .expect("CalendarEvent/query failed against the real server");
    let events = direct
        .event_get(&account_id, &query.ids)
        .expect("CalendarEvent/get failed against the real server");
    let stored = events
        .list
        .iter()
        .find(|event| event.title.as_deref() == Some(summary.as_str()))
        .unwrap_or_else(|| panic!("the server has no event titled {summary:?}\n{report}"));
    let participants = stored.participants.clone().unwrap_or_default();
    let mut addresses: Vec<&str> = participants
        .values()
        .filter_map(|participant| participant.get("calendarAddress")?.as_str())
        .collect();
    addresses.sort_unstable();
    assert_eq!(
        addresses,
        [
            "mailto:attendee1@example.com",
            "mailto:attendee2@example.com",
            "mailto:organizer@example.com"
        ],
        "the server did not store the guest list the create sent: {participants:?}"
    );
    assert!(
        participants.values().any(|participant| {
            participant.get("calendarAddress").and_then(|a| a.as_str())
                == Some("mailto:organizer@example.com")
                && participant.pointer("/roles/owner") == Some(&serde_json::Value::Bool(true))
        }),
        "the organizer is not the participant holding `owner`: {participants:?}"
    );
    assert_eq!(
        stored.organizer_calendar_address.as_deref(),
        Some("mailto:organizer@example.com")
    );

    direct
        .event_destroy(
            &account_id,
            stored.id.as_ref().expect("the server named the event"),
        )
        .expect("CalendarEvent/set destroy failed against the real server");
}
