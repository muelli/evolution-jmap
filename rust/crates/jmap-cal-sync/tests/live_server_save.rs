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
