// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The calendar test, against a real Stalwart instead of the in-process mock.
//!
//! This is the calendar version of `live-stalwart-book.rs`. It tests less
//! than `calendar.rs`: `functional-cal-live-client`
//! (`tests/functional/cal-live-client.c`) creates and reads back only one
//! non-recurring event. `functional-cal-client` writes many events (all-day,
//! zoned, recurring with exceptions, a THISANDFUTURE split). On a real
//! server, one difference in those events makes the full client fail. See
//! the header of the client.
//!
//! The method is the same as in the address-book test. `spawn_loopback_proxy`
//! satisfies the loopback-only plaintext rule of `connect_target` in
//! `jmap-backend-core`, and sends the traffic to Stalwart.
//! `JMAP_FUNCTIONAL_STORE_PASSWORD` supplies the password that a real server
//! needs. `JMAP_LIVE_SERVER_REBASE_URLS` rebases the `apiUrl` of the server
//! onto the address of the connection.
//!
//! ## How to run it
//!
//! Use the same environment as the `live_server_*.rs` tests of the
//! `jmap-*-sync` crates (see `docs/manual-test-live-server.md`): a throwaway
//! `stw seed` account, then
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

const LOCATION: &str = "Room 42";
const CATEGORIES: &str = "offsite,planning";
const TRANSP: &str = "TRANSPARENT";
const PRIORITY: &str = "1";
const CLASS: &str = "CONFIDENTIAL";
const ALARM_TRIGGER: &str = "-PT15M";

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read an event from an earlier run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-book.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `calendar.rs`, with a real `User=`. This field causes
/// `jmap-backend-core::connect::credentials` to use Basic, not anonymous.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart calendar test\n\
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
fn evolution_opens_the_calendar_and_a_write_reaches_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server calendar leg"
        );
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_CAL_LIVE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    // The session document gives the configured `apiUrl`, not the proxy
    // address. See the comment on this line in `live-stalwart-book.rs`.
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let summary = format!("agent-fncal-{:x}", unique_suffix() & 0xffff_ffff_ffff);
    let output = session.run(&client, &["jmap-functional", &summary]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check this before the exit status. A read-only calendar causes each
    // later failure to show "Permission denied", but the cause is the connect.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected against the real server\n{report}"
    );
    assert_eq!(
        readonly, "0",
        "EDS opened the real-server calendar read-only\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added event\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added an event with no UID\n{report}"
    );

    assert_eq!(
        seen.get("read-back-summary"),
        Some(&summary.as_str()),
        "the event EDS handed back is not the one that went in\n{report}"
    );
    assert_eq!(
        seen.get("read-back-location"),
        Some(&LOCATION),
        "the event EDS handed back lost or moved its location\n{report}"
    );
    assert_eq!(
        seen.get("read-back-categories"),
        Some(&CATEGORIES),
        "the event EDS handed back lost or split its categories\n{report}"
    );
    assert_eq!(
        seen.get("read-back-transp"),
        Some(&TRANSP),
        "the event EDS handed back lost whether it blocks the time it occupies\n{report}"
    );
    assert_eq!(
        seen.get("read-back-priority"),
        Some(&PRIORITY),
        "the event EDS handed back lost its priority\n{report}"
    );
    assert_eq!(
        seen.get("read-back-class"),
        Some(&CLASS),
        "the event EDS handed back lost or mistranslated its privacy\n{report}"
    );
    assert_eq!(
        seen.get("read-back-alarm-trigger"),
        Some(&ALARM_TRIGGER),
        "the event EDS handed back lost its reminder\n{report}"
    );
    // Not `== "1"`. `calendar.rs` starts a new mock for each run, but a real
    // account can keep events from an earlier run of this test. Thus the test
    // asserts that the event is present, not the exact count.
    let events_after = seen
        .get("events-after")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("the client reported no parseable events-after\n{report}"));
    assert!(
        events_after >= 1,
        "the added event is not in the calendar it was added to\n{report}"
    );
}
