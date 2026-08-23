// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, calendar, D2's write half: a real, running
//! `evolution-calendar-factory` — not the in-process fixtures
//! `jmap-backend-cal/tests/{backend,ops}.rs` stop at — reacting to a local
//! colour edit and pushing it to the mock JMAP server.
//!
//! `jmap-backend-cal/tests/backend.rs` proves the `source_changed` vtable
//! slot is installed and differs from the parent's; `jmap-backend-cal/tests/
//! ops.rs` proves the pure diff-and-push decision
//! (`ops::on_source_changed`) against literal colours with no `ESource`
//! involved. Neither drives the FFI glue in between: reading the *live*
//! `ESourceSelectable` colour off a real `ESource` inside the vfunc body,
//! which only a real `ECalMetaBackend` instance — one actually watching a
//! real `ESource` for its own `"changed"` signal — can exercise. This test
//! is that missing link, the same way `collection-create.rs` is the missing
//! link between `jmap-backend-collection`'s own `create_resource.rs` tests
//! and a real registry.
//!
//! `tests/functional/cal-color-client.c` opens the calendar (which is what
//! makes the factory dlopen the backend and keep it alive), edits the
//! colour the way the calendar-properties dialog's colour picker does, and
//! waits — see that file's own header for why the wait is a plain sleep
//! rather than a poll.

use jmap_functional::{Session, observations, required_path};

const NEW_COLOR: &str = "#336699";

/// No `[Resource] Identity=`, mirroring `calendar.rs`'s own keyfile: the
/// backend asks the server for the account's default calendar, which is
/// what makes seeding one flagged default enough to answer it.
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
fn a_local_colour_edit_reaches_the_server() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_COLOR_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let calendar_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        state
            .account_mut(&account_id)
            .expect("the mock's default account")
            .seed_calendar("Personal", true)
    };

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/calendar-color"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(&client, &["jmap-functional", NEW_COLOR]);
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
        seen.get("written"),
        Some(&"1"),
        "the client never reported writing the colour edit\n{report}"
    );

    // The default `ESourceSelectable:color` EDS's own GParamSpec supplies —
    // asserted so a future change to that default, or to this test's own
    // keyfile, is caught here rather than read as "the push already
    // happened before the edit".
    assert_eq!(
        seen.get("initial-color"),
        Some(&"#62a0ea"),
        "the calendar's colour was not at EDS's own default before the edit\n{report}"
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let calendar = state
        .account(&account_id)
        .expect("the mock's default account")
        .calendars
        .get(&calendar_id)
        .expect("the seeded calendar is still there");
    assert_eq!(
        calendar.color.as_deref(),
        Some(NEW_COLOR),
        "the running backend's source_changed vfunc never pushed the colour edit\n{report}"
    );
}
