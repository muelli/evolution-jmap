// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Examines the 429/duplicate-commit RFC-SUSPECT under sustained, sequential
//! bulk-import load. This load shape first showed the problem (the
//! 1,000/3,000-event scale checkpoints of this crate). A prior investigation
//! already excluded the isolated single-429 shape.
//!
//! The suspected mechanism: a sequential create for a new UID gets an HTTP 429
//! (`Http.rateLimitAuthenticated`). The caller retries the same create after a
//! delay, as every write-path test in this repository does on a 429. The retry
//! then answers `already exists`, although this is the first create for the
//! UID. An isolated reproduction with one forced 429 did not show this. A
//! 90-second poll with no retry found nothing, and a delayed retry always
//! created cleanly. That investigation asked for a reproduction under real
//! sustained sequential load.
//!
//! This test probes directly. When a create gets a 429, the test polls
//! `CalendarEvent/query` for that UID at short intervals *before* it sends the retry.
//! Thus it detects an early landing independently of the result of the retry.
//!
//! ## Running it
//!
//! The environment is the same as for the other live-server scale tests of this
//! crate (refer to `docs/manual-test-live-server.md`). Use a dedicated
//! temporary account (`stw seed`), not a shared one, because this test leaves
//! some hundred events in its calendar.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_429_commit_repro -- --ignored --nocapture
//! ```
//!
//! `JMAP_429_REPRO_EVENTS` overrides the batch size (default 900). This value
//! approaches the 1000 req/60s window of `Http.rateLimitAuthenticated`, as the
//! original field sessions did, but does not always cross it.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Error as ClientError};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_test_support::{connect_for_write, env_count, unique_suffix};

/// Returns whether a new `CalendarEvent/query` on the server shows `uid`.
///
/// The query uses the same rate-limit budget as the creates. Thus a 429 here
/// means "not known", not a failure, and the poll loop of the caller tries again.
fn uid_exists(client: &Client, account_id: &Id, uid: &str) -> bool {
    let filter = CalendarEventQueryFilter {
        uid: Some(uid.to_owned()),
        ..CalendarEventQueryFilter::default()
    };
    match client.event_query(account_id, filter) {
        Ok(response) => !response.ids.is_empty(),
        Err(ClientError::Http { status: 429, .. }) => false,
        Err(error) => panic!("CalendarEvent/query by uid {uid} failed: {error}"),
    }
}

/// Returns whether `error` is a `/set` failure with an `already exists` description.
fn is_already_exists(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Set(set_error)
            if set_error
                .description
                .as_deref()
                .is_some_and(|description| description.contains("already exists"))
    )
}

#[derive(Default)]
struct Stats {
    rate_limited: usize,
    early_landings: usize,
    duplicate_after_retry: usize,
}

/// Creates one event and examines the first 429 for its UID. That 429 starts a
/// poll loop (at most 5 s) for the UID, before the retry. Otherwise it retries
/// as the `retrying()` helpers do: 5 s sleep, but at most 10 attempts here.
fn create_with_429_diagnostics(
    client: &Client,
    account_id: &Id,
    event: &CalendarEvent,
    uid: &str,
    stats: &mut Stats,
) {
    let mut polled_after_first_429 = false;
    for remaining in (0..10).rev() {
        match client.event_create(account_id, event) {
            Ok(_) => return,
            Err(ClientError::Http { status: 429, .. }) => {
                stats.rate_limited += 1;
                if !polled_after_first_429 {
                    polled_after_first_429 = true;
                    let poll_start = Instant::now();
                    while poll_start.elapsed() < Duration::from_secs(5) {
                        if uid_exists(client, account_id, uid) {
                            stats.early_landings += 1;
                            eprintln!(
                                "EARLY LANDING: uid {uid} visible {:?} after its own 429, \
                                 before any retry was sent",
                                poll_start.elapsed()
                            );
                            break;
                        }
                        sleep(Duration::from_millis(250));
                    }
                }
                if remaining == 0 {
                    panic!("uid {uid} exhausted 10 retries, still 429");
                }
                sleep(Duration::from_secs(5));
            }
            Err(error) if is_already_exists(&error) => {
                stats.duplicate_after_retry += 1;
                eprintln!(
                    "CONFIRMED: uid {uid}'s earlier 429-rejected create was committed \
                     anyway (a later attempt for the same UID got 'already exists')"
                );
                return;
            }
            Err(error) => panic!("create of uid {uid} failed unexpectedly: {error}"),
        }
    }
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_429_during_sustained_sequential_import_is_checked_for_an_early_commit() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the 429 repro");
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

    let n = env_count("JMAP_429_REPRO_EVENTS", 900);
    let run = unique_suffix();
    let mut stats = Stats::default();

    let import_start = Instant::now();
    for i in 0..n {
        let uid = format!("agent-429repro-{run}-{i}@localhost");
        let mut event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("agent-429repro-{run}-{i}"),
            "2026-06-01T12:00:00",
            "PT1H",
        );
        event.uid = Some(uid.clone());

        create_with_429_diagnostics(&client, &account_id, &event, &uid, &mut stats);
    }

    eprintln!(
        "SUMMARY: {n} creates in {:?}, {} hit 429 at least once, {} caught landing early by \
         direct polling, {} confirmed duplicate-committed via a later 'already exists'",
        import_start.elapsed(),
        stats.rate_limited,
        stats.early_landings,
        stats.duplicate_after_retry
    );

    if stats.rate_limited == 0 {
        eprintln!(
            "Never hit Http.rateLimitAuthenticated in {n} creates; raise \
             JMAP_429_REPRO_EVENTS to reproduce the load shape that originally surfaced this."
        );
    }
}
