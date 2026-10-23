// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The real-EDS scale test for calendars. It is the calendar version of
//! `live-stalwart-book-scale.rs` (see the header of that file for the
//! reason).
//!
//! `jmap-cal-sync/tests/live_server_scale.rs` measures `CalSync` against the
//! real server without EDS. This test measures the first open of a large
//! calendar by `evolution-calendar-factory`: the time and the peak memory. It
//! includes the meta-backend work (cache population, JSCalendar-to-
//! `ICalComponent` conversion for each event).
//!
//! A raw [`Client`] creates the events over JMAP with
//! [`CalendarEvent::simple`] (as the second client of
//! `live-stalwart-calendar-changes.rs` does). Thus the account is large
//! before EDS connects. The listing uses `functional-cal-changes-client`
//! without change: it connects, lists each event and prints the count, as
//! the `list` phase of `functional-book-client` does for the book test.
//!
//! ## How to run it
//!
//! Use the same environment as the other `live-stalwart-*` tests in this
//! crate (see `docs/manual-test-live-server.md`). Build with
//! `-DENABLE_FUNCTIONAL_TESTS=ON`, so that `JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT`/
//! `JMAP_FUNCTIONAL_CAL_MODULE` are set. `JMAP_SCALE_TEST_EVENTS` sets the
//! batch size (default 3000). Use a throwaway write-test account only for
//! this test, because the test leaves all the events in its calendar.
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-scale -- --ignored --nocapture
//! ```
//!
//! The test also needs the EDS runtime packages (`docs/functional-tests.md`).
//! If the host does not have them, run the test in the `ubuntu:24.04` podman
//! recipe in the section "The `functional` job itself, reproduced from
//! scratch" of `docs/eds-version-matrix.md`.
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read an event from an earlier run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn batch_size() -> usize {
    env::var("JMAP_SCALE_TEST_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3000)
}

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-book-scale.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// Retries `attempt` on a 429 (`Http.rateLimitAuthenticated`) up to 19 times,
/// with a 5-second sleep before each retry. A copy of `retrying` in
/// `live-stalwart-book-scale.rs` (see its comment for the reason).
fn retrying<T>(mut attempt: impl FnMut() -> Result<T, ClientError>) -> Result<T, ClientError> {
    for remaining in (0..20).rev() {
        match attempt() {
            Err(ClientError::Http { status: 429, .. }) if remaining > 0 => {
                sleep(Duration::from_secs(5));
            }
            other => return other,
        }
    }
    unreachable!("the loop above always returns on its last iteration (remaining == 0)")
}

/// The keyfile of `live-stalwart-calendar.rs`, with a different
/// `DisplayName=`.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart calendar scale test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={user}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn the_calendar_factorys_first_open_of_a_real_sized_calendar_is_measured() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-EDS calendar scale leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");
    let n = batch_size();
    let run = unique_suffix();

    // A raw client that connects directly to the real server, not through
    // the loopback proxy or EDS. It makes the large calendar before EDS sees
    // it.
    // Always rebase, as the backend does behind the loopback proxy.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the raw seeding client");
    let account_id = raw_client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = raw_client
        .calendars(&account_id)
        .expect("Calendar/get failed for the raw seeding client")
        .into_iter()
        .find(|calendar| calendar.is_default == Some(true))
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the default calendar");

    let import_start = Instant::now();
    for i in 0..n {
        let summary = format!("agent-fncalscale-{run:x}-{i}");
        let start = format!("2026-01-{:02}T13:00:00", 1 + (i % 28));
        let event = CalendarEvent::simple(calendar_id.clone(), &summary, &start, "PT1H");
        retrying(|| raw_client.event_create(&account_id, &event))
            .unwrap_or_else(|error| panic!("CalendarEvent/set create #{i} failed: {error}"));
    }
    let import_elapsed = import_start.elapsed();
    eprintln!("SCALE: seeded {n} events directly over JMAP in {import_elapsed:?}");

    // EDS starts only now, in a new `Session`. Thus this is the first open of
    // the account by `evolution-calendar-factory`, on a calendar that is
    // already large.
    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-scale"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let open_start = Instant::now();
    let (output, peak_rss_kb) = session.run_measuring_peak_rss(
        &client_binary,
        &["jmap-functional"],
        "evolution-calendar-factory",
    );
    let open_elapsed = open_start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let listed: usize = seen
        .get("events")
        .unwrap_or_else(|| panic!("no 'events' observation\n{report}"))
        .parse()
        .unwrap_or_else(|_| panic!("'events' was not a number\n{report}"));
    assert_eq!(
        listed, n,
        "the factory's first listing should see exactly the {n} events this run just seeded, \
         on a freshly seeded throwaway account\n{report}"
    );

    eprintln!(
        "SCALE: evolution-calendar-factory's first open + cold listing of {listed} events \
         took {open_elapsed:?}"
    );
    match peak_rss_kb {
        Some(kb) => eprintln!("SCALE: evolution-calendar-factory peak RSS {kb} kB"),
        None => eprintln!(
            "SCALE: never saw an evolution-calendar-factory process to measure -- \
             did the open even reach the factory?"
        ),
    }
}
