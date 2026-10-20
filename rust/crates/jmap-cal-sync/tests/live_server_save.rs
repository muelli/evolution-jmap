// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync::save_component`/`remove_component` end to end against a real
//! JMAP server. `ECalMetaBackend::save_component_sync`/`remove_component_sync`
//! call these sync-layer functions.
//!
//! `jmap-client/tests/live_server.rs` proves the `CalendarEvent/set` round
//! trips against a real Stalwart server directly through `Client`.
//! `jmap-cal-sync/tests/live_server.rs` proves `CalSync::free_busy` (a
//! read-side decision) against it. This file tests
//! `CalSync::save_component`/`remove_component`: the
//! iCalendar-to-`CalendarEvent` mapping (`jmap_ical::ical_to_event`) and the
//! create/update decision of `save_component`.
//! `jmap-book-sync/tests/live_server.rs` tests the same for
//! `BookSync::save_contact`/`remove_contact`. The mock-server counterpart is
//! `jmap-cal-sync/tests/save.rs` (on `jmap-mockd`). This file uses the same
//! recipe as `jmap-book-sync`.
//!
//! ## Running it
//!
//! The environment is the same as for `jmap-cal-sync/tests/live_server.rs`.
//! Refer to `docs/manual-test-live-server.md`. Set the
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` variables that the
//! `jmap-client` write-path tests use, then run:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_save -- --ignored
//! ```
//!
//! No `--features live-server` gate is necessary. As for the other live-server
//! files of this crate, `#[ignore]` alone keeps the tests out of a plain
//! `cargo test`.
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::env;

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_proto::session::CAPABILITY_CALENDARS;

/// Returns a value unique to this process, so a leftover event from a different
/// run cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `jmap-book-sync/tests/live_server.rs::connect_for_write`.
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

/// Saves a new iCalendar VEVENT with `CalSync::save_component` and confirms it
/// with `list_existing`. Changes the summary, as the appointment editor of
/// Evolution does on a rename, and confirms with `load_component`. Then removes
/// the event with `remove_component` and confirms that it is gone.
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

/// Returns the first `RRULE` line in an iCalendar document, or `None`.
fn rrule_line(icalendar: &str) -> Option<&str> {
    icalendar.lines().find(|line| line.starts_with("RRULE:"))
}

