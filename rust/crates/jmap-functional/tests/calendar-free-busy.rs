// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_free_busy_sync` through a real, running `evolution-calendar-factory`.
//! The in-process tests in `jmap-backend-cal/tests/{backend,ops}.rs` do not
//! go this far.
//!
//! `backend.rs` shows only that the `get_free_busy_sync` vtable slot is
//! installed on the correct parent class (`ECalBackendSyncClass`, one level
//! above `ECalMetaBackend`). `ops.rs` tests only the attendee decision
//! (`ops::get_free_busy`), without an `ESource` or `ECalClient`. Neither test
//! does the FFI step between them: a real `ECalClient` asks a real, connected
//! `ECalMetaBackend` for free/busy. That calls
//! `jmap_cal_sync::CalSync::free_busy` (the `Principal/query` and
//! `Principal/getAvailability` round trip), and `marshal::free_busy_list`
//! converts the result into the `GSList` that EDS gives the client. This test
//! does that step, as `calendar-color.rs` does for the colour push.
//!
//! `tests/functional/cal-free-busy-client.c` opens the calendar, so the
//! factory loads the backend and keeps it alive. Then it calls
//! `e_cal_client_get_free_busy_sync()` for the address of one seeded
//! principal, over a window that contains one seeded busy event.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::principals::Principal;

const USER_EMAIL: &str = "bob@example.test";
const WINDOW_START: &str = "20260901T080000Z";
const WINDOW_END: &str = "20260901T120000Z";

/// No `[Resource] Identity=`, as in the keyfiles of `calendar.rs` and
/// `calendar-color.rs`. The backend asks the server for the default calendar
/// of the account, so one seeded default calendar is sufficient.
/// `get_free_busy_sync` gets the account id in the same way as the colour
/// push (`jmap-backend-cal/src/connect.rs`), so no collection backend is
/// necessary.
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
fn a_real_backend_answers_free_busy_for_a_seeded_principal() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_FREE_BUSY_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        let calendar_id = account.seed_calendar("Personal", true);
        account.seed_principal(Principal {
            name: "Bob".to_owned(),
            email: Some(USER_EMAIL.to_owned()),
            ..Principal::default()
        });
        // 09:00-10:00 UTC, in the [08:00, 12:00) window of the client. This
        // is the only busy period that `Principal/getAvailability` must
        // report for this attendee.
        account.calendar_events.seed(CalendarEvent::simple(
            calendar_id,
            "Busy meeting",
            "2026-09-01T09:00:00",
            "PT1H",
        ));
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar-free-busy"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(
        &client,
        &["jmap-functional", USER_EMAIL, WINDOW_START, WINDOW_END],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    // Check this before the exit status, as the other calendar tests do. For
    // the client, a calendar that the backend could not open looks the same
    // as one that it opened but did not mark connected. The exit status does
    // not tell where the failure occurred.
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("free-busy-component-count"),
        Some(&"1"),
        "the running backend's get_free_busy_sync vfunc never answered for the seeded principal\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-attendee"),
        Some(&"mailto:bob@example.test"),
        "the answer named the wrong attendee\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-period-count"),
        Some(&"1"),
        "the seeded busy event never reached the answer\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-period-0"),
        Some(&"20260901T090000Z/20260901T100000Z"),
        "the busy period's times were wrong\n{report}"
    );
    assert_eq!(
        seen.get("free-busy-fbtype-0"),
        Some(&"BUSY"),
        "a confirmed event should read back as FBTYPE=BUSY\n{report}"
    );
}
