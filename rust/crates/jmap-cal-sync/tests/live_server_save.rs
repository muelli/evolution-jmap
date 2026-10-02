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

/// The first `RRULE` line in an iCalendar document, `None` if there is none.
fn rrule_line(icalendar: &str) -> Option<&str> {
    icalendar.lines().find(|line| line.starts_with("RRULE:"))
}

/// Saves a recurring `VEVENT`, then saves an edit that only changes its
/// summary, and confirms the reloaded event's `RRULE` is exactly what the
/// server returned on create. `patch::diff_recurrence` only ever sends
/// `recurrenceRule` when the baseline (the server's own event, rendered to
/// iCalendar and re-parsed) differs from the edit; the baseline exists
/// specifically so real Stalwart's own `RRULE` spelling is never misread as a
/// user edit and narrowed or dropped. This is the first time that guard has
/// been driven by an actual Stalwart round trip rather than `jmap-mockd`'s
/// modeled `RRULE` strings.
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

/// The `CATEGORIES` line of an iCalendar document, `None` if there is none.
fn categories_line(icalendar: &str) -> Option<&str> {
    icalendar
        .lines()
        .find(|line| line.starts_with("CATEGORIES"))
}

/// Saves a `VEVENT` carrying two tags, one of them holding a comma (so the
/// `CATEGORIES` escaping round-trips too), then saves an edit that only
/// changes the summary, and confirms the reloaded event's `CATEGORIES` line
/// is unchanged. `patch::diff_keywords` only ever sends `keywords` when the
/// baseline (the server's own event, rendered to iCalendar and re-parsed)
/// differs from the edit, the same guard `diff_recurrence`/`diff_overrides`
/// use for `RRULE`/overrides — here so real Stalwart's own `CATEGORIES`
/// spelling is never misread as the user clearing or changing their tags.
/// This is the first time that guard has been driven by an actual Stalwart
/// round trip rather than `jmap-mockd`'s modeled `CATEGORIES` strings.
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

/// Saves a `VEVENT` carrying a single `CATEGORIES` tag, then saves an edit
/// that adds a second tag to the same line, and confirms the reloaded
/// event's `CATEGORIES` carries both. `patch::diff_keywords` sends the whole
/// `keywords` map on any change, never a per-tag patch — the same shape
/// `diff_alerts` uses and already has live confirmation for.
/// `tests/save.rs`'s `adding_a_tag_to_a_tagged_event_sends_the_whole_set`
/// proves this against `jmap-mockd`; this is the first time the positive
/// edit itself, rather than only the guard that leaves an untouched
/// `CATEGORIES` alone (the test above), has been driven by an actual
/// Stalwart round trip.
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

/// The `LOCATION` line of an iCalendar document, `None` if there is none.
fn location_line(icalendar: &str) -> Option<&str> {
    icalendar.lines().find(|line| line.starts_with("LOCATION"))
}

/// Saves a `VEVENT` carrying a single `LOCATION`, then saves an edit that only
/// changes the summary, and confirms the reloaded event's `LOCATION` line is
/// unchanged. `patch::diff_locations` only ever sends `locations/<key>/name`
/// when the baseline (the server's own event, rendered to iCalendar and
/// re-parsed) names a different place than the edit, the same guard
/// `diff_recurrence`/`diff_keywords`/`diff_overrides` use for their own
/// properties — here so real Stalwart's own `LOCATION`/`X-JMAP-KEY` round trip
/// is never misread as the user renaming or clearing the place. This is the
/// first time that guard has been driven by an actual Stalwart round trip
/// rather than `jmap-mockd`'s modeled one
/// (`a_place_that_did_not_change_is_not_sent_at_all` in `tests/save.rs`).
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

/// Saves a `VEVENT` carrying a single `LOCATION`, then saves an edit that
/// renames the place by rewriting the `LOCATION` line itself without its
/// `X-JMAP-KEY` (the way Evolution's appointment editor writes a `LOCATION`
/// it has no UI concept of a key for), and confirms the reloaded event shows
/// the new name on the server's own entry rather than a second, duplicate
/// one. `patch::diff_locations` resolves which entry a keyless rename
/// belongs to by position (there is only ever one place to rename) and
/// patches `locations/<key>/name` under the server's own key, never the
/// property whole — `tests/save.rs`'s
/// `a_place_renamed_without_its_key_still_reaches_the_servers_own_entry`
/// proves this against `jmap-mockd`; this is the first time the positive
/// rename itself, rather than only the guard that leaves an untouched
/// `LOCATION` alone (the test above), has been driven by an actual Stalwart
/// round trip.
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

/// The `CONFERENCE` line of an iCalendar document, `None` if there is none.
fn conference_line(icalendar: &str) -> Option<&str> {
    icalendar
        .lines()
        .find(|line| line.starts_with("CONFERENCE"))
}

/// Saves a `VEVENT` carrying a single `CONFERENCE` (RFC 7986 section 5.11),
/// then saves an edit that only changes the summary, and confirms the
/// reloaded event's `CONFERENCE` line is unchanged. `patch::diff_virtual_locations`
/// only ever sends `virtualLocations/<key>/uri` when the baseline (the
/// server's own event, rendered to iCalendar and re-parsed) names a
/// different place to join online than the edit, the same guard
/// `diff_locations` uses for `LOCATION` — here so real Stalwart's own
/// `CONFERENCE`/`X-JMAP-KEY` round trip is never misread as the user
/// renaming or clearing the place to join online. This is the first time
/// that guard has been driven by an actual Stalwart round trip rather than
/// `jmap-mockd`'s modeled one (`a_place_that_did_not_change_is_not_sent_at_all`
/// in `tests/save.rs`).
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

