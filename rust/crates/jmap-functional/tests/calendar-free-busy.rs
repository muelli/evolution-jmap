// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Track E Path A's `get_free_busy_sync`, proven through a real, running
//! `evolution-calendar-factory` — not the in-process fixtures
//! `jmap-backend-cal/tests/{backend,ops}.rs` stop at.
//!
//! `backend.rs` only proves the `get_free_busy_sync` vtable slot is
//! installed on the right (`ECalBackendSyncClass`, two levels up from
//! `ECalMetaBackend`) parent class; `ops.rs` only proves the pure
//! attendee-answering decision (`ops::get_free_busy`) against a literal
//! `CalSync::free_busy` stand-in, with no `ESource`/`ECalClient` involved.
//! Neither drives the FFI glue in between: a real `ECalClient` asking a
//! real, connected `ECalMetaBackend` instance for free/busy, which is what
//! actually calls `jmap_cal_sync::CalSync::free_busy` — the
//! `Principal/query` + `Principal/getAvailability` round trip — and marshals
//! its answer back through `marshal::free_busy_list` into the `GSList` EDS
//! hands the client. This test is that missing link, the same way
//! `calendar-color.rs` is the missing link for D2's colour push.
//!
//! `tests/functional/cal-free-busy-client.c` opens the calendar (which is
//! what makes the factory dlopen the backend and keep it alive), then calls
//! `e_cal_client_get_free_busy_sync()` for one seeded principal's address
//! over a window that contains one seeded busy event.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::calendars::CalendarEvent;
use jmap_proto::principals::Principal;

const USER_EMAIL: &str = "bob@example.test";
const WINDOW_START: &str = "20260901T080000Z";
const WINDOW_END: &str = "20260901T120000Z";

/// No `[Resource] Identity=`, mirroring `calendar.rs`/`calendar-color.rs`'s
/// own keyfile: the backend asks the server for the account's default
/// calendar, which is what makes seeding one flagged default enough to
/// answer it. `get_free_busy_sync`'s own account-id resolution
/// (`jmap-backend-cal/src/connect.rs`) is the same one D2's colour push
/// uses, so this needs no collection-backend wiring either.
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
        // 09:00-10:00 UTC, inside the [08:00, 12:00) window the client asks
        // about — the one thing `Principal/getAvailability` should report
        // back as this attendee's busy period.
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

    // Checked before the exit status, for the reason every other calendar
    // leg's own assertion is: a calendar the backend could not open reaches
    // the client looking exactly like one it opened and forgot to claim
    // connected, and the exit status alone would only say "somewhere
    // earlier", not where.
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
