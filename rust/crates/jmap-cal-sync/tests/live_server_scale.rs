// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `CalSync` against a large calendar.
//!
//! This file asks the same questions of calendars that
//! `jmap-book-sync/tests/live_server_scale.rs` asks of address books: cold
//! listing time and incremental `get_changes` cost for a calendar that is not
//! small. One third of the events recur with an override, so not all events
//! have a single occurrence.
//!
//! The scope of this file is the same as that of the `jmap-book-sync`
//! equivalent. That file found that Stalwart's `Http.rateLimitAuthenticated`
//! (1000 req/60s) stops `ContactCard/set` before 500 calls, also on the default
//! temporary plan. Thus this file starts at 500 events, and [`retrying`]
//! absorbs a 429 in the same way. Larger checkpoints (1,000 and 3,000 events)
//! use this file with a higher `JMAP_SCALE_TEST_EVENTS`. The real-EDS test
//! against a live-Stalwart calendar factory is
//! `jmap-functional/tests/live-stalwart-calendar-scale.rs`.
//!
//! ## Running it
//!
//! The environment is the same as for the other live-server tests of this crate.
//! Refer to `docs/manual-test-live-server.md`. Set
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD`, then run:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_scale -- --ignored --nocapture
//! ```
//!
//! `JMAP_SCALE_TEST_EVENTS` overrides the batch size (default 500). Use a
//! temporary write-test account only for this test (refer to `stw seed` in the
//! manual-test doc). The test does not remove its events: it leaves them in the
//! calendar for inspection.
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_cal_sync::{CalSync, SyncError};
use jmap_client::Error as ClientError;
use jmap_proto::calendars::Calendar;
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_test_support::{connect_for_write, env_count, unique_suffix};

/// Retries `attempt` after a 5 s sleep on a 429
/// (`Http.rateLimitAuthenticated`), with at most 20 attempts. Thus a scale run
/// does not fail when it is faster than the rate-limit window of the server.
/// Returns any other error, or the last 429, to the caller. Same as
/// `jmap-book-sync/tests/live_server_scale.rs::retrying`.
fn retrying<T>(mut attempt: impl FnMut() -> Result<T, SyncError>) -> Result<T, SyncError> {
    for remaining in (0..20).rev() {
        match attempt() {
            Err(SyncError::Client(ClientError::Http { status: 429, .. })) if remaining > 0 => {
                sleep(Duration::from_secs(5));
            }
            other => return other,
        }
    }
    unreachable!("the loop above always returns on its last iteration (remaining == 0)")
}

fn batch_size() -> usize {
    env_count("JMAP_SCALE_TEST_EVENTS", 500)
}

/// Returns the peak resident set size of this process from `/proc` (Linux only,
/// as are all runners).
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
    })
}

/// Whether `year` is a Gregorian leap year.
fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Days in `month` (1-12) of `year`.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!("month is always 1-12"),
    }
}

