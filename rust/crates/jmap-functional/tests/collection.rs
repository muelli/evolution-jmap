// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection backend: `evolution-source-registry` loads
//! `module-jmap-backend.so` and makes the address book and calendar children
//! of one account `.source` keyfile, as the manual procedure in
//! `docs/manual-test-collection-backend.md` describes.
//!
//! All other functional tests in this crate open a *leaf* backend (address
//! book, calendar, mail) from a `.source` file that names the server. This
//! test examines the layer above them: whether the populate/fan-out of a
//! collection account reaches a real registry and writes children that a
//! leaf backend can open. The tests of `jmap-backend-collection` make an
//! `EServerSideSource` in-process and call the vfunc bodies directly. They
//! never use the module loading and file writing of the registry.
//!
//! The test examines both sides, as the other tests do: the client program
//! reports what the registry API says about the children of the account, and
//! the mock reports what the backend requested from the server.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};

/// `docs/examples/jmap-mock-collection.source` with the ephemeral port of the
/// mock and a different `DisplayName`. It is written here for the same reason
/// as the `keyfile` of `address-book.rs`: a change to the documented file
/// does not silently change this test.
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
fn evolution_source_registry_fans_the_account_out_into_a_book_and_a_calendar() {
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // The populate step makes children from these: one address book and one
    // calendar. Both are default, so the discovery of the collection backend
    // (not this test) selects the resource of a child without
    // `[Resource] Identity=`.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_address_book("Personal", true);
        account.seed_calendar("Personal", true);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    const ACCOUNT_UID: &str = "jmap-functional-collection";
    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/collection"));
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
        seen.get("children-found"),
        Some(&"2"),
        "the populate/fan-out did not produce both children in time\n{report}"
    );
    assert_eq!(
        seen.get("address-books-found"),
        Some(&"1"),
        "the account did not get an address book child\n{report}"
    );
    assert_eq!(
        seen.get("calendars-found"),
        Some(&"1"),
        "the account did not get a calendar child\n{report}"
    );

    // Each child names our backend and has the account as its parent. These
    // are the two tasks of `child_added`.
    for prefix in ["address-book", "calendar"] {
        assert_eq!(
            seen.get(format!("{prefix}-backend-name").as_str()),
            Some(&"jmap"),
            "the {prefix} child does not name this backend\n{report}"
        );
        assert_eq!(
            seen.get(format!("{prefix}-parent").as_str()),
            Some(&ACCOUNT_UID),
            "the {prefix} child does not belong to the account\n{report}"
        );
        assert_eq!(
            seen.get(format!("{prefix}-enabled").as_str()),
            Some(&"1"),
            "the {prefix} child was created disabled\n{report}"
        );
    }

    // The server side: the backend asked for both kinds of collection, as
    // `docs/manual-test-collection-backend.md` says a fan-out does.
    let calls = server.method_calls();
    assert!(
        calls.iter().any(|call| call == "AddressBook/get"),
        "the fan-out never asked the server for its address books; it asked \
         for {calls:?}\n{report}"
    );
    assert!(
        calls.iter().any(|call| call == "Calendar/get"),
        "the fan-out never asked the server for its calendars; it asked \
         for {calls:?}\n{report}"
    );
    assert!(
        !calls.iter().any(|call| call == "Mailbox/get"),
        "MailEnabled=false in the keyfile should gate the mail fan-out off; \
         it asked for {calls:?}\n{report}"
    );
}
