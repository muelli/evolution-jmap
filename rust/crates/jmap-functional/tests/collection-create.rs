// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection backend write path: `evolution-source-registry` calls
//! `ECollectionBackendClass::create_resource_sync` and `delete_resource_sync`
//! through the public calls that "New Address Book" and "Delete" in
//! Evolution use: `e_source_remote_create_sync()` on the account and
//! `e_source_remote_delete_sync()` on the child that it returns. The tests
//! in `jmap-backend-collection/tests/{create_resource,delete_resource}.rs`
//! use in-process `EServerSideSource` fixtures instead.
//!
//! `collection.rs` tests the read direction: populate/fan-out finds what the
//! server holds. This test examines the write direction, with the same kind
//! of real registry. It uses address books only.
//! `collection-create-calendar.rs` does the same steps with
//! `E_SOURCE_EXTENSION_CALENDAR`.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};

/// The same keyfile as in `collection.rs`. This test needs contacts enabled,
/// so the populate before the create marks the account `remote-creatable`.
fn keyfile(port: u16) -> String {
    Keyfile::new(
        SourceKind::Collection {
            contacts: true,
            calendars: true,
            mail: false,
        },
        port,
    )
    .to_string()
}

#[test]
fn evolution_source_registry_creates_and_deletes_an_address_book() {
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_CREATE_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    const ACCOUNT_UID: &str = "jmap-functional-collection-create";
    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/collection-create"));
    session.write_source(ACCOUNT_UID, &keyfile(port));
    session.stage_collection_backend(&module);

    let output = session.run(&client, &[ACCOUNT_UID]);
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
        seen.get("account-creatable"),
        Some(&"1"),
        "the account never became remote-creatable, so create_resource_sync \
         is unreachable\n{report}"
    );
    assert_eq!(
        seen.get("created"),
        Some(&"1"),
        "e_source_remote_create_sync did not produce a source the registry \
         can see\n{report}"
    );
    assert_eq!(
        seen.get("created-backend-name"),
        Some(&"jmap"),
        "the created child does not name this backend\n{report}"
    );
    assert_eq!(
        seen.get("created-parent"),
        Some(&ACCOUNT_UID),
        "the created child does not belong to the account\n{report}"
    );
    assert_eq!(
        seen.get("created-enabled"),
        Some(&"1"),
        "the created child was created disabled\n{report}"
    );
    assert_eq!(
        seen.get("created-writable"),
        Some(&"1"),
        "the created child is not writable, so a rename would fail\n{report}"
    );
    assert_eq!(
        seen.get("deleted"),
        Some(&"1"),
        "e_source_remote_delete_sync did not make the created child \
         disappear from the registry\n{report}"
    );

    // The server side: the backend asked the server to create and then
    // destroy the address book. A local-only change does not do this.
    let calls = server.method_calls();
    let set_calls = calls
        .iter()
        .filter(|call| *call == "AddressBook/set")
        .count();
    assert_eq!(
        set_calls, 2,
        "expected one AddressBook/set for the create and one for the \
         destroy; the mock saw {calls:?}\n{report}"
    );
}
