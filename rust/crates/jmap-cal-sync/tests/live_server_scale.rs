// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync` against a calendar of a size nothing else here has ever reached.
//!
//! Every other live-server test in `jmap-cal-sync` uses a single event; item
//! 94(b) asks the same question of calendars that item 94(a) already asked
//! of address books (`jmap-book-sync/tests/live_server_scale.rs`): cold
//! listing time and incremental `get_changes` cost once the calendar is no
//! longer tiny, with a third of the events recurring with an override so the
//! mix is not all single-occurrence events.
//!
//! This file covers the first slice, scoped down the same way
//! `jmap-book-sync`'s equivalent was: that file found Stalwart's
//! `Http.rateLimitAuthenticated` (1000 req/60s) well before 500
//! `ContactCard/set` calls even at the default throwaway plan, so this starts
//! at the same 500-event size rather than assuming the 1,000/3,000-event
//! checkpoints the item names are reachable in one call budget. [`retrying`]
//! absorbs a 429 the same way.
//! The 1,000/3,000-event checkpoints and the real-EDS leg (item 80's
//! live-Stalwart calendar factory) are still open.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server tests — see
//! `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_scale -- --ignored --nocapture
//! ```
//!
//! `JMAP_SCALE_TEST_EVENTS` overrides the batch size (default 500); the
//! write-test account should be a throwaway created for this alone (see
//! `stw seed` in the manual-test doc) since this leaves that many events in
//! its calendar for inspection rather than cleaning up after itself.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset, the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_cal_sync::{CalSync, SyncError};
use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_proto::session::CAPABILITY_CALENDARS;

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) with a short
/// sleep, up to 10 times, so a scale run does not fail just because it moved
/// faster than the server's own rate-limit window. Any other error, or
/// exhausting the retries, is returned as-is for the caller to `unwrap`/panic
/// on. Mirrors `jmap-book-sync/tests/live_server_scale.rs::retrying` exactly.
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

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover events can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write`, minus the
/// account address that file also returns, which this one has no use for.
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

fn batch_size() -> usize {
    env::var("JMAP_SCALE_TEST_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500)
}

/// The resident set size this process has peaked at, read from `/proc`. Linux
/// only, which every runner this test actually runs on is.
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

/// `(year, month, day)` plus `offset` days, rolling over months and years
/// correctly -- unlike treating a date as a plain `YYYYMMDD` integer, which
/// breaks the moment a batch runs past the end of a month.
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

/// One event, as iCalendar: a plain one-hour `VEVENT` for most `i`, and every
/// third one a daily series with one detached, overridden occurrence
/// instead, the same "a third recurring with overrides" mix item 94(b) asks
/// for. Each event starts a day later than the last (wrapped to stay inside
/// 2026) so a calendar of this size is spread out rather than piled on one
/// date, closer to what a real account looks like.
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
    let calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let sync = CalSync::new(client, account_id, calendar_id);
    let n = batch_size();
    let run = unique_suffix();

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

/// Deleting from an already-large calendar, the case the import/edit test
/// above does not reach. Mirrors
/// `jmap-book-sync/tests/live_server_scale.rs::
/// deleting_from_a_real_sized_address_book_is_reported_correctly`: unlike
/// `jmap-mail-sync`'s `messages_since`, `CalSync::get_changes` has no
/// `catch_up_limit`/relist concept to trip either, so this is confirming that
/// reading against a real server rather than hunting a threshold bug.
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
    let calendar_id = client
        .calendars(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let sync = CalSync::new(client, account_id, calendar_id);
    let n = batch_size();
    let run = unique_suffix();

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

    // "A few hundred", the same wording item 94 uses for both siblings,
    // capped at the batch size itself so a small manual run never tries to
    // delete more than exists.
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
