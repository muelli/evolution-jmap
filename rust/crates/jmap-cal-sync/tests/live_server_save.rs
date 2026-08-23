// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::save_component`/`remove_component` against a real JMAP server —
//! the sync-layer functions `ECalMetaBackend::save_component_sync`/
//! `remove_component_sync` actually call, exercised end to end for the first
//! time.
//!
//! `jmap-client/tests/live_server.rs` already proves `CalendarEvent/set`
//! round-trips against real Stalwart directly through `Client`, and
//! `jmap-cal-sync/tests/live_server.rs` already proves `CalSync::free_busy`
//! (a read-side decision) against it — but nothing has ever driven
//! `CalSync::save_component`/`remove_component` themselves: the
//! iCalendar-to-`CalendarEvent` mapping (`jmap_ical::ical_to_event`) and the
//! create/update decision `save_component` makes, the calendar-side
//! counterpart of exactly what `jmap-book-sync/tests/live_server.rs` already
//! proved for `BookSync::save_contact`/`remove_contact`. Only `jmap-mockd`
//! has ever exercised this crate's own write functions
//! (`jmap-cal-sync/tests/save.rs`); this file is their live-server
//! counterpart, following the same recipe as `jmap-book-sync`'s.
//!
//! ## Running it
//!
//! Same environment as `jmap-cal-sync/tests/live_server.rs` — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up for
//! `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_save -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like this crate's other
//! live-server file, `#[ignore]` alone already keeps it out of a plain
//! `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository
//! gives an unconfigured environment.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_CALENDARS;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover event can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-book-sync/tests/live_server.rs::connect_for_write` exactly.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Saves a new iCalendar VEVENT via `CalSync::save_component`, confirms it
/// via `list_existing`, edits it (a summary change, mirroring what
/// Evolution's appointment editor sends on a rename), confirms the edit via
/// `load_component`, then removes it via `remove_component` and confirms it
/// is gone.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn saving_then_removing_an_event_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
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

    let sync = CalSync::new(client, account_id, calendar_id);

    let local_uid = format!("agent-calsync-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    assert_ne!(
        saved.uid, local_uid,
        "the locally invented UID must not be sent as the JMAP id"
    );
    assert!(
        saved.icalendar.contains(&summary),
        "the created event should carry the summary we sent: {}",
        saved.icalendar
    );

    let (_, existing) = sync.list_existing().expect("listing the calendar failed");
    assert!(
        existing.iter().any(|event| event.uid == saved.uid),
        "the newly created event should be listed in its calendar"
    );

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    let updated = sync
        .save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    assert_eq!(updated.uid, saved.uid, "an edit must not change the id");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the edit should be visible on reload: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
    let (_, remaining) = sync
        .list_existing()
        .expect("listing the calendar failed after removal");
    assert!(
        !remaining.iter().any(|event| event.uid == saved.uid),
        "the removed event should no longer be listed"
    );
}