/// Saves a recurring `VEVENT`, then saves an edit that changes only its
/// summary. The reloaded `RRULE` must be exactly the `RRULE` that the server
/// returned on create. `patch::diff_recurrence` sends `recurrenceRule` only
/// when the baseline differs from the edit. The baseline is the server event,
/// rendered to iCalendar and parsed again. Thus the `RRULE` spelling of
/// Stalwart is not read as a user edit, which can narrow or drop the rule. This
/// test checks that guard with a real Stalwart round trip, not with the `RRULE`
/// strings of `jmap-mockd`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_a_recurring_events_rrule_unchanged() {
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

    let local_uid = format!("agent-calsync-rrule-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-rrule-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let rrule = rrule_line(&saved.icalendar)
        .expect("the created event should carry the RRULE we sent")
        .to_owned();

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert_eq!(
        rrule_line(&reloaded.icalendar),
        Some(rrule.as_str()),
        "an edit that never touched recurrence must not narrow or drop the server's own RRULE: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with an `RRULE`, then saves an edit that adds `COUNT` to
/// the rule. The reloaded `RRULE` must have the new rule, not the old one.
/// `patch::diff_recurrence` sends the full edited `recurrenceRule` when it
/// differs from the baseline. The rule-editing tests of `tests/save.rs` (for
/// example `the_weeks_of_the_year_a_rule_repeats_in_reach_the_server`) prove
/// this against `jmap-mockd`. This test checks the edit itself with a real
/// Stalwart round trip. The test above checks only the guard.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn changing_an_events_recurrence_rule_reaches_the_real_server() {
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

    let local_uid = format!("agent-calsync-rrule-edit-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-rrule-edit-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let old_rrule = rrule_line(&saved.icalendar)
        .expect("the created event should carry the RRULE we sent")
        .to_owned();

    let edited_icalendar = saved
        .icalendar
        .replacen(&old_rrule, &format!("{old_rrule};COUNT=5"), 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert_ne!(
        rrule_line(&reloaded.icalendar),
        Some(old_rrule.as_str()),
        "the rule edit should have reached the server: {}",
        reloaded.icalendar
    );
    assert!(
        rrule_line(&reloaded.icalendar)
            .expect("the edited event should still carry an RRULE")
            .contains("COUNT=5"),
        "the reloaded RRULE should carry the new rule part: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Returns the `CATEGORIES` line of an iCalendar document, or `None`.
fn categories_line(icalendar: &str) -> Option<&str> {
    icalendar
        .lines()
        .find(|line| line.starts_with("CATEGORIES"))
}

/// Saves a `VEVENT` with two tags, and one tag has a comma (this also tests the
/// `CATEGORIES` escaping). Then saves an edit that changes only the summary.
/// The reloaded `CATEGORIES` line must not change. `patch::diff_keywords` sends
/// `keywords` only when the baseline (the server event, rendered to iCalendar
/// and parsed again) differs from the edit. `diff_recurrence`/`diff_overrides`
/// use the same guard for `RRULE`/overrides. Here, the guard prevents a
/// misreading of the `CATEGORIES` spelling of Stalwart as a tag change by the
/// user. This test checks that guard with a real Stalwart round trip, not with
/// the `CATEGORIES` strings of `jmap-mockd`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_an_events_categories_unchanged() {
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

    let local_uid = format!("agent-calsync-categories-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-categories-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         CATEGORIES:Berlin\\, offsite,travel\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let categories = categories_line(&saved.icalendar)
        .expect("the created event should carry the CATEGORIES we sent")
        .to_owned();

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert_eq!(
        categories_line(&reloaded.icalendar),
        Some(categories.as_str()),
        "an edit that never touched tags must not drop or reword the server's own CATEGORIES: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with one `CATEGORIES` tag, then saves an edit that adds a
/// second tag to the same line. The reloaded `CATEGORIES` must have the two
/// tags. On each change, `patch::diff_keywords` sends the full `keywords` map,
/// never a per-tag patch. `diff_alerts` uses the same shape, and live tests
/// confirm it. `adding_a_tag_to_a_tagged_event_sends_the_whole_set` in
/// `tests/save.rs` proves this against `jmap-mockd`. This test checks the edit
/// itself with a real Stalwart round trip. The test above checks only the
/// guard.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn adding_a_tag_to_an_events_categories_reaches_the_real_server() {
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

    let local_uid = format!("agent-calsync-addtag-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-addtag-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         CATEGORIES:offsite\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let categories = categories_line(&saved.icalendar)
        .expect("the created event should carry the CATEGORIES we sent");
    assert!(categories.contains("offsite"), "{categories}");

    let edited_icalendar = saved
        .icalendar
        .replace("CATEGORIES:offsite", "CATEGORIES:offsite,travel");
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    let reloaded_categories = categories_line(&reloaded.icalendar)
        .expect("the edited event should still carry a CATEGORIES line");
    assert!(
        reloaded_categories.contains("offsite") && reloaded_categories.contains("travel"),
        "adding a tag must reach the server alongside the one already there: {reloaded_categories}"
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Returns the `LOCATION` line of an iCalendar document, or `None`.
fn location_line(icalendar: &str) -> Option<&str> {
    icalendar.lines().find(|line| line.starts_with("LOCATION"))
}

/// Saves a `VEVENT` with one `LOCATION`, then saves an edit that changes only
/// the summary. The reloaded `LOCATION` line must not change.
/// `patch::diff_locations` sends `locations/<key>/name` only when the baseline
/// (the server event, rendered to iCalendar and parsed again) names a different
/// place than the edit. `diff_recurrence`/`diff_keywords`/`diff_overrides` use
/// the same guard for their properties. Here, the guard prevents a misreading
/// of the Stalwart `LOCATION`/`X-JMAP-KEY` round trip as a rename or clear.
/// This test checks that guard with a real Stalwart round trip, not with the
/// `jmap-mockd` model (`a_place_that_did_not_change_is_not_sent_at_all` in
/// `tests/save.rs`).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_an_events_location_unchanged() {
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

    let local_uid = format!("agent-calsync-location-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-location-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         LOCATION:Room 42\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let location = location_line(&saved.icalendar)
        .expect("the created event should carry the LOCATION we sent")
        .to_owned();

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert_eq!(
        location_line(&reloaded.icalendar),
        Some(location.as_str()),
        "an edit that never touched the place must not drop or reword the server's own LOCATION: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with one `LOCATION`. Then saves an edit that writes the
/// `LOCATION` line again with a new name and without its `X-JMAP-KEY`, as the
/// appointment editor of Evolution does. (The editor has no UI concept of a
/// key.) The reloaded event must show the new name on the server entry, not on
/// a second, duplicate entry. `patch::diff_locations` uses the key of the one
/// server entry (there is only one place to rename). It patches
/// `locations/<key>/name` under that key, never the full property.
/// `a_place_renamed_without_its_key_still_reaches_the_servers_own_entry` in
/// `tests/save.rs` proves this against `jmap-mockd`. This test checks the
/// rename itself with a real Stalwart round trip. The test above checks only
/// the guard.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn renaming_an_events_location_reaches_the_real_server() {
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

    let local_uid = format!(
        "agent-calsync-location-rename-{}@localhost",
        unique_suffix()
    );
    let summary = format!("agent-calsync-location-rename-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         LOCATION:Room 42\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let location_before = location_line(&saved.icalendar)
        .expect("the created event should carry the LOCATION we sent")
        .to_owned();

    let edited_icalendar = saved
        .icalendar
        .replace(&location_before, "LOCATION:Room 43");
    assert!(
        edited_icalendar.contains("LOCATION:Room 43"),
        "{edited_icalendar}"
    );
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the renamed event failed");

    let location_lines: Vec<&str> = reloaded
        .icalendar
        .lines()
        .filter(|line| line.starts_with("LOCATION"))
        .collect();
    assert_eq!(
        location_lines.len(),
        1,
        "the rename must patch the server's own entry, not create a second one: {}",
        reloaded.icalendar
    );
    assert!(
        location_lines[0].ends_with(":Room 43"),
        "the renamed place must reach the server: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Returns the `CONFERENCE` line of an iCalendar document, or `None`.
fn conference_line(icalendar: &str) -> Option<&str> {
    icalendar
        .lines()
        .find(|line| line.starts_with("CONFERENCE"))
}

/// Saves a `VEVENT` with one `CONFERENCE` (RFC 7986 section 5.11), then saves
/// an edit that changes only the summary. The reloaded `CONFERENCE` line must
/// not change. `patch::diff_virtual_locations` sends
/// `virtualLocations/<key>/uri` only when the baseline (the server event,
/// rendered to iCalendar and parsed again) names a different online place than
/// the edit. `diff_locations` uses the same guard for `LOCATION`. Here, the
/// guard prevents a misreading of the Stalwart `CONFERENCE`/`X-JMAP-KEY` round
/// trip as a rename or clear. This test checks that guard with a real Stalwart
/// round trip, not with the `jmap-mockd` model
/// (`a_place_that_did_not_change_is_not_sent_at_all` in `tests/save.rs`).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_an_events_conference_unchanged() {
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

    let local_uid = format!("agent-calsync-conference-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-conference-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         CONFERENCE;VALUE=URI:https://meet.example.com/planning\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let conference = conference_line(&saved.icalendar)
        .expect("the created event should carry the CONFERENCE we sent")
        .to_owned();

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert_eq!(
        conference_line(&reloaded.icalendar),
        Some(conference.as_str()),
        "an edit that never touched the place to join online must not drop or reword the server's own CONFERENCE: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with one `CONFERENCE`, then saves an edit that changes only
/// the URI in that line. The edit keeps the `X-JMAP-KEY` that the server gave
/// on create, as a client that knows the key does. The reloaded event must show
/// the new address on the server entry, not on a second, duplicate entry.
/// `patch::diff_virtual_locations` finds the entry by the server key and writes
/// only `virtualLocations/<key>/uri`. All other members stay.
/// `moving_where_an_event_is_joined_online_patches_the_link_in_place` in
/// `tests/save.rs` proves this against `jmap-mockd`. This test checks the
/// change itself with a real Stalwart round trip. The test above checks only
/// the guard.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn renaming_an_events_conference_reaches_the_real_server() {
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

    let local_uid = format!(
        "agent-calsync-conference-rename-{}@localhost",
        unique_suffix()
    );
    let summary = format!("agent-calsync-conference-rename-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         CONFERENCE;VALUE=URI:https://meet.example.com/planning\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let conference_before = conference_line(&saved.icalendar)
        .expect("the created event should carry the CONFERENCE we sent")
        .to_owned();

    let conference_after = conference_before.replace(
        "https://meet.example.com/planning",
        "https://meet.example.com/planning-2",
    );
    assert_ne!(
        conference_before, conference_after,
        "the rewrite must actually change the URI: {conference_before}"
    );
    let edited_icalendar = saved
        .icalendar
        .replace(&conference_before, &conference_after);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the renamed event failed");

    let conference_lines: Vec<&str> = reloaded
        .icalendar
        .lines()
        .filter(|line| line.starts_with("CONFERENCE"))
        .collect();
    assert_eq!(
        conference_lines.len(),
        1,
        "the rename must patch the server's own entry, not create a second one: {}",
        reloaded.icalendar
    );
    assert!(
        conference_lines[0].ends_with(":https://meet.example.com/planning-2"),
        "the renamed conference must reach the server: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

fn attach_line(icalendar: &str) -> Option<&str> {
    icalendar.lines().find(|line| line.starts_with("ATTACH"))
}

/// Saves a `VEVENT` with one `ATTACH`, then saves an edit that changes only the
/// summary. The `ATTACH` is an https URI, because `jmap_ical` never reads a
/// `file:` URI. The reloaded `ATTACH` line must not change. `patch::diff_links`
/// sends `links/<key>/href` only when the baseline (the server event, rendered
/// to iCalendar and parsed again) names a different address than the edit.
/// `diff_locations` and `diff_virtual_locations` use the same guard. Here, the
/// guard prevents a misreading of the Stalwart `ATTACH`/`X-JMAP-KEY` round trip
/// as an address change. This test checks that guard with a real Stalwart round
/// trip, not with the `jmap-mockd` model (the links tests in `tests/save.rs`).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_an_events_attach_unchanged() {
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

    let local_uid = format!("agent-calsync-attach-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-attach-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         ATTACH:https://files.example.com/planning.pdf\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let attach = attach_line(&saved.icalendar)
        .expect("the created event should carry the ATTACH we sent")
        .to_owned();

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert_eq!(
        attach_line(&reloaded.icalendar),
        Some(attach.as_str()),
        "an edit that never touched the attachment must not drop or reword the server's own ATTACH: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with one `ATTACH`, then saves an edit that changes only the
/// URI in that line. The edit keeps the `X-JMAP-KEY` that the server gave on
/// create, as a client that knows the key does. The reloaded event must show
/// the new address on the server entry, not on a second, duplicate entry.
/// `patch::diff_links` finds the entry by the server key and confirms it with
/// the href match of `the_servers_own_entry`. Then it writes only
/// `links/<key>/href`. All other members stay.
/// `moving_an_attachment_patches_the_entry_the_server_chose` in `tests/save.rs`
/// proves this against `jmap-mockd`. This test checks the href change itself
/// with a real Stalwart round trip. The test above checks only the guard.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn renaming_an_events_attachment_reaches_the_real_server() {
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

    let local_uid = format!("agent-calsync-attach-rename-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-attach-rename-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         ATTACH:https://files.example.com/planning.pdf\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let attach_before = attach_line(&saved.icalendar)
        .expect("the created event should carry the ATTACH we sent")
        .to_owned();

    let attach_after = attach_before.replace(
        "https://files.example.com/planning.pdf",
        "https://files.example.com/planning-2.pdf",
    );
    assert_ne!(
        attach_before, attach_after,
        "the rewrite must actually change the URI: {attach_before}"
    );
    let edited_icalendar = saved.icalendar.replace(&attach_before, &attach_after);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the renamed event failed");

    let attach_lines: Vec<&str> = reloaded
        .icalendar
        .lines()
        .filter(|line| line.starts_with("ATTACH"))
        .collect();
    assert_eq!(
        attach_lines.len(),
        1,
        "the rehref must patch the server's own entry, not create a second one: {}",
        reloaded.icalendar
    );
    assert!(
        attach_lines[0].ends_with(":https://files.example.com/planning-2.pdf"),
        "the renamed attachment must reach the server: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a recurring `VEVENT` and deletes one occurrence (an `EXDATE`). Then
/// saves a second edit that changes only the summary. The reloaded event must
/// still have the `EXDATE`. `patch::diff_overrides` sends `recurrenceOverrides`
/// only when the baseline (the server event, rendered to iCalendar and parsed
/// again) differs from the edit. `diff_recurrence` uses the same guard for
/// `RRULE`. Here, an unrelated edit must not drop an excluded instance. This
/// test checks that guard for overrides with a real Stalwart round trip, not
/// with the `jmap-mockd` model
/// (`deleting_one_occurrence_reaches_the_server_as_an_excluded_override` in
/// `tests/save.rs`).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_a_deleted_occurrences_exdate_in_place() {
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

    let local_uid = format!("agent-calsync-exdate-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-exdate-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         RRULE:FREQ=DAILY\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");

    let excluded_icalendar = saved.icalendar.replace(
        "END:VEVENT\r\n",
        "EXDATE;TZID=Europe/Berlin:20260923T130000\r\nEND:VEVENT\r\n",
    );
    let after_delete = sync
        .save_component(&excluded_icalendar, Some(&saved.uid))
        .expect(
            "CalendarEvent/set update (deleting one occurrence) failed against the real server",
        );
    assert!(
        after_delete.icalendar.contains("EXDATE"),
        "the deleted occurrence should carry an EXDATE on reload: {}",
        after_delete.icalendar
    );

    let new_summary = format!("{summary}-renamed");
    let unrelated_edit = after_delete.icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&unrelated_edit, Some(&saved.uid))
        .expect("CalendarEvent/set update (unrelated summary edit) failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );
    assert!(
        reloaded.icalendar.contains("EXDATE"),
        "an edit that never touched recurrence overrides must not drop the server's own EXDATE: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Returns the `UID` and `TRIGGER` lines of the one `VALARM` block in an
/// iCalendar document, or `None`.
fn valarm_uid_and_trigger(icalendar: &str) -> Option<(&str, &str)> {
    let start = icalendar.find("BEGIN:VALARM")?;
    let end = icalendar[start..].find("END:VALARM")? + start;
    let block = &icalendar[start..end];
    let uid = block.lines().find(|line| line.starts_with("UID:"))?;
    let trigger = block.lines().find(|line| line.starts_with("TRIGGER"))?;
    Some((uid, trigger))
}

/// Saves a `VEVENT` with one reminder (a `VALARM` with an explicit RFC 9074
/// `UID`). Then saves an edit that changes only the summary. The reloaded
/// `VALARM` must be byte-identical. `patch::diff_alerts` sends the full
/// `alerts` map only when the baseline (the server event, rendered to iCalendar
/// and parsed again) names a different set of reminders than the edit. The
/// locations/virtualLocations/links families use the same guard. Here, the
/// guard prevents a misreading of the Stalwart `VALARM`/`UID` round trip as a
/// reminder change. `a_reminder_that_did_not_change_is_not_sent_at_all` in
/// `tests/save.rs` proves this against `jmap-mockd`. This test checks the guard
/// with a real Stalwart round trip. The test below checks the edit.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_unrelated_edit_leaves_an_events_reminder_unchanged() {
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

    let local_uid = format!("agent-calsync-reminder-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-reminder-{}", unique_suffix());
    let alarm_uid = format!("agent-calsync-reminder-alarm-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         BEGIN:VALARM\r\n\
         UID:{alarm_uid}\r\n\
         ACTION:DISPLAY\r\n\
         TRIGGER:-PT15M\r\n\
         END:VALARM\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let reminder = valarm_uid_and_trigger(&saved.icalendar)
        .expect("the created event should carry the VALARM we sent");
    assert_eq!(
        reminder.0,
        format!("UID:{alarm_uid}"),
        "{}",
        saved.icalendar
    );
    assert_eq!(reminder.1, "TRIGGER:-PT15M", "{}", saved.icalendar);

    let new_summary = format!("{summary}-renamed");
    let edited_icalendar = saved.icalendar.replacen(&summary, &new_summary, 1);
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");
    assert!(
        reloaded.icalendar.contains(&new_summary),
        "the summary edit should be visible on reload: {}",
        reloaded.icalendar
    );

    let valarm_count = reloaded.icalendar.matches("BEGIN:VALARM").count();
    assert_eq!(
        valarm_count, 1,
        "an edit that never touched the reminder must not drop or duplicate the server's own VALARM: {}",
        reloaded.icalendar
    );
    assert_eq!(
        valarm_uid_and_trigger(&reloaded.icalendar),
        Some(reminder),
        "an edit that never touched the reminder must not drop or reword the server's own VALARM: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with one reminder (a `VALARM` with an explicit RFC 9074
/// `UID`, as a client that knows the key writes it). Then saves an edit that
/// changes only the `TRIGGER` of that `VALARM`. The reloaded event must show
/// the new offset under the same `UID`, not a second, duplicate reminder.
/// `patch::diff_alerts` sends the full `alerts` map, not a per-member patch.
/// The map keys are the `UID`s of the `VALARM`s (`jmap_ical::read_alerts`).
/// Thus an edit under the server key must go to the existing entry, not to a
/// new entry. `moving_a_reminder_sends_the_whole_set_under_the_servers_own_key`
/// in `tests/save.rs` proves this against `jmap-mockd`. This test checks the
/// edit with a real Stalwart round trip.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn moving_an_events_reminder_reaches_the_real_server() {
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

    let local_uid = format!("agent-calsync-reminder-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-reminder-{}", unique_suffix());
    let alarm_uid = format!("agent-calsync-reminder-alarm-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         BEGIN:VALARM\r\n\
         UID:{alarm_uid}\r\n\
         ACTION:DISPLAY\r\n\
         TRIGGER:-PT15M\r\n\
         END:VALARM\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let (uid_before, trigger_before) = valarm_uid_and_trigger(&saved.icalendar)
        .expect("the created event should carry the VALARM we sent");
    assert_eq!(
        uid_before,
        format!("UID:{alarm_uid}"),
        "{}",
        saved.icalendar
    );
    assert_eq!(trigger_before, "TRIGGER:-PT15M", "{}", saved.icalendar);

    let edited_icalendar = saved.icalendar.replace("TRIGGER:-PT15M", "TRIGGER:-PT1H");
    sync.save_component(&edited_icalendar, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the edited event failed");

    let valarm_count = reloaded.icalendar.matches("BEGIN:VALARM").count();
    assert_eq!(
        valarm_count, 1,
        "the edit must patch the server's own reminder, not create a second one: {}",
        reloaded.icalendar
    );
    let (uid_after, trigger_after) = valarm_uid_and_trigger(&reloaded.icalendar)
        .expect("the reloaded event should still carry a VALARM");
    assert_eq!(
        uid_after,
        format!("UID:{alarm_uid}"),
        "the edit must keep the server's own key: {}",
        reloaded.icalendar
    );
    assert_eq!(
        trigger_after, "TRIGGER:-PT1H",
        "the new offset must reach the server: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Saves a `VEVENT` with a reminder, then saves the dismissal as EDS writes it.
/// (`e_cal_util_set_alarm_acknowledged` adds an RFC 9074 §6.1 `ACKNOWLEDGED` to
/// the `VALARM`.) The server must store it as the RFC 8984 §4.5.2
/// `acknowledged` under the same key. The reloaded event must show it on the
/// one reminder, at the same instant. The `jmap-mockd` counterpart is
/// `dismissing_a_reminder_reaches_the_server_as_its_acknowledged_instant` in
/// `tests/save.rs`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn dismissing_an_events_reminder_reaches_the_real_server() {
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

    let local_uid = format!("agent-calsync-dismiss-{}@localhost", unique_suffix());
    let summary = format!("agent-calsync-dismiss-{}", unique_suffix());
    let alarm_uid = format!("agent-calsync-dismiss-alarm-{}", unique_suffix());
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{local_uid}\r\n\
         SUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         RRULE:FREQ=WEEKLY\r\n\
         BEGIN:VALARM\r\n\
         UID:{alarm_uid}\r\n\
         ACTION:DISPLAY\r\n\
         TRIGGER:-PT15M\r\n\
         END:VALARM\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    assert!(
        !saved.icalendar.contains("ACKNOWLEDGED"),
        "{}",
        saved.icalendar
    );

    let dismissed = saved.icalendar.replace(
        "TRIGGER:-PT15M\r\n",
        "TRIGGER:-PT15M\r\nACKNOWLEDGED:20260922T104600Z\r\n",
    );
    assert_ne!(dismissed, saved.icalendar, "{}", saved.icalendar);
    sync.save_component(&dismissed, Some(&saved.uid))
        .expect("CalendarEvent/set update failed against the real server");
    let reloaded = sync
        .load_component(&saved.uid)
        .expect("loading the dismissed event failed");

    assert_eq!(
        reloaded.icalendar.matches("BEGIN:VALARM").count(),
        1,
        "the dismissal must land on the server's own reminder: {}",
        reloaded.icalendar
    );
    let (uid_after, trigger_after) = valarm_uid_and_trigger(&reloaded.icalendar)
        .expect("the reloaded event should still carry its VALARM");
    assert_eq!(
        uid_after,
        format!("UID:{alarm_uid}"),
        "{}",
        reloaded.icalendar
    );
    assert_eq!(trigger_after, "TRIGGER:-PT15M", "{}", reloaded.icalendar);
    assert_eq!(
        reloaded
            .icalendar
            .matches("ACKNOWLEDGED:20260922T104600Z\r\n")
            .count(),
        1,
        "the server must keep the dismissal: {}",
        reloaded.icalendar
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}

/// Returns the unfolded `ATTENDEE` line for `address`, or `None`.
fn attendee_line<'a>(icalendar: &'a str, address: &str) -> Option<&'a str> {
    icalendar
        .split("\r\n")
        .find(|line| line.starts_with("ATTENDEE") && line.ends_with(address))
}

/// Edits the guest list of a meeting as the meeting editor of Evolution does:
/// one changed reply, one removed guest, one added guest.
/// Each change must go to the server entry for that address.
/// After the server normalises the roles, the same guest list must give no patch.
/// (In that list, the organizer is also a guest, and the `ROLE` is spelled out.)
///
/// All addresses use a `.test` domain (RFC 2606), so no message from the server
/// can reach a real person.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn editing_a_meetings_guests_reaches_the_real_server() {
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
    let sync = CalSync::new(client, account_id.clone(), calendar_id);

    let suffix = unique_suffix();
    let olga = format!("mailto:olga-{suffix}@agent-guests.test");
    let vera = format!("mailto:vera-{suffix}@agent-guests.test");
    let ida = format!("mailto:ida-{suffix}@agent-guests.test");
    let nils = format!("mailto:nils-{suffix}@agent-guests.test");
    let icalendar = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:agent-guests-{suffix}@localhost\r\n\
         SUMMARY:agent-guests-{suffix}\r\n\
         DTSTART;TZID=Europe/Berlin:20260922T130000\r\n\
         DURATION:PT1H\r\n\
         ORGANIZER;CN=Olga:{olga}\r\n\
         ATTENDEE;CN=Olga;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:{olga}\r\n\
         ATTENDEE;CN=Vera;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION:{vera}\r\n\
         ATTENDEE;CN=Ida;ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION:{ida}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    );
    let saved = sync
        .save_component(&icalendar, None)
        .expect("CalendarEvent/set create failed against the real server");
    let shown = sync
        .load_component(&saved.uid)
        .unwrap()
        .icalendar
        .replace("\r\n ", "");
    eprintln!("as the server drew it:\n{shown}");
    let vera_line = attendee_line(&shown, &vera).expect("Vera was invited");
    let ida_line = attendee_line(&shown, &ida).expect("Ida was invited");

    let edited = shown
        .replace(
            vera_line,
            &vera_line.replace("PARTSTAT=NEEDS-ACTION", "PARTSTAT=ACCEPTED"),
        )
        .replace(&format!("{ida_line}\r\n"), "")
        .replace(
            "END:VEVENT",
            &format!("ATTENDEE;CN=Nils;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:{nils}\r\nEND:VEVENT"),
        );
    sync.save_component(&edited, Some(&saved.uid))
        .expect("CalendarEvent/set update of the guest list failed");

    let held = sync
        .client()
        .event_get(&account_id, &[saved.uid.as_str().into()])
        .unwrap()
        .list
        .remove(0);
    eprintln!(
        "as the server holds it: {}",
        serde_json::to_string_pretty(&held.participants).unwrap()
    );
    let guests = held.participants.clone().expect("the guest list survived");
    let find = |address: &str| {
        guests
            .values()
            .find(|guest| guest["calendarAddress"].as_str() == Some(address))
            .cloned()
    };
    assert_eq!(
        find(&vera).expect("Vera is still invited")["participationStatus"],
        "accepted"
    );
    assert!(find(&ida).is_none(), "Ida was removed: {guests:?}");
    let added = find(&nils).expect("Nils was added");
    assert_eq!(added["name"], "Nils");
    assert_eq!(added["expectReply"], true);
    assert!(find(&olga).is_some(), "the organizer stays: {guests:?}");
    assert_eq!(guests.len(), 3, "{guests:?}");

    // Compares the normalised server shape with the same guest list as
    // Evolution writes it: no patch.
    let reshown = sync
        .load_component(&saved.uid)
        .unwrap()
        .icalendar
        .replace("\r\n ", "");
    let as_evolution_writes_it = reshown.replace(
        "ORGANIZER;",
        &format!("ATTENDEE;CN=Olga;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:{olga}\r\nORGANIZER;"),
    );
    let patch = jmap_cal_sync::patch::diff(
        &held,
        &jmap_ical::ical_to_event(&as_evolution_writes_it).unwrap(),
    );
    assert!(
        patch.is_empty(),
        "the server's normalised guest list would be patched back: {patch:?}\n{reshown}"
    );

    sync.remove_component(&saved.uid)
        .expect("CalendarEvent/set destroy failed against the real server");
}
