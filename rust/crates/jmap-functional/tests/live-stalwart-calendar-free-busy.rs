// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `calendar-free-busy.rs`'s own `get_free_busy_sync` leg, against real
//! Stalwart instead of the in-process mock.
//!
//! `calendar-free-busy.rs` seeds a principal and a busy event straight into
//! `jmap_mock::state::Store`; no such backdoor exists on a real server, so
//! the busy event comes from a genuine second [`jmap_client::Client`],
//! independent of the EDS-backed [`Session`] under test, the same role it
//! plays in `live-stalwart-calendar-changes.rs`. Asking the server to answer
//! for a *different* person needs a second real account (AGY's
//! `calendar_event_with_participant_reports_busy_period_in_free_busy_query`,
//! `jmap-client/tests/live_server.rs`, already covers that at the protocol
//! layer); what this test confirms is one layer up, that the running EDS
//! backend's `get_free_busy_sync` vfunc actually reaches a real server's
//! `Principal/query` + `Principal/getAvailability` for the account's own
//! email at all, so it asks about the write-test account itself -- a plain
//! event on an account's own default calendar is enough to make that account
//! read busy for its own availability, no participant required (the same
//! finding AGY Batch 21 made when it fell back to `Principal/query` by email
//! for the owner).
//!
//! The event's date is picked clear of every other live calendar test's own
//! fixed date, so a leftover event from an earlier run of a *different* test
//! can never land inside this test's query window: `live-stalwart-
//! calendar.rs` and `live-stalwart-calendar-changes.rs` both write to
//! 2026-01-15, and AGY's own free/busy test uses 2026-10-15.
//!
//! Reuses `functional-cal-free-busy-client`'s existing binary (now also
//! storing a password before connect, which the mock never needed) and the
//! loopback-proxy/password-seed/rebase-urls mechanism `live-stalwart-
//! calendar.rs` already established.
//!
//! ## Running it
//!
//! Same environment as `live-stalwart-calendar.rs` -- see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-free-busy -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// Compact-form date (no dashes, as the FREEBUSY period text and the
/// client's `<window-start>`/`<window-end>` arguments both want), clear of
/// every other live calendar test's own fixed date -- see the module docs.
const DATE_COMPACT: &str = "20270601";
/// The same date, dashed, as `CalendarEvent::simple`'s `start` wants it.
const DATE_ISO: &str = "2027-06-01";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's event for this
/// one's query window. Mirrors every live-server test's own copy.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// `(origin, user, password)`, or `None` to skip -- mirrors
/// `live-stalwart-calendar.rs`'s own `live_server_params`.
fn live_server_params() -> Option<(String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user, password))
}

/// `live-stalwart-calendar.rs`'s own keyfile, unchanged.
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

    // One of 20 one-hour slots on `DATE_ISO`, so repeated runs of this same
    // test do not collide with each other either.
    let hour = 1 + (unique_suffix() % 20) as u8;
    let end_hour = hour + 1;
    let start_time = format!("{DATE_ISO}T{hour:02}:00:00");
    let window_start = format!("{DATE_COMPACT}T{hour:02}0000Z");
    let window_end = format!("{DATE_COMPACT}T{end_hour:02}0000Z");
    let expected_period = format!("{window_start}/{window_end}");

    // A genuine second client: a raw `jmap_client::Client` talking to the
    // real server directly, not through the loopback proxy or the EDS
    // backend under test -- see `live-stalwart-calendar-changes.rs`'s own
    // comment on this same pattern.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let raw_client = Client::builder()
        .rebase_urls_to_origin(rebase)
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

    // Checked before the exit status, same reason every other live
    // calendar leg does: a calendar the backend could not open reaches the
    // client looking exactly like one it opened and forgot to claim
    // connected.
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
