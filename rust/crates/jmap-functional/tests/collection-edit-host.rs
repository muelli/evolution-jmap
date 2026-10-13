// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether editing a JMAP collection account's host while it is live
//! triggers a fresh authenticate, the way
//! `rust/crates/jmap-backend-collection/src/source_changed.rs` intends, is
//! a question an AT-SPI session against a real Evolution cannot settle on
//! its own: neither this crate's own `debug_print` nor the GUI gives any
//! visibility into the backend's internal state, so a run where no traffic
//! reaches the mock after an edit cannot tell `AccountWatch` correctly
//! declining a repopulate apart from a real gap in the live trigger, and
//! AT-SPI timing cannot rule out "the edit never really landed" either.
//!
//! This is the same scenario against the same real
//! `evolution-source-registry` and the same real collection-backend module,
//! driven by `tests/functional/collection-edit-client.c` instead of a GUI:
//! the account starts pointed at a port nothing listens on, so its first
//! populate's fan-out fails (anonymous auth, no credentials round trip to
//! wait on, and a loopback refusal is immediate), then the client edits the
//! account's port to the mock's real one and commits it with
//! `e_source_write_sync`, the same call the Account Editor's own "OK"
//! button makes. EDS re-emits "changed" on its own in-process `ESource`
//! either way, which is what `source_changed`'s handler listens for, so this
//! is a faithful stand-in for the question, not a different one.
//!
//! The oracle is the mock's own `api_requests()`: nothing else in this
//! window reschedules a populate (`source_changed.rs`'s own doc names the
//! four things that do, and none of them fire here: no part toggled, no
//! online transition, no manual refresh), so a nonzero count once the client
//! exits means the edit's "changed" handler is what got a connection
//! through.

use jmap_functional::{Session, observations, required_path};
use std::net::TcpListener;

/// A port nothing is listening on, picked rather than guessed: bind it,
/// read it back, then drop the listener before anything connects. A loopback
/// `connect()` to a port with no listener and no lingering connection gets
/// an immediate refusal, which is what the account's first populate needs to
/// fail on before this test's client has even started.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test account\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=true\n\
         CalendarEnabled=false\n\
         MailEnabled=false\n\
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
fn editing_a_broken_accounts_host_triggers_a_fresh_authenticate() {
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_EDIT_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_address_book("Personal", true);
    }

    let real_port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");
    let broken_port = closed_port();

    const ACCOUNT_UID: &str = "jmap-functional-collection-edit-host";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/collection-edit-host"
    ));
    session.write_source(ACCOUNT_UID, &keyfile(broken_port));
    session.stage_collection_backend(&module);

    let output = session.run(&client, &[ACCOUNT_UID, &real_port.to_string()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );
    assert_eq!(
        seen.get("account-found"),
        Some(&"1"),
        "the registry never saw the account keyfile at all\n{report}"
    );
    assert_eq!(
        seen.get("write-ok"),
        Some(&"1"),
        "the client's e_source_write_sync of the corrected port failed\n{report}"
    );

    assert!(
        server.api_requests() > 0,
        "editing the account's port back to a reachable one never reached \
         the mock: either AccountWatch::wants_repopulate declined a retry \
         it should have granted, or on_account_changed never ran at all\n{report}"
    );
}
