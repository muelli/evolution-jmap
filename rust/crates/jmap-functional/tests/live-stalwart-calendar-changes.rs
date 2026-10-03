// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `calendar-changes.rs`'s own two-connects-one-cache leg, against real
//! Stalwart instead of the in-process mock.
//!
//! `calendar-changes.rs` seeds its "a change happened on the server" step
//! straight into `jmap_mock::state::Store`'s transaction log, something a
//! real server has no backdoor for. This test makes the change the way a
//! real second person would: a genuine second [`jmap_client::Client`],
//! independent of the EDS-backed [`Session`] under test, creates a calendar
//! event directly over JMAP in between the two connects -- the same kind of
//! raw second-client write `jmap-cal-sync`'s own `live_server_changes.rs`
//! already uses to confirm `CalSync::get_changes` itself against real
//! Stalwart. What this test confirms is one layer up: that EDS's own second
//! connect, reusing the first connect's on-disk cache, actually surfaces a
//! change a real server made independently, not just one the mock chose to
//! report.
//!
//! Reuses `functional-cal-changes-client`'s existing binary unchanged, and
//! the loopback-proxy/password-seed/rebase-urls mechanism
//! `live-stalwart-calendar.rs` already established for pointing the EDS side
//! at a real, non-loopback server.
//!
//! Unlike `calendar-changes.rs`, which starts from an empty mock and
//! controls every event in it, a real account can carry events earlier runs
//! left behind, so the assertion is not "the list is exactly these two
//! summaries" but "this run's own unique new event, absent from the first
//! connect's list, is present in the second".
//!
//! ## Running it
//!
//! Same environment as `live-stalwart-calendar.rs` -- see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-changes -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::session::CAPABILITY_CALENDARS;

const START: &str = "2026-01-15T13:00:00";
const DURATION: &str = "PT30M";

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's event for this
/// one's. Mirrors every live-server test's own copy.
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
         DisplayName=JMAP functional live-Stalwart calendar changes test\n\
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

/// Runs `functional-cal-changes-client` and hands back its sorted
/// `event-<i>` summaries.
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

    // A genuine second client: a raw `jmap_client::Client` talking to the
    // real server directly, not through the loopback proxy or the EDS
    // backend under test -- the same role `live_server_changes.rs`'s own
    // second connections play for the sync-layer version of this question.
    // `rebase_urls_to_origin` mirrors every `jmap-*-sync` live-server test's
    // own `connect_for_write`: the session document can name a configured
    // hostname this runner cannot reach.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let raw_client = Client::builder()
        .rebase_urls_to_origin(rebase)
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

    // Reuses `session`'s own on-disk cache from the first connect -- a fresh
    // process and a fresh private bus, but the same `XDG_CACHE_HOME`, so
    // EDS's own stored sync tag is what the second connect's post-connect
    // refresh has to work with.
    let (second_summaries, second_report) = list_summaries(&session, &client_binary);
    assert!(
        second_summaries.contains(&new_summary),
        "the second connect should see the event the raw second client \
         created on the real server between the two connects\n{second_report}"
    );
}
