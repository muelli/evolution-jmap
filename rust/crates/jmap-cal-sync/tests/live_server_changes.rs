// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::get_changes` against a real JMAP server. This is the
//! state-token delta path of the `get_changes_sync` vfunc, which EDS uses for
//! incremental sync. A full `list_existing` download is the fallback, not the
//! usual case.
//!
//! `jmap-cal-sync/tests/live_server.rs`/`live_server_save.rs` prove the
//! `free_busy`/`save_component`/`remove_component` round trips against a real
//! Stalwart server. But those tests do not call `get_changes`: all assertions
//! use `list_existing`/`load_component`. `jmap-cal-sync/tests/sync.rs` proves
//! the classification logic (`ChangeSet` to `Changes`) against `jmap-mockd`.
//! This file is its live-server counterpart, with the same recipe as
//! `jmap-book-sync/tests/live_server_changes.rs`.
//!
//! ## Running it
//!
//! The environment is the same as for `live_server.rs`. Refer to
//! `docs/manual-test-live-server.md`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_changes -- --ignored
//! ```
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that variable is set,
//! a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.
//! The cross-account test also skips when `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset.
//! When that variable is set, a missing `_RECIPIENT_PASSWORD` causes a panic.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::calendars::Calendar;
use jmap_proto::session::CAPABILITY_CALENDARS;
use serde_json::json;

/// Returns a value unique to this process, so a leftover event from a different
/// run cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `jmap-book-sync/tests/live_server_changes.rs::connect_for_write`.
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

/// Same as `jmap-book-sync/tests/live_server_changes.rs::connect_recipient`.
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// Creates, edits, and removes an event. After each step, `get_changes` from
/// the previous state must report the step. Uses the `get_changes` assertions
/// of `jmap-cal-sync/tests/sync.rs` and the live-server recipe of
/// `jmap-book-sync/tests/live_server_changes.rs`.
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

/// Same as the test with this name in
/// `jmap-book-sync/tests/live_server_changes.rs`. Per RFC 8620 a state is
/// opaque, but not free-form. A well-formed state from a *different* account
/// reliably causes `cannotCalculateChanges` on the real server. A hand-written
/// incorrect string fails the state-syntax validation of Stalwart first
/// (`invalidArguments`). The `jmap-mockd` simulation, by contrast, gives this
/// error for any unparseable string. `jmap-cal-sync::get_changes` has the same
/// `SyncError::is_cannot_calculate_changes` shape as
/// `jmap-book-sync::get_changes`, so the same method applies here.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_changes_reports_cannot_calculate_changes_for_another_accounts_state() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Some(other) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the write-path test"
        );
        return;
    };

    let owner_account_id = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let owner_calendar_id = owner
        .calendars(&owner_account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");
    let owner_sync = CalSync::new(owner, owner_account_id, owner_calendar_id);
    let (owner_state, _) = owner_sync
        .list_existing()
        .expect("listing the owner's calendar failed against the real server");

    let other_account_id = other
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the recipient account needs the calendars capability");
    let other_calendar_id = other
        .calendars(&other_account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the recipient account needs a default calendar")
        .id
        .expect("the server named the calendar");
    let other_sync = CalSync::new(other, other_account_id, other_calendar_id);

    let error = other_sync
        .get_changes(&owner_state)
        .expect_err("a state from a different account must not be answered as if it were valid");
    assert!(
        error.is_cannot_calculate_changes(),
        "expected cannotCalculateChanges for another account's state, got: {error:?}"
    );
}

/// `CalendarEvent/changes` is account-wide. Thus `CalSync::classify` reports an
/// **updated** event that this calendar does not hold as `removed`, not
/// `changed`: the event moved to a different calendar. The other tests here
/// only edit or destroy an event in one calendar, so `holds()` always gives
/// `Ok(true)`. `an_event_moved_to_another_calendar_is_reported_as_removed` in
/// `jmap-cal-sync/tests/sync.rs` tests this against `jmap-mockd`. This test
/// confirms the real server behaviour that the `removed` branch of `classify`
/// needs. A raw `CalendarEvent/set` update of `calendarIds` to a different
/// calendar must show in `CalendarEvent/changes` as an updated id, not as a
/// destroy. Same as
/// `a_card_moved_to_another_address_book_is_reported_as_removed_against_the_real_server`
/// in `jmap-book-sync/tests/live_server_changes.rs`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_event_moved_to_another_calendar_is_reported_as_removed_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let origin_calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let other_calendar = client
        .calendar_create(
            &account_id,
            &Calendar {
                name: format!("agent-calsync-move-target-{}", unique_suffix()),
                ..Calendar::default()
            },
        )
        .expect("Calendar/set create failed against the real server");
    let other_calendar_id = other_calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    // `CalSync::new` takes ownership of its `Client`, so the raw
    // `event_update`/`event_destroy`/`calendar_destroy` calls use a second connection.
    let sync_client =
        connect_for_write().expect("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD vanished mid-test");
    let sync = CalSync::new(sync_client, account_id.clone(), origin_calendar_id);

    let (state_before_create, _) = sync
        .list_existing()
        .expect("listing the calendar failed before the create");

    let local_uid = format!("agent-calsync-move-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-move-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T140000\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");

    let state_before_move = sync
        .get_changes(&state_before_create)
        .expect("get_changes after the create failed against the real server")
        .new_state;

    let event_id = Id::from(saved.uid.as_str());
    client
        .event_update(
            &account_id,
            &event_id,
            json!({"calendarIds": {other_calendar_id.to_string(): true}}),
        )
        .expect("CalendarEvent/set move failed against the real server");

    let after_move = sync
        .get_changes(&state_before_move)
        .expect("get_changes after the move failed against the real server");
    assert!(
        after_move.removed.contains(&saved.uid),
        "an event moved to another calendar should be reported as removed: {:?}",
        after_move
    );
    assert!(
        !after_move.changed.iter().any(|c| c.uid == saved.uid),
        "an event moved out of this calendar must not also be reported as changed: {:?}",
        after_move
    );

    client
        .event_destroy(&account_id, &event_id)
        .expect("cleanup: CalendarEvent/set destroy failed against the real server");
    client
        .calendar_destroy(&account_id, &other_calendar_id)
        .expect("cleanup: Calendar/set destroy failed against the real server");
}
