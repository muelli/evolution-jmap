// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The test of `calendar-changes.rs` (two connects, one cache), against real
//! Stalwart instead of the in-process mock.
//!
//! `calendar-changes.rs` writes its server-side change directly into the
//! transaction log of `jmap_mock::state::Store`. A real server has no such
//! backdoor. Thus a second [`jmap_client::Client`], independent of the
//! [`Session`] under test, creates an event over JMAP between the two
//! connects. `live_server_changes.rs` in `jmap-cal-sync` uses the same
//! method to test `CalSync::get_changes`. This test checks one layer higher:
//! the second EDS connect, with the on-disk cache of the first connect,
//! shows a change that the real server made.
//!
//! This test uses `functional-cal-changes-client` without change. It also
//! uses the loopback proxy, the password seed and the URL rebase from
//! `live-stalwart-calendar.rs`.
//!
//! `calendar-changes.rs` starts from an empty mock. A real account can keep
//! events from earlier runs. Thus the test asserts only that the unique new
//! event of this run is not in the first list and is in the second list.
//!
//! ## How to run it
//!
//! Use the same environment as `live-stalwart-calendar.rs` (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-changes -- --ignored
//! ```
//!
//! When `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are not set, the test skips
//! and does not fail.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_loopback_proxy,
};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

const START: &str = "2026-01-15T13:00:00";
const DURATION: &str = "PT30M";

/// A value that is unique to this process. Thus a new run on the same
/// throwaway account does not read an event from an earlier run.
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
    Keyfile::new(SourceKind::Calendar, port)
        .display_name("JMAP functional live-Stalwart calendar changes test")
        .user(user)
        .to_string()
}

/// Runs `functional-cal-changes-client` and returns its sorted `event-<i>`
/// summaries.
fn list_summaries(session: &Session, client: &std::path::Path) -> (Vec<String>, String) {
    let output = session.run(client, &["jmap-functional"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let count: usize = seen
        .get("events")
        .unwrap_or_else(|| panic!("no 'events' observation\n{report}"))
        .parse()
        .unwrap_or_else(|_| panic!("'events' was not a number\n{report}"));
    let summaries = (0..count)
        .map(|index| {
            seen.get(format!("event-{index}").as_str())
                .unwrap_or_else(|| panic!("no 'event-{index}' observation\n{report}"))
                .to_string()
        })
        .collect();
    (summaries, report)
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_second_connect_pulls_a_real_servers_change_through_get_changes_sync() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server calendar-changes leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-changes"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let (first_summaries, first_report) = list_summaries(&session, &client_binary);

    // A second client: a raw `jmap_client::Client` that connects directly to
    // the real server, not through the loopback proxy or the EDS backend.
    // `rebase_urls_to_origin` is necessary because the session document can
    // name a hostname that this runner cannot reach.
    let raw_client = Client::builder()
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user, password))
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

    let new_summary = format!(
        "agent-fncalchanges-{:x}",
        unique_suffix() & 0xffff_ffff_ffff
    );
    assert!(
        !first_summaries.contains(&new_summary),
        "the unique new summary collided with one already on the server\n{first_report}"
    );
    let event = CalendarEvent::simple(calendar_id, &new_summary, START, DURATION);
    raw_client
        .event_create(&account_id, &event)
        .expect("CalendarEvent/set create failed for the raw second client");

    // A new process and a new private bus, but the same `XDG_CACHE_HOME`.
    // Thus the refresh after the second connect starts from the sync tag
    // that EDS stored on the first connect.
    let (second_summaries, second_report) = list_summaries(&session, &client_binary);
    assert!(
        second_summaries.contains(&new_summary),
        "the second connect should see the event the raw second client \
         created on the real server between the two connects\n{second_report}"
    );
}
