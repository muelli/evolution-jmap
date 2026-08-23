// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::get_changes` against a real JMAP server — the
//! `get_changes_sync` vfunc's state-token delta path, which is what EDS
//! actually uses for incremental sync (a full `list_existing` re-download is
//! the fallback, not the common case).
//!
//! `jmap-cal-sync/tests/live_server.rs`/`live_server_save.rs` already prove
//! `free_busy`/`save_component`/`remove_component` round-trip against real
//! Stalwart, but nothing there drives `get_changes` itself — every
//! assertion goes through `list_existing`/`load_component` instead.
//! `jmap-cal-sync/tests/sync.rs` proves the classification logic
//! (`ChangeSet` → `Changes`) against `jmap-mockd`; this file is its
//! live-server counterpart, following the same recipe as
//! `jmap-book-sync/tests/live_server_changes.rs`.
//!
//! ## Running it
//!
//! Same environment as `live_server.rs` — see
//! `docs/manual-test-live-server.md`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_changes -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

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

/// Mirrors `jmap-book-sync/tests/live_server_changes.rs::connect_for_write`
/// exactly.
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

/// Creates an event, then confirms `get_changes` reports it as changed from
/// the state captured just before the create; edits it and confirms the edit
/// shows up from the post-create state; removes it and confirms the removal
/// shows up from the post-edit state. Mirrors
/// `jmap-cal-sync/tests/sync.rs`'s `get_changes` assertions and
/// `jmap-book-sync/tests/live_server_changes.rs`'s live-server recipe.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_changes_reports_a_create_an_edit_and_a_removal_against_the_real_server() {
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

    let (state_before_create, _) = sync
        .list_existing()
        .expect("listing the calendar failed before the create");

    let local_uid = format!("agent-calsync-changes-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-changes-{}", unique_suffix());
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

    let after_create = sync
        .get_changes(&state_before_create)
        .expect("get_changes after the create failed against the real server");
    assert!(
        after_create.changed.iter().any(|c| c.uid == saved.uid),
        "the created event should be reported as changed since before it existed: {:?}",
        after_create
            .changed
            .iter()
            .map(|c| &c.uid)
            .collect::<Vec<_>>()
    );
    assert!(
        !after_create.removed.contains(&saved.uid),
        "a brand-new event must not also be reported as removed"
    );

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");

    let after_edit = sync
        .get_changes(&after_create.new_state)
        .expect("get_changes after the edit failed against the real server");
    let edited = after_edit
        .changed
        .iter()
        .find(|c| c.uid == saved.uid)
        .unwrap_or_else(|| {
            panic!(
                "the edited event should be reported as changed since right after its creation: {:?}",
                after_edit.changed.iter().map(|c| &c.uid).collect::<Vec<_>>()
            )
        });
    assert!(
        edited.icalendar.contains(&new_summary),
        "the changed event get_changes reports should carry the edit: {}",
        edited.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");

    let after_remove = sync
        .get_changes(&after_edit.new_state)
        .expect("get_changes after the removal failed against the real server");
    assert!(
        after_remove.removed.contains(&saved.uid),
        "the removed event should be reported as removed since right after its edit: {:?}",
        after_remove.removed
    );
    assert!(
        !after_remove.changed.iter().any(|c| c.uid == saved.uid),
        "a removed event must not also be reported as changed"
    );
}
