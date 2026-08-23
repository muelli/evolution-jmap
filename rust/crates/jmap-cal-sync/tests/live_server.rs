// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::free_busy` against a real JMAP server — the sync-layer function
//! `ECalBackendSync::get_free_busy_sync` actually calls, exercised end to end
//! for the first time.
//!
//! `jmap-client/tests/live_server.rs` already proves `Calendar/set` and
//! `CalendarEvent/set` round-trip against real Stalwart, and
//! `jmap-client/examples/principals-availability-probe.rs` already confirmed
//! `Principal/getAvailability`'s wire field names match this crate's types.
//! Neither drives `CalSync::free_busy` itself — the two-call decision
//! (`Principal/query` by email, then `Principal/getAvailability`) plus
//! `jmap_ical::busy_periods_to_vfreebusy`'s marshalling — which is what a
//! real meeting-scheduler slot-picker in Evolution will actually run. Only
//! `jmap-mockd` has ever exercised this function
//! (`jmap-cal-sync/tests/freebusy.rs`); this file is its live-server
//! counterpart, following the same recipe as `jmap-client`'s.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`'s "free/busy test" section. In
//! short, with the same `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/
//! `_WRITE_PASSWORD` already set up for `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — unlike `jmap-client`,
//! this crate has no such feature, and `#[ignore]` alone is what already
//! keeps `jmap-client`'s own live-server tests out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover event can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// The write-test account's client and its own address — the latter doubles
/// as the one attendee this file asks `free_busy` about, since a real
/// server's own account is guaranteed to have a `Principal` of its own (RFC
/// 9670's `currentUserPrincipalId`), with no second throwaway account to
/// provision. Mirrors `jmap-client/tests/live_server.rs::connect_for_write`
/// exactly, plus the address it builds `Credentials::basic` from.
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

/// Creates a one-hour event on the write-test account's default calendar,
/// asks `CalSync::free_busy` about the account's own address over a window
/// spanning the whole event day, confirms the answer names that address and
/// carries a `FREEBUSY;FBTYPE=BUSY` line for exactly the event's window, then
/// asks about a second, nonexistent address in the same call and confirms it
/// is silently absent (mirrors `jmap-cal-sync/tests/
/// freebusy.rs::an_address_no_principal_matches_is_silently_absent`, now
/// against a real server) — then destroys the event.
///
/// `CalendarEvent::simple` sets `timeZone: "Etc/UTC"`, so the event's start
/// is anchored, not floating: the `FREEBUSY` line's digits are expected to
/// match the event's own start/duration exactly, the same bar
/// `jmap-cal-sync/tests/freebusy.rs`'s mock-based tests hold to.
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
