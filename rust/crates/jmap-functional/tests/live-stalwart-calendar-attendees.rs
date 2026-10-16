// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 108, part (1): live confirmation of item 102's code-reading
//! answer (1) -- does a meeting Evolution creates locally keep its
//! attendees after EDS reads it back?
//!
//! `jmap_ical::ical_to_event` hardcodes `participants: None` on a create
//! (see `event.rs`'s own comment beside `drawn_participants`), and
//! `jmap-cal-sync::CalSync::save_component` re-reads the event from the
//! server rather than trusting the component EDS handed it (see that
//! function's own comment), so by code reading the organizer and attendees
//! never reach the server and the local copy loses them on the very same
//! create, no second sync needed. This test drives that path through a
//! real `ECalClient` against real Stalwart (`functional-cal-attendees-
//! client`, `tests/functional/cal-attendees-client.c`) and also asks the
//! server directly over JMAP what it actually stored, so the answer is
//! evidence rather than another reading of the same source.
//!
//! The mechanism is unchanged from `live-stalwart-calendar.rs`:
//! `spawn_loopback_proxy` satisfies `jmap-backend-core::connect_target`'s
//! plaintext-stays-loopback rule while the traffic actually continues to
//! Stalwart; `JMAP_FUNCTIONAL_STORE_PASSWORD` seeds the password a real
//! server needs; `JMAP_LIVE_SERVER_REBASE_URLS` rebases the server's stated
//! `apiUrl` onto the address actually connected through.
//!
//! ## Running it
//!
//! Same environment as `live-stalwart-calendar.rs` -- see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-calendar-attendees -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};
use jmap_proto::calendars::CalendarEventQueryFilter;
use jmap_proto::session::CAPABILITY_CALENDARS;

/// A value unique to this process invocation, so a repeated run against the
/// same throwaway account never mistakes a previous run's event for this
/// one's -- mirrors every other live-server test in this crate.
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
         DisplayName=JMAP functional live-Stalwart attendees test\n\
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
fn attendees_do_not_survive_a_local_create_against_the_real_server() {
    let Some((origin, user, password)) = live_server_params() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the real-server attendees leg"
        );
        return;
    };
    let client_binary = required_path("JMAP_FUNCTIONAL_CAL_ATTENDEES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    // A direct connection, the same one every `jmap-cal-sync`/`jmap-client`
    // live-server test uses, for checking the server's own copy -- not
    // routed through EDS, so it answers what Stalwart actually stored
    // rather than what EDS made of it.
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let direct = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch the session document for the write-test account");
    let account_id = direct
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");
    let calendar_id = direct
        .calendars(&account_id)
        .expect("Calendar/get failed against the real server")
        .into_iter()
        .next()
        .expect("the write-test account needs a default calendar")
        .id
        .expect("the server named the calendar");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-calendar-attendees"
    ));
    session.write_source("jmap-functional", &keyfile(port, &user));
    session.set_variable("JMAP_FUNCTIONAL_STORE_PASSWORD", &password);
    session.set_variable("JMAP_LIVE_SERVER_REBASE_URLS", "1");
    session.stage_calendar_backend(&module);

    let summary = format!(
        "agent-fncal-attendees-{:x}",
        unique_suffix() & 0xffff_ffff_ffff
    );
    let output = session.run(&client_binary, &["jmap-functional", &summary]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
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

    // The local-copy half of item 108's question: EDS's own read-back,
    // right after the create, already shows what `CalSync::save_component`'s
    // fresh reload left in the cache.
    assert_eq!(
        seen.get("read-back-organizer"),
        Some(&""),
        "the event EDS handed back kept an organizer -- item 102's code-reading \
         answer no longer holds\n{report}"
    );
    assert_eq!(
        seen.get("read-back-attendees"),
        Some(&""),
        "the event EDS handed back kept its attendees -- item 102's code-reading \
         answer no longer holds\n{report}"
    );

    // The server-copy half: ask Stalwart directly, not through EDS, what it
    // actually stored for the event this create produced.
    let query = direct
        .event_query(
            &account_id,
            CalendarEventQueryFilter::in_calendar(calendar_id),
        )
        .expect("CalendarEvent/query failed against the real server");
    let events = direct
        .event_get(&account_id, &query.ids)
        .expect("CalendarEvent/get failed against the real server");
    let stored = events
        .list
        .iter()
        .find(|event| event.title.as_deref() == Some(summary.as_str()))
        .unwrap_or_else(|| panic!("the server has no event titled {summary:?}\n{report}"));
    assert!(
        stored
            .participants
            .as_ref()
            .is_none_or(|participants| participants.is_empty()),
        "the server stored participants for an event this product never sent any for: {:?}",
        stored.participants
    );

    direct
        .event_destroy(
            &account_id,
            stored.id.as_ref().expect("the server named the event"),
        )
        .expect("CalendarEvent/set destroy failed against the real server");
}
