// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, collection backend: `evolution-source-registry` loading
//! `module-jmap-backend.so`, turning one account `.source` keyfile into the
//! address-book and calendar children `docs/manual-test-collection-
//! backend.md`'s hand-run recipe describes.
//!
//! Every other functional test in this crate opens a *leaf* backend
//! (address book, calendar, mail) from a `.source` file that already names
//! the server directly. This one is about the layer above them: whether a
//! collection account's populate/fan-out actually reaches a real registry
//! and writes children a leaf backend could then open, which nothing below
//! `jmap_functional` can ask — `jmap-backend-collection`'s own tests build an
//! `EServerSideSource` in-process and call the vfunc bodies directly, never
//! through the registry's own module-loading and file-writing.
//!
//! Checked from the two ends, same as every other test here: the client
//! program reports what the registry's own API says about the account's
//! children, and the mock reports what the backend asked the server for.

use jmap_functional::{Session, observations, required_path};

/// `docs/examples/jmap-mock-collection.source`, with the mock's ephemeral
/// port filled in. Kept as a literal for the reason `address-book.rs`'s own
/// `keyfile` is: a change to the documented recipe fails this test loudly
/// instead of quietly retargeting it.
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
fn evolution_source_registry_fans_the_account_out_into_a_book_and_a_calendar() {
    let client = required_path("JMAP_FUNCTIONAL_COLLECTION_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    // What the populate step fans out into children: one address book and
    // one calendar, both flagged default so the collection backend's own
    // discovery (not this test) picks a `[Resource] Identity=`-less child's
    // resource for it.
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

    // Each child names our backend and the account as its parent — the two
    // things `child_added` is actually responsible for, not merely a source
    // of some kind existing.
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

    // The other end: the server was actually asked for both kinds of
    // collection, `ids: null`, the way `docs/manual-test-collection-
    // backend.md` says a fan-out asks.
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
