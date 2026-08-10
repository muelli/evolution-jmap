// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, calendar: `evolution-calendar-factory` loading
//! `libecalbackendjmap.so`, opening a calendar from a `.source` keyfile, and
//! serving a write through it to the mock JMAP server.
//!
//! The twin of `address-book.rs`, and deliberately so: the two backends are
//! mirrors of each other, which is exactly why one of them can carry a bug
//! the other's tests would have caught. Everything here is checked from the
//! two ends and nothing in between — the client program says what EDS gave a
//! libecal consumer, the mock says what the backend asked the server for.

use jmap_functional::{Session, observations, required_path};

/// The event the client writes. The summary is passed on its command line
/// and looked for in the mock's store, so the two ends cannot disagree about
/// it by a typo; the start is the JSCalendar spelling of the `DTSTART` in
/// `tests/functional/cal-client.c`.
const SUMMARY: &str = "Sprint planning";
const START: &str = "2026-01-15T13:00:00";

/// The keyfile from `docs/examples/jmap-mock-calendar.source`, with the
/// mock's ephemeral port filled in. Kept as a literal here rather than read
/// from `docs/` so that a change to the documented recipe fails this test
/// loudly instead of quietly retargeting it.
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

#[test]
fn evolution_opens_the_calendar_and_a_write_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // No `[Resource] Identity=` in the keyfile above, so the backend asks the
    // server for the account's default calendar. Seeding one flagged default
    // is what makes that question answerable.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        state
            .account_mut(&account_id)
            .expect("the mock's default account")
            .seed_calendar("Personal", true);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(&client, &["jmap-functional", SUMMARY]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Checked before the exit status, for the reason `address-book.rs` gives:
    // a read-only calendar turns every later failure into "Permission
    // denied", a message about the write that is really about the connect.
    //
    // It is deliberately a broad net. `e_cal_client_connect_sync` succeeds
    // even when the backend's `connect_sync` failed — `ECalMetaBackend` opens
    // the calendar and schedules the connect — so a calendar the backend
    // could not open reaches the client looking exactly like one it opened
    // and forgot to claim writable. Both are this assertion's business.
    //
    // Unless the client never got this far, in which case the failure is
    // earlier than anything here — the module missing from the factory's
    // directory, say — and the exit status is what says so.
    let readonly = seen.get("readonly").copied().unwrap_or_else(|| {
        panic!(
            "the client failed before it opened the calendar, with {}\n{report}",
            output.status
        )
    });
    assert_eq!(readonly, "0", "EDS opened the calendar read-only\n{report}");

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("events-before"),
        Some(&"0"),
        "a fresh cache against an empty calendar should hold nothing\n{report}"
    );

    let added = seen
        .get("added")
        .unwrap_or_else(|| panic!("the client reported no added event\n{report}"));
    assert!(
        !added.is_empty(),
        "EDS added an event with no UID\n{report}"
    );

    // Read back through EDS: what the meta backend kept of the write.
    assert_eq!(
        seen.get("read-back-summary"),
        Some(&SUMMARY),
        "the event EDS handed back is not the one that went in\n{report}"
    );
    assert_eq!(
        seen.get("events-after"),
        Some(&"1"),
        "the added event is not in the calendar it was added to\n{report}"
    );

    // And the other end: what the server was actually asked to do. The read
    // path is deliberately not asserted here — `ECalMetaBackend` schedules its
    // refresh rather than running it, so whether `CalendarEvent/query` has
    // happened by now is a race. The write is synchronous.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "CalendarEvent/set"),
        "the write never reached the server; it asked for {calls:?}\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let account = state
        .account(&account_id)
        .expect("the mock's default account");
    let events: Vec<_> = account.calendar_events.iter().collect();
    assert_eq!(
        events.len(),
        1,
        "the server holds {} events, not one",
        events.len()
    );

    let (_, event) = events[0];
    assert_eq!(
        event.title.as_deref(),
        Some(SUMMARY),
        "the event on the server has the wrong title: {event:?}"
    );
    assert_eq!(
        event.start.as_deref(),
        Some(START),
        "the event on the server starts at the wrong time: {event:?}"
    );
    assert!(
        event
            .calendar_ids
            .as_ref()
            .is_some_and(|calendars| calendars.values().any(|included| *included)),
        "the event on the server is in no calendar: {event:?}"
    );
}
