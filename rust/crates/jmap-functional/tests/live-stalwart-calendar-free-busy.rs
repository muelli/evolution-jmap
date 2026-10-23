// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `get_free_busy_sync` test of `calendar-free-busy.rs`, against real
//! Stalwart instead of the in-process mock.
//!
//! `calendar-free-busy.rs` writes a principal and a busy event directly into
//! `jmap_mock::state::Store`. A real server has no such backdoor. Thus a
//! second [`jmap_client::Client`], independent of the [`Session`] under test,
//! creates the busy event (as in `live-stalwart-calendar-changes.rs`).
//!
//! A query for a different person needs a second real account.
//! `calendar_event_with_participant_reports_busy_period_in_free_busy_query`
//! in `jmap-client/tests/live_server.rs` tests that at the protocol layer.
//! This test checks one layer higher: the `get_free_busy_sync` vfunc of the
//! EDS backend gets to `Principal/query` + `Principal/getAvailability` of a
//! real server for the email of the account. Thus the test asks about the
//! write-test account itself. An event on the default calendar of the
//! account is sufficient to make the account busy, with no participant.
//!
//! The date of the event is different from the fixed dates of the other live
//! calendar tests. Thus an old event from a different test cannot be in the
//! query window. `live-stalwart-calendar.rs` and
//! `live-stalwart-calendar-changes.rs` write to 2026-01-15, and the free/busy
//! test in `jmap-client` uses 2026-10-15.
//!
//! This test uses `functional-cal-free-busy-client`, which also stores a
//! password before the connect. It also uses the loopback proxy, the password
//! seed and the URL rebase from `live-stalwart-calendar.rs`.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-calendar.rs` (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-free-busy -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// The date in compact form (no dashes), for the FREEBUSY period text and the
/// `<window-start>`/`<window-end>` arguments of the client. See the module
/// docs for why this date.
const DATE_COMPACT: &str = "20270601";
/// The same date with dashes, for the `start` of `CalendarEvent::simple`.
const DATE_ISO: &str = "2027-06-01";

/// A value that is unique to this process. It selects the hour slot of this
/// run.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip. The same as
/// `live_server_params` in `live-stalwart-calendar.rs`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// The keyfile of `live-stalwart-calendar.rs`, with a different
/// `DisplayName=`.
fn keyfile(port: u16, user: &str) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional live-Stalwart free/busy test\n\
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
fn a_real_backend_answers_free_busy_for_the_write_test_account_against_stalwart() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server free/busy leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_FREE_BUSY_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    // One of 20 one-hour slots on `DATE_ISO`, so that two runs of this test
    // usually use different slots.
    let hour = 1 + (unique_suffix() % 20) as u8;
    let end_hour = hour + 1;
    let start_time = format!("{DATE_ISO}T{hour:02}:00:00");
    let window_start = format!("{DATE_COMPACT}T{hour:02}0000Z");
    let window_end = format!("{DATE_COMPACT}T{end_hour:02}0000Z");
    let expected_period = format!("{window_start}/{window_end}");

    // A second client that connects directly to the real server, not
    // through the loopback proxy or the EDS backend.
    // Always rebase, as the backend does behind the loopback proxy.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the raw second client");
    let account_id = raw_client
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = raw_client
        .calendars(&account_id)
        .expect("Calendar/get failed for the raw second client")
        .into_iter()
        .find(|calendar| calendar.is_default == Some(true))
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the default calendar");

    let event = CalendarEvent::simple(calendar_id, "agent-fnfreebusy", &start_time, "PT1H");
    raw_client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create failed for the raw second client");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-free-busy"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let output = session.run(
        &client_binary,
        &["jmap-functional", &user, &window_start, &window_end],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check this before the exit status. Otherwise a calendar that the
    // backend could not open looks the same as an open calendar that did not
    // report "connected".
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected against the real server\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed against the real server with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("free-busy-component-count"),
        Some(&"1"),
        "the running backend's get_free_busy_sync vfunc never answered for the write-test account\n{report}"
    );
    let expected_attendee = format!("mailto:{user}");
    assert_eq!(
        seen.get("free-busy-attendee"),
        Some(&expected_attendee.as_str()),
        "the answer named the wrong attendee\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-period-count"),
        Some(&"1"),
        "the event the raw second client created never reached the real server's answer\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-period-0"),
        Some(&expected_period.as_str()),
        "the busy period's times were wrong\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-fbtype-0"),
        Some(&"BUSY"),
        "a confirmed event should read back as FBTYPE=BUSY\n{report}"
    );
}
