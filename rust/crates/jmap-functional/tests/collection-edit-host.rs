// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Make sure that an edit of the host of a live JMAP collection account
//! starts a new authenticate, as
//! `rust/crates/jmap-backend-collection/src/source_changed.rs` intends. An
//! AT-SPI session against a real Evolution cannot show this. The
//! `debug_print` of the backend and the GUI do not show the internal state of
//! the backend. Thus, if no traffic reaches the mock after an edit, the
//! cause can be that `AccountWatch` correctly refuses a repopulate, or a
//! real fault in the live trigger. With AT-SPI timing, the cause can also be
//! that the edit was not saved.
//!
//! This test uses the same scenario, the same real
//! `evolution-source-registry` and the same real collection backend module,
//! with `tests/functional/collection-edit-client.c` in place of a GUI. The
//! account starts with a port that has no listener, so the fan-out of the
//! first populate fails. (The authentication is anonymous, so no credentials
//! round trip occurs, and a loopback refusal is immediate.) Then the client
//! changes the port of the account to the port of the mock and saves it with
//! `e_source_write_sync`, the same call as the "OK" button of the Account
//! Editor. In both cases, EDS emits "changed" on its in-process `ESource`,
//! and the `source_changed` handler monitors that signal. Thus this test
//! examines the same question.
//!
//! The test uses `api_requests()` of the mock as its result. The doc of
//! `source_changed.rs` names the four events that schedule a populate. None
//! of them occurs after the edit: no toggle changes, no online transition
//! occurs, no manual refresh occurs, and construction is before the edit.
//! Thus a count above zero after the client exits shows that the "changed"
//! handler of the edit made a connection.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use std::net::TcpListener;

/// A port without a listener. Bind it, read it, then drop the listener
/// before a connection occurs. A loopback `connect()` to this port gets an
/// immediate refusal, so the first populate fails before the client starts.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn keyfile(port: u16) -> String {
    Keyfile::new(
        SourceKind::Collection {
            contacts: true,
            calendars: false,
            mail: false,
        },
        port,
    )
    .to_string()
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
