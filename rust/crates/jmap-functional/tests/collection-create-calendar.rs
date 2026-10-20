// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection backend write path for a calendar: the same
//! `e_source_remote_create_sync`/`e_source_remote_delete_sync` test as
//! `collection-create.rs`, but with `E_SOURCE_EXTENSION_CALENDAR` and
//! `Calendar/set` in place of `E_SOURCE_EXTENSION_ADDRESS_BOOK` and
//! `AddressBook/set`.
//!
//! `ECollectionBackendClass::create_resource_sync`/`delete_resource_sync`
//! must select the correct `/set` call for each kind of child. An address
//! book and a calendar can have the same resource id (RFC 8620 §1.2).
//! `the_two_kinds_of_child_of_one_jmap_id_do_not_share_a_resource_id` in
//! `jmap-backend-collection/tests/resource_id.rs` tests this risk on the
//! read side. This test examines the write side, through a real registry.

use jmap_functional::{Session, observations, required_path};

/// The same keyfile as in `collection-create.rs`. With contacts or calendars
/// enabled, the account is `remote-creatable` for each kind of child
/// (`Populating::offer_creation` in `jmap-backend-collection/src/populate.rs`).
/// The mock has no seeded data, so populate finds nothing, and the account
/// has no calendar child before the create of this test.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test account\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=true\n\
         CalendarEnabled=true\n\
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
fn evolution_source_registry_creates_and_deletes_a_calendar() {
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_CREATE_CALENDAR_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    const ACCOUNT_UID: &str = "jmap-functional-collection-create-calendar";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/collection-create-calendar"
    ));
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
    // destroy the calendar. A local-only change does not do this, and a
    // mixup of resource kinds sends the address book `/set` instead.
    let calls = server.method_calls();
    let calendar_set_calls = calls.iter().filter(|call| *call == "Calendar/set").count();
    assert_eq!(
        calendar_set_calls, 2,
        "expected one Calendar/set for the create and one for the destroy; \
         the mock saw {calls:?}\n{report}"
    );
    assert!(
        !calls.iter().any(|call| call == "AddressBook/set"),
        "creating and deleting a calendar should not touch AddressBook/set; \
         the mock saw {calls:?}\n{report}"
    );
}
