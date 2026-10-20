// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::free_busy` end to end against a real JMAP server.
//! `ECalBackendSync::get_free_busy_sync` calls this sync-layer function.
//!
//! `jmap-client/tests/live_server.rs` proves the `Calendar/set` and
//! `CalendarEvent/set` round trips against a real Stalwart server.
//! `jmap-client/examples/principals-availability-probe.rs` confirms that the
//! `Principal/getAvailability` wire field names agree with the types of this crate.
//! Neither one calls `CalSync::free_busy`. That function makes two calls
//! (`Principal/query` by email, then `Principal/getAvailability`) and marshals
//! the result with `jmap_ical::busy_periods_to_vfreebusy`. The meeting-scheduler
//! slot-picker in Evolution runs this code. The mock-server counterpart is
//! `jmap-cal-sync/tests/freebusy.rs` (on `jmap-mockd`). This file uses the same
//! recipe as `jmap-client`.
//!
//! ## Running it
//!
//! Refer to the "free/busy test" section of `docs/manual-test-live-server.md`.
//! Set the `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` variables that
//! the `jmap-client` write-path tests use, then run:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync -- --ignored
//! ```
//!
//! This crate has no `live-server` feature (unlike `jmap-client`).
//! `#[ignore]` alone keeps the test out of a plain `cargo test`.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// Returns a value unique to this process, so a leftover event from a different
/// run cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Returns the client and the address of the write-test account. The address is
/// also the attendee for `free_busy`. The account always has a `Principal` (RFC
/// 9670 `currentUserPrincipalId`), so no second account is necessary. Same as
/// `jmap-client/tests/live_server.rs::connect_for_write`, but also returns the
/// user address.
fn connect_for_write() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the write-test account");
    Some((client, user))
}

/// Creates a one-hour event on the default calendar of the write-test account.
/// Asks `CalSync::free_busy` about the account address and a nonexistent
/// address for the full event day. The answer must name only the account
/// address, with a `FREEBUSY;FBTYPE=BUSY` line for the event window. The
/// nonexistent address must be silently absent, as in
/// `jmap-cal-sync/tests/freebusy.rs::an_address_no_principal_matches_is_silently_absent`.
/// The test then destroys the event.
///
/// `CalendarEvent::simple` sets `timeZone: "Etc/UTC"`, so the event start is
/// anchored, not floating. Thus the `FREEBUSY` digits must agree exactly with
/// the event start and duration, as in the mock-based tests of
/// `jmap-cal-sync/tests/freebusy.rs`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn free_busy_of_the_calendar_owner_reflects_a_real_event_against_the_real_server() {
    let Some((client, address)) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let title = format!("agent-freebusy-{}", unique_suffix());
    let event = CalendarEvent::simple(calendar_id.clone(), &title, "2026-09-20T09:00:00", "PT1H");
    let created = client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create failed against the real server");
    let event_id = created.id.clone().expect("the server named the new event");

    let sync = CalSync::new(client, account_id, calendar_id);
    let stranger = format!("agent-freebusy-nobody-{}@example.invalid", unique_suffix());
    let answers = sync
        .free_busy(
            &[address.clone(), stranger],
            "2026-09-20T00:00:00Z",
            "2026-09-21T00:00:00Z",
        )
        .expect("free_busy failed against the real server");

    let users: Vec<&str> = answers.iter().map(|answer| answer.user.as_str()).collect();
    assert_eq!(
        users,
        vec![address.as_str()],
        "the nonexistent address should be silently absent, not the account's own"
    );
    let ics = &answers[0].icalendar;
    assert!(
        ics.starts_with("BEGIN:VFREEBUSY\r\n"),
        "not a VFREEBUSY component: {ics}"
    );
    assert!(
        ics.contains(&format!("\r\nATTENDEE:mailto:{address}\r\n")),
        "missing the account's own ATTENDEE line: {ics}"
    );
    assert!(
        ics.contains("\r\nFREEBUSY;FBTYPE=BUSY:20260920T090000Z/20260920T100000Z\r\n"),
        "missing the created event's busy period: {ics}"
    );

    sync.client()
        .event_destroy(sync.account_id(), &event_id)
        .expect("CalendarEvent/set destroy failed against the real server");
}
