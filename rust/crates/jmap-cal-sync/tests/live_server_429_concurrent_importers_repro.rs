// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Chases the same 429/duplicate-commit RFC-SUSPECT as
//! `live_server_429_commit_repro.rs`, but under a concurrent load shape
//! neither that test nor the isolated repro before it tried: two importers
//! writing to the SAME account at the SAME time, rather than one sequential
//! writer alone.
//!
//! Both prior investigations end on the same unexplored lead: a single
//! sequential writer never shows a 429-rejected create landing anyway, but
//! the original field observation happened on a Stalwart instance shared
//! across concurrent lanes, so contention on the account's write path or its
//! rate limiter (a check-then-act race between two writers, not a property
//! of any one writer's own retry) remains untested.
//!
//! Two independent `Client` connections authenticate to the same account and
//! each run their own thread, each with its own UID namespace (no
//! deliberate UID collision between the two importers; the point is
//! concurrent pressure on the account, not a shared UID). Each thread uses
//! the same live-diagnosis-before-retry shape as the sequential repro: the
//! instant a create 429s, it polls for that exact UID before sending the
//! retry, so an early landing is caught directly rather than inferred from
//! the retry's own outcome.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server scale tests (see
//! `docs/manual-test-live-server.md`); use a dedicated throwaway account
//! (`stw seed`), not a shared one.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_429_concurrent_importers_repro -- --ignored --nocapture
//! ```
//!
//! `JMAP_429_REPRO_EVENTS` overrides the batch size per importer (default
//! 550 each, so two importers together approach the same ballpark around
//! `Http.rateLimitAuthenticated`'s 1000 req/60s window the sequential repro
//! used, split across two concurrent writers instead of one).
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset, the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::session::CAPABILITY_CALENDARS;

/// Mirrors `live_server_429_commit_repro.rs::connect_for_write` exactly.
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

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover events can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn batch_size() -> usize {
    env::var("JMAP_429_REPRO_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(550)
}

/// Whether `uid` is visible yet via a fresh `CalendarEvent/query`, read
/// straight from the server rather than inferred from any create response.
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

/// Whether `error` is the specific rejection this hypothesis is about: an
/// `already exists`-shaped `/set` failure, not some other rejection.
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

/// Creates one event, diagnosing a 429 the moment it happens rather than
/// just retrying through it. Identical shape to the sequential repro's own
/// helper.
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

/// One importer's whole run: connects independently, then creates `n`
/// events under its own `importer_tag` UID namespace.
fn run_importer(
    origin: String,
    user: String,
    password: String,
    importer_tag: &str,
    n: usize,
) -> Stats {
    let client = Client::builder()
        .rebase_urls_to_origin(
            env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0"),
        )
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");

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
    let Some(probe) = connect_for_write() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the concurrent 429 repro"
        );
        return;
    };
    drop(probe);

    let origin = env::var("JMAP_LIVE_SERVER_URL").unwrap();
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD").unwrap();
    let n = batch_size();

    let import_start = Instant::now();
    let (origin_a, user_a, password_a) = (origin.clone(), user.clone(), password.clone());
    let importer_a = std::thread::spawn(move || run_importer(origin_a, user_a, password_a, "a", n));
    let (origin_b, user_b, password_b) = (origin, user, password);
    let importer_b = std::thread::spawn(move || run_importer(origin_b, user_b, password_b, "b", n));

    let stats_a = importer_a.join().expect("importer a panicked");
    let stats_b = importer_b.join().expect("importer b panicked");

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
