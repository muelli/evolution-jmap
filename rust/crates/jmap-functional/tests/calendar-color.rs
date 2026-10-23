// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar colour write path: a real, running `evolution-calendar-factory`
//! reacts to a local colour edit and sends it to the mock JMAP server. The
//! in-process tests in `jmap-backend-cal/tests/{backend,ops}.rs` do not go
//! this far.
//!
//! `jmap-backend-cal/tests/backend.rs` shows that the `source_changed`
//! vtable slot is installed and that the parent leaves it unset.
//! `jmap-backend-cal/tests/ops.rs` tests the diff-and-push decision
//! (`ops::on_source_changed`) with literal colours and no `ESource`. Neither
//! test does the FFI step between them: the vfunc body reads the *live*
//! `ESourceSelectable` colour from a real `ESource`. Only a real
//! `ECalMetaBackend` that monitors the `"changed"` signal of a real `ESource`
//! can do this step. This test does it, as `collection-create.rs` does for
//! the `create_resource.rs` tests of `jmap-backend-collection` and a real
//! registry.
//!
//! `tests/functional/cal-color-client.c` opens the calendar, so the factory
//! loads the backend and keeps it alive. Then it edits the colour as the
//! colour picker of the calendar-properties dialog does, and waits. The
//! header of that file tells why the wait is a fixed sleep and not a poll.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};

const NEW_COLOR: &str = "#336699";

/// No `[Resource] Identity=`, as in the keyfile of `calendar.rs`. The backend
/// asks the server for the default calendar of the account, so one seeded
/// default calendar is sufficient.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
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
        seen.get("written"),
        Some(&"1"),
        "the client never reported writing the colour edit\n{report}"
    );

    // The default `ESourceSelectable:color` from the EDS GParamSpec. If that
    // default or this keyfile changes, this assertion fails. Otherwise the
    // change can look like a push before the edit.
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