/// Returns `(year, month, day)` plus `offset` days, with correct month and year rollover.
/// A plain `YYYYMMDD` integer gives incorrect dates after the end of a month.
fn date_plus_days(mut year: u32, mut month: u32, mut day: u32, offset: u32) -> (u32, u32, u32) {
    for _ in 0..offset {
        day += 1;
        if day > days_in_month(year, month) {
            day = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
    }
    (year, month, day)
}

/// Returns one event as iCalendar. Most events are a one-hour `VEVENT`.
/// Each third event is a daily series with one detached, overridden occurrence.
/// Each event starts one day after the previous one (wrapped after 300 days, in 2026),
/// so the events spread across dates, as in a real account.
fn icalendar(run: u128, label: &str, i: usize) -> String {
    let uid = format!("agent-scale-cal-{run}-{label}-{i}@localhost");
    let (year, month, day) = date_plus_days(2026, 1, 1, (i % 300) as u32);
    let base = format!("{year:04}{month:02}{day:02}T130000Z");

    if i.is_multiple_of(3) {
        let (oyear, omonth, oday) = date_plus_days(year, month, day, 1);
        let override_base = format!("{oyear:04}{omonth:02}{oday:02}T130000Z");
        format!(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:{uid}\r\n\
             SUMMARY:agent-scale-{run}-{label}-{i}\r\n\
             DTSTART:{base}\r\n\
             DURATION:PT1H\r\n\
             RRULE:FREQ=DAILY;COUNT=5\r\n\
             END:VEVENT\r\n\
             BEGIN:VEVENT\r\n\
             UID:{uid}\r\n\
             RECURRENCE-ID:{override_base}\r\n\
             DTSTART:{oyear:04}{omonth:02}{oday:02}T153000Z\r\n\
             DURATION:PT1H\r\n\
             SUMMARY:agent-scale-{run}-{label}-{i}-override\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        )
    } else {
        format!(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:{uid}\r\n\
             SUMMARY:agent-scale-{run}-{label}-{i}\r\n\
             DTSTART:{base}\r\n\
             DURATION:PT1H\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        )
    }
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_real_sized_calendar_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let n = batch_size();
    let run = unique_suffix();
    // A dedicated calendar, not the account's default one. The two tests in
    // this file run concurrently under cargo's default test threading;
    // `get_changes` is account-wide before `classify` narrows it to one
    // calendar, so sharing a calendar would let one test's events show up as
    // another test's changes.
    let calendar_id = client
        .calendar_create(
            &account_id,
            &Calendar::new(format!("agent-scale-main-{run}")),
        )
        .expect("Calendar/set create failed against the real server")
        .id
        .expect("the server named the new calendar");

    let sync = CalSync::new(client, account_id, calendar_id);

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    for i in 0..n {
        let saved = retrying(|| sync.save_component(&icalendar(run, "main", i), None))
            .unwrap_or_else(|error| panic!("CalendarEvent/set create #{i} failed: {error}"));
        saved_uids.push(saved.uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} events (a third recurring with an override) in {:?} ({:?}/event)",
        import_elapsed,
        import_elapsed / n as u32
    );

    let listing_start = Instant::now();
    let (state_after_import, listed) = sync
        .list_existing()
        .expect("listing the calendar failed after the bulk import");
    let listing_elapsed = listing_start.elapsed();
    eprintln!(
        "SCALE: cold listing of {} events took {:?}",
        listed.len(),
        listing_elapsed
    );
    for uid in &saved_uids {
        assert!(
            listed.iter().any(|event| &event.uid == uid),
            "every imported event should be in the cold listing"
        );
    }

    let edit_count = 20usize.min(n);
    let to_edit: Vec<String> = saved_uids.iter().take(edit_count).cloned().collect();
    let edit_start = Instant::now();
    for uid in &to_edit {
        let edited = icalendar(run, "main", 999_999).replace("agent-scale", "agent-scale-edited");
        retrying(|| sync.save_component(&edited, Some(uid.as_str())))
            .unwrap_or_else(|error| panic!("CalendarEvent/set update of {uid} failed: {error}"));
    }
    let edit_elapsed = edit_start.elapsed();
    eprintln!(
        "SCALE: edited {edit_count} of {n} events in {:?} ({:?}/event)",
        edit_elapsed,
        edit_elapsed / edit_count as u32
    );

    let changes_start = Instant::now();
    let changes = sync
        .get_changes(&state_after_import)
        .expect("get_changes failed after editing a few events in an already-large calendar");
    let changes_elapsed = changes_start.elapsed();
    eprintln!(
        "SCALE: get_changes after editing {edit_count} of {n} events took {:?}, reported {} changed",
        changes_elapsed,
        changes.changed.len()
    );
    for uid in &to_edit {
        assert!(
            changes.changed.iter().any(|event| &event.uid == uid),
            "every edited event should be reported changed by get_changes"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}

/// Tests deletion from a large calendar, which the import/edit test does not
/// do. Same as
/// `jmap-book-sync/tests/live_server_scale.rs::deleting_from_a_real_sized_address_book_is_reported_correctly`.
/// `CalSync::get_changes` has no `catch_up_limit`/relist concept (unlike
/// `messages_since` of `jmap-mail-sync`). Thus this test confirms the result
/// against a real server, and does not look for a threshold bug.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn deleting_from_a_real_sized_calendar_is_reported_correctly() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the scale test");
        return;
    };

    let account_id = client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let n = batch_size();
    let run = unique_suffix();
    // A dedicated calendar, not the account's default one; see the same note
    // in `a_real_sized_calendar_round_trips_through_the_real_server`.
    let calendar_id = client
        .calendar_create(
            &account_id,
            &Calendar::new(format!("agent-scale-del-{run}")),
        )
        .expect("Calendar/set create failed against the real server")
        .id
        .expect("the server named the new calendar");

    let sync = CalSync::new(client, account_id, calendar_id);

    let import_start = Instant::now();
    let mut saved_uids = Vec::with_capacity(n);
    for i in 0..n {
        let saved = retrying(|| sync.save_component(&icalendar(run, "del", i), None))
            .unwrap_or_else(|error| panic!("CalendarEvent/set create #{i} failed: {error}"));
        saved_uids.push(saved.uid);
    }
    let import_elapsed = import_start.elapsed();
    eprintln!(
        "SCALE: imported {n} events in {:?} ({:?}/event), ahead of the deletion case",
        import_elapsed,
        import_elapsed / n as u32
    );

    let (state_after_import, _listed) = sync
        .list_existing()
        .expect("listing the calendar failed after the bulk import");

    // Deletes at most 300 events, and never more than the batch size.
    let delete_count = n.min(300);
    let to_delete: Vec<String> = saved_uids.into_iter().take(delete_count).collect();

    let delete_start = Instant::now();
    for uid in &to_delete {
        retrying(|| sync.remove_component(uid))
            .unwrap_or_else(|error| panic!("CalendarEvent/set destroy of {uid} failed: {error}"));
    }
    let delete_elapsed = delete_start.elapsed();
    eprintln!(
        "SCALE: deleted {delete_count} of {n} events in {:?} ({:?}/event)",
        delete_elapsed,
        delete_elapsed / delete_count as u32
    );

    let changes_start = Instant::now();
    let changes = sync
        .get_changes(&state_after_import)
        .expect("get_changes failed after deleting from an already-large calendar");
    let changes_elapsed = changes_start.elapsed();
    eprintln!(
        "SCALE: get_changes after deleting {delete_count} from a {n}-event calendar took {:?}, \
         reported {} removed",
        changes_elapsed,
        changes.removed.len()
    );
    assert!(
        changes.changed.is_empty(),
        "nothing but deletions happened, so changed should be empty, got {} rows",
        changes.changed.len()
    );
    for uid in &to_delete {
        assert!(
            changes.removed.contains(uid),
            "every deleted event should be reported removed by get_changes"
        );
    }

    if let Some(kb) = peak_rss_kb() {
        eprintln!("SCALE: peak RSS so far {kb} kB");
    }
}