/// Saves a `VEVENT` carrying a single `CONFERENCE`, then saves an edit that
/// rewrites only the URI inside the same line (keeping whatever
/// `X-JMAP-KEY` the server assigned on create, the way a client that
/// understood the key would edit it), and confirms the reloaded event shows
/// the new address on the server's own entry rather than a second, duplicate
/// one. `patch::diff_virtual_locations` resolves the entry to patch by the
/// server-chosen key and writes `virtualLocations/<key>/uri` alone, leaving
/// any other member untouched — `tests/save.rs`'s
/// `moving_where_an_event_is_joined_online_patches_the_link_in_place` proves
/// this against `jmap-mockd`; this is the first time the positive rename
/// itself, rather than only the guard that leaves an untouched `CONFERENCE`
/// alone (the test above), has been driven by an actual Stalwart round trip.
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

/// Saves a `VEVENT` carrying a single `ATTACH` (an https URI, not a `file:`
/// one — `diff_links` never reads those back), then saves an edit that only
/// changes the summary, and confirms the reloaded event's `ATTACH` line is
/// unchanged. `patch::diff_links` only ever sends `links/<key>/href` when the
/// baseline (the server's own event, rendered to iCalendar and re-parsed)
/// names a different address than the edit, the same guard `diff_locations`
/// and `diff_virtual_locations` use — here so real Stalwart's own
/// `ATTACH`/`X-JMAP-KEY` round trip is never misread as the user rewriting or
/// clearing the resource's address. This is the first time that guard has
/// been driven by an actual Stalwart round trip rather than `jmap-mockd`'s
/// modeled one (`tests/save.rs`'s links coverage).
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

/// Saves a `VEVENT` carrying a single `ATTACH`, then saves an edit that
/// rewrites only the URI inside the same line (keeping whatever
/// `X-JMAP-KEY` the server assigned on create, the way a client that
/// understood the key would edit it), and confirms the reloaded event shows
/// the new address on the server's own entry rather than a second, duplicate
/// one. `patch::diff_links` resolves the entry to patch by the
/// server-chosen key, confirms it via `the_servers_own_entry`'s href match,
/// and writes `links/<key>/href` alone, leaving any other member untouched
/// — `tests/save.rs`'s `moving_an_attachment_patches_the_entry_the_server_
/// chose` proves this against `jmap-mockd`; this is the first time the
/// positive rehref itself, rather than only the guard that leaves an
/// untouched `ATTACH` alone (the test above), has been driven by an actual
/// Stalwart round trip.
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

/// Saves a recurring `VEVENT`, deletes one occurrence (an `EXDATE`), then
/// saves a second edit that only changes the summary, and confirms the
/// `EXDATE` is still present on reload. `patch::diff_overrides` only ever
/// sends `recurrenceOverrides` when the baseline (the server's own event,
/// rendered to iCalendar and re-parsed) differs from the edit, the same
/// guard `diff_recurrence` uses for `RRULE` — here so a real excluded
/// instance is never misread by an unrelated edit and dropped. This is the
/// first time that guard has been driven by an actual Stalwart round trip
/// for overrides rather than `jmap-mockd`'s modeled ones
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

/// The lines of the single `VALARM` block in an iCalendar document: its
/// `UID` and `TRIGGER` lines, or `None` if there is no `VALARM`.
fn valarm_uid_and_trigger(icalendar: &str) -> Option<(&str, &str)> {
    let start = icalendar.find("BEGIN:VALARM")?;
    let end = icalendar[start..].find("END:VALARM")? + start;
    let block = &icalendar[start..end];
    let uid = block.lines().find(|line| line.starts_with("UID:"))?;
    let trigger = block.lines().find(|line| line.starts_with("TRIGGER"))?;
    Some((uid, trigger))
}

/// Saves a `VEVENT` carrying a single reminder (a `VALARM` naming an explicit
/// RFC 9074 `UID`), then saves an edit that only changes the summary, and
/// confirms the reloaded event's `VALARM` is byte-identical. `patch::
/// diff_alerts` only sends the whole `alerts` map when the baseline (the
/// server's own event, rendered to iCalendar and re-parsed) names a
/// different set of reminders than the edit, the same guard the
/// locations/virtualLocations/links families use — here so real Stalwart's
/// own `VALARM`/`UID` round trip is never misread as the user moving or
/// clearing the reminder. `tests/save.rs`'s
/// `a_reminder_that_did_not_change_is_not_sent_at_all` proves this against
/// `jmap-mockd`; this is the first time the guard itself, rather than only
/// the positive edit (the test below), has been driven by an actual
/// Stalwart round trip.
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

/// Saves a `VEVENT` carrying a single reminder (a `VALARM` naming an explicit
/// RFC 9074 `UID`, the way a client that understood the key would write one),
/// then saves an edit that rewrites only the `TRIGGER` of that same `VALARM`,
/// and confirms the reloaded event shows the new offset under the same `UID`
/// rather than a second, duplicate reminder. `patch::diff_alerts` sends the
/// whole `alerts` map, not a per-member patch, but keyed by each `VALARM`'s
/// own `UID` (`jmap_ical::read_alerts`), so an edit under the server's own
/// key must still land on the one entry that already exists, not a new one
/// next to it. `tests/save.rs`'s
/// `moving_a_reminder_sends_the_whole_set_under_the_servers_own_key` proves
/// this against `jmap-mockd`; this is the first time the positive edit
/// itself has been driven by an actual Stalwart round trip.
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
