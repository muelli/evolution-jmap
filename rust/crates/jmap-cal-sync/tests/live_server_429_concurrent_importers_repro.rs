// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Examines the same 429/duplicate-commit RFC-SUSPECT as
//! `live_server_429_commit_repro.rs`, but under a concurrent load shape that no
//! earlier test used. Two importers write to the SAME account at the SAME time,
//! not one sequential writer.
//!
//! Both prior investigations stop at the same lead. One sequential writer never
//! shows a 429-rejected create that the server commits. But the original field
//! observation occurred on a Stalwart instance that concurrent lanes shared.
//! Thus contention on the account write path or on its rate limiter is not
//! tested. This is a check-then-act race between two writers, not a property of
//! the retry of one writer.
//!
//! Two independent `Client` connections authenticate to the same account, each
//! in its own thread. Each thread has its own UID namespace: the goal is
//! concurrent pressure on the account, not a shared UID. Each thread examines a
//! 429 as the sequential repro does. When a create gets a 429, the thread polls
//! for that UID before it sends the retry. Thus it detects an early landing
//! directly.
//!
//! ## Running it
//!
//! The environment is the same as for the other live-server scale tests of this crate
//! (refer to `docs/manual-test-live-server.md`). Use a dedicated temporary account
//! (`stw seed`), not a shared one.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_429_concurrent_importers_repro -- --ignored --nocapture
//! ```
//!
//! `JMAP_429_REPRO_EVENTS` overrides the batch size per importer (default 550).
//! Thus the two importers together approach the 1000 req/60s window of
//! `Http.rateLimitAuthenticated`, as the sequential repro does with one writer.
//!
//! The test skips when `JMAP_LIVE_SERVER_WRITE_USER` is unset. When that
//! variable is set, a missing `_WRITE_PASSWORD` or `JMAP_LIVE_SERVER_URL` causes a panic.

use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Error as ClientError};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::session::CAPABILITY_CALENDARS;
use jmap_test_support::{LiveAccount, env_count, unique_suffix};

/// Returns whether a new `CalendarEvent/query` on the server shows `uid`. A 429
/// gives `false`.
fn uid_exists(client: &Client, account_id: &Id, uid: &str) -> bool {
    let filter = CalendarEventQueryFilter::default().uid(uid);
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

/// Creates one event and examines the first 429 before the retry.
/// Same as the helper of the sequential repro.
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

/// Runs one importer: connects independently, then creates `n` events in the
/// `importer_tag` UID namespace.
fn run_importer(account: &LiveAccount, importer_tag: &str, n: usize) -> Stats {
    let client = account.connect();

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

    let run = unique_suffix();
    let mut stats = Stats::default();
    for i in 0..n {
        let uid = format!("agent-429concurrent-{importer_tag}-{run}-{i}@localhost");
        let mut event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("agent-429concurrent-{importer_tag}-{run}-{i}"),
            "2026-06-01T12:00:00",
            "PT1H",
        );
        event.uid = Some(uid.clone());

        create_with_429_diagnostics(&client, &account_id, &event, &uid, &mut stats);
    }
    stats
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_429_under_two_concurrent_importers_on_the_same_account_is_checked_for_an_early_commit() {
    let Some(account) = LiveAccount::write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the concurrent 429 repro"
        );
        return;
    };
    let n = env_count("JMAP_429_REPRO_EVENTS", 900);

    let import_start = Instant::now();
    let (stats_a, stats_b) = std::thread::scope(|scope| {
        let importer_a = scope.spawn(|| run_importer(&account, "a", n));
        let importer_b = scope.spawn(|| run_importer(&account, "b", n));
        (
            importer_a.join().expect("importer a panicked"),
            importer_b.join().expect("importer b panicked"),
        )
    });

    let total_rate_limited = stats_a.rate_limited + stats_b.rate_limited;
    let total_early_landings = stats_a.early_landings + stats_b.early_landings;
    let total_duplicates = stats_a.duplicate_after_retry + stats_b.duplicate_after_retry;

    eprintln!(
        "SUMMARY: 2 concurrent importers x {n} creates in {:?}, {} hit 429 at least once \
         (a={}, b={}), {} caught landing early by direct polling, {} confirmed \
         duplicate-committed via a later 'already exists'",
        import_start.elapsed(),
        total_rate_limited,
        stats_a.rate_limited,
        stats_b.rate_limited,
        total_early_landings,
        total_duplicates
    );

    if total_rate_limited == 0 {
        eprintln!(
            "Never hit Http.rateLimitAuthenticated in 2x{n} concurrent creates; raise \
             JMAP_429_REPRO_EVENTS to reproduce the load shape that originally surfaced this."
        );
    }
}
