// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Chases the 429/duplicate-commit RFC-SUSPECT under the same sustained,
//! sequential bulk-import load shape that first surfaced it (this crate's
//! own 1,000/3,000-event scale checkpoints), rather than the isolated
//! single-429 shape a prior investigation already ruled out.
//!
//! The suspected mechanism: a sequential create for a brand-new UID gets an
//! HTTP 429 (`Http.rateLimitAuthenticated`), the caller retries the exact
//! same create after a delay (as every write-path test in this repository
//! does on a 429), and the retry answers `already exists` even though this
//! is the UID's first-ever create. An isolated, one-off reproduction of a
//! single deliberately-forced 429 never showed this: a 90-second poll with
//! no retry at all found nothing, and a delayed retry always created
//! cleanly. That investigation's own write-up asked for the one thing it
//! could not do in isolation: reproduce under genuine sustained sequential
//! load instead.
//!
//! This probes directly rather than inferring after the fact: the moment a
//! create 429s, it polls `CalendarEvent/query` for that exact UID at short
//! intervals *before* sending the retry, so an early landing, if any, is
//! caught independently of whatever the retry itself goes on to do.
//!
//! ## Running it
//!
//! Same environment as this crate's other live-server scale tests (see
//! `docs/manual-test-live-server.md`); use a dedicated throwaway account
//! (`stw seed`), not a shared one, since this leaves a few hundred events in
//! its calendar.
//!
//! ```console
//! $ cargo test -p evolution-jmap-cal-sync --test live_server_429_commit_repro -- --ignored --nocapture
//! ```
//!
//! `JMAP_429_REPRO_EVENTS` overrides the batch size (default 900, chosen to
//! approach but not guarantee crossing `Http.rateLimitAuthenticated`'s 1000
//! req/60s window the way the original field sessions did).
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

/// Mirrors `jmap-cal-sync/tests/live_server_scale.rs::connect_for_write`
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
        .unwrap_or(900)
}

/// Whether `uid` is visible yet via a fresh `CalendarEvent/query`, read
/// straight from the server rather than inferred from any create response.
///
/// The query itself competes for the same rate-limit budget as the creates
/// it is diagnosing, so a 429 here is "not confirmed either way", not a
/// failure: the caller's own poll loop just tries again shortly.
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
/// just retrying through it: the first 429 for this UID triggers a direct
/// poll loop (up to 5s) for the UID's own existence, independent of the
/// retry this function then goes on to make, mirroring every other
/// write-path test's own `retrying()` helper otherwise.
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

    let n = batch_size();
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
