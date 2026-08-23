// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, calendar: the calendar-side twin of `book-changes.rs` — whether
//! `ECalMetaBackendClass::get_changes_sync` is actually reached through a
//! real, running `evolution-calendar-factory`, not just through
//! `jmap-backend-cal`'s own tests, which link the crate directly and call
//! `ops::get_changes` as a plain function.
//!
//! `calendar.rs`'s legs each open a calendar exactly once, so every one of
//! them drives `list_existing_sync` (a fresh meta-backend cache has no
//! stored revision to diff from) and none reaches `get_changes_sync` at
//! all — the same gap `book-changes.rs` closed for the address book,
//! explicitly left as a calendar-side follow-up there. This test opens the
//! same calendar *twice*, reusing one [`Session`]'s on-disk cache across two
//! separate `session.run()` calls (each its own process, its own private
//! bus, its own freshly started factory): the first connect has nothing
//! cached and lists in full; the second connect, against the warm cache the
//! first left behind, is where EDS's own post-connect refresh has a stored
//! sync tag to hand `get_changes_sync` instead.
//!
//! An event seeded between the two connects — straight into the mock's store
//! via [`jmap_mock::state::Store::transaction`], not
//! [`jmap_mock::state::Store::seed`], since only a transaction bumps the
//! state counter and logs a `Change` for `CalendarEvent/changes` to report —
//! is what makes the second connect's answer observably different from the
//! first, rather than merely asserting the right method name was called on
//! nothing.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::calendars::CalendarEvent;

const FIRST_EVENT: &str = "Team Standup";
const SECOND_EVENT: &str = "Retro";
const START: &str = "2026-01-15T13:00:00";
const DURATION: &str = "PT30M";

/// `docs/examples/jmap-mock-calendar.source`, with the mock's ephemeral port
/// filled in — the same keyfile shape `calendar.rs`'s own `keyfile` writes.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// Runs the calendar-changes client and hands back its sorted `event-<i>`
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
fn a_second_connect_pulls_a_change_through_get_changes_sync() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CHANGES_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let calendar_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        let calendar_id = account.seed_calendar("Personal", true);

        // The baseline event: a plain fixture, not a logged change — the
        // first connect is expected to see it via `list_existing_sync`'s
        // full query, which reads the store directly rather than the change
        // log `Store::seed` deliberately does not touch.
        let id = account.calendar_events.alloc_id();
        let mut event = CalendarEvent::simple(calendar_id.clone(), FIRST_EVENT, START, DURATION);
        event.id = Some(id.clone());
        event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        account.calendar_events.seed_with_id(id, event);

        calendar_id
    };

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar-changes"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let (first_summaries, first_report) = list_summaries(&session, &client);
    assert_eq!(
        first_summaries,
        vec![FIRST_EVENT.to_owned()],
        "the first connect should see only the seeded baseline event\n{first_report}"
    );

    let calls_after_first = server.method_calls();
    assert!(
        calls_after_first
            .iter()
            .any(|call| call == "CalendarEvent/get"),
        "the first connect's list_existing_sync should have called CalendarEvent/get\n{calls_after_first:?}"
    );
    assert!(
        !calls_after_first
            .iter()
            .any(|call| call == "CalendarEvent/changes"),
        "the first connect has no prior sync tag, so it should not have called CalendarEvent/changes\n{calls_after_first:?}"
    );

    // A real, logged change: `Store::transaction` bumps the state counter,
    // which is what makes it visible to a later `CalendarEvent/changes` at
    // all — `Store::seed`/`seed_with_id` above deliberately does not do this.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.calendar_events.transaction(|transaction| {
            let id = transaction.alloc_id();
            let mut event =
                CalendarEvent::simple(calendar_id.clone(), SECOND_EVENT, START, DURATION);
            event.id = Some(id.clone());
            event.uid = Some(format!("urn:example:event:{}", id.as_str()));
            transaction.create(id, event);
        });
    }

    // Reuses `session`'s own on-disk cache from the first connect — a fresh
    // process and a fresh private bus, but the same `XDG_CACHE_HOME`, so
    // EDS's own stored sync tag is what the second connect's post-connect
    // refresh has to work with.
    let (second_summaries, second_report) = list_summaries(&session, &client);
    assert_eq!(
        second_summaries,
        // Alphabetical, per the client's own sort: "Retro" before "Team Standup".
        vec![SECOND_EVENT.to_owned(), FIRST_EVENT.to_owned()],
        "the second connect should see the change made between the two runs\n{second_report}"
    );

    let calls_after_second = server.method_calls();
    assert!(
        calls_after_second.len() > calls_after_first.len(),
        "the second connect should have made at least one more request\n{calls_after_second:?}"
    );
    assert!(
        calls_after_second[calls_after_first.len()..]
            .iter()
            .any(|call| call == "CalendarEvent/changes"),
        "the second connect's post-connect refresh should have gone through \
         get_changes_sync (CalendarEvent/changes), not list_existing_sync again\n\
         {calls_after_second:?}"
    );
}
