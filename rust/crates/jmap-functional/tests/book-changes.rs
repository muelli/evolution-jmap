// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book: make sure that a real `evolution-addressbook-factory` calls
//! `EBookMetaBackendClass::get_changes_sync`. The tests of
//! `jmap-backend-book` call `ops::get_changes` only as a plain function.
//!
//! The twelve tests in `address-book.rs` each open a book one time. Each
//! open calls `list_existing_sync`, because a new meta-backend cache has no
//! stored revision. None of them reaches `get_changes_sync`. This test opens
//! the same book *two* times with the on-disk cache of one [`Session`]. Each
//! `session.run()` has its own process, private bus and factory. The first
//! connect has no cache and lists all contacts. The second connect has the
//! cache from the first, so the EDS post-connect refresh gives the stored
//! sync tag to `get_changes_sync`.
//!
//! Between the two connects, the test adds a contact with
//! [`jmap_mock::state::Store::transaction`], not
//! [`jmap_mock::state::Store::seed`]. Only a transaction increments the state
//! counter and logs a `Change` for `ContactCard/changes`. Thus the second
//! connect gives a different, observable result.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_proto::contacts::ContactCard;

const FIRST_CONTACT: &str = "Alice Adams";
const SECOND_CONTACT: &str = "Bob Newman";

/// `docs/examples/jmap-mock.source` with the ephemeral port of the mock. The
/// `keyfile` of `address-book.rs` writes the same shape.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::AddressBook, port).to_string()
}

/// Run the `list` phase and return its sorted `contact-<i>` names.
fn list_names(session: &Session, client: &std::path::Path) -> (Vec<String>, String) {
    let output = session.run(client, &["jmap-functional", "list"]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let count: usize = seen
        .get("contacts")
        .unwrap_or_else(|| panic!("no 'contacts' observation\n{report}"))
        .parse()
        .unwrap_or_else(|_| panic!("'contacts' was not a number\n{report}"));
    let names = (0..count)
        .map(|index| {
            seen.get(format!("contact-{index}").as_str())
                .unwrap_or_else(|| panic!("no 'contact-{index}' observation\n{report}"))
                .to_string()
        })
        .collect();
    (names, report)
}

#[test]
fn a_second_connect_pulls_a_change_through_get_changes_sync() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let book_id = {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        let book_id = account.seed_address_book("Personal", true);

        // The baseline contact is a fixture, not a logged change. The first
        // connect sees it through the full query of `list_existing_sync`.
        // `Store::seed_with_id` does not write to the change log.
        let id = account.contact_cards.alloc_id();
        let mut card = ContactCard::simple(book_id.clone(), FIRST_CONTACT, "alice@example.com");
        card.id = Some(id.clone());
        card.uid = Some(format!("urn:example:card:{}", id.as_str()));
        account.contact_cards.seed_with_id(id, card);

        book_id
    };

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/book-changes"));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let (first_names, first_report) = list_names(&session, &client);
    assert_eq!(
        first_names,
        vec![FIRST_CONTACT.to_owned()],
        "the first connect should see only the seeded baseline contact\n{first_report}"
    );

    let calls_after_first = server.method_calls();
    assert!(
        calls_after_first
            .iter()
            .any(|call| call == "ContactCard/get"),
        "the first connect's list_existing_sync should have called ContactCard/get\n{calls_after_first:?}"
    );
    assert!(
        !calls_after_first
            .iter()
            .any(|call| call == "ContactCard/changes"),
        "the first connect has no prior sync tag, so it should not have called ContactCard/changes\n{calls_after_first:?}"
    );

    // A logged change: `Store::transaction` increments the state counter,
    // so a later `ContactCard/changes` shows it.
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.contact_cards.transaction(|transaction| {
            let id = transaction.alloc_id();
            let mut card = ContactCard::simple(book_id.clone(), SECOND_CONTACT, "bob@example.com");
            card.id = Some(id.clone());
            card.uid = Some(format!("urn:example:card:{}", id.as_str()));
            transaction.create(id, card);
        });
    }

    // A new process and private bus, but the same `XDG_CACHE_HOME`. Thus the
    // post-connect refresh starts from the sync tag that EDS stored.
    let (second_names, second_report) = list_names(&session, &client);
    assert_eq!(
        second_names,
        vec![FIRST_CONTACT.to_owned(), SECOND_CONTACT.to_owned()],
        "the second connect should see the change made between the two runs\n{second_report}"
    );

    let calls_after_second = server.method_calls();
    assert!(
        calls_after_second.len() > calls_after_first.len(),
        "the second connect should have made at least one more request\n{calls_after_second:?}"
    );
    assert!(
        calls_after_second[calls_after_first.len()..]
            .iter()
            .any(|call| call == "ContactCard/changes"),
        "the second connect's post-connect refresh should have gone through \
         get_changes_sync (ContactCard/changes), not list_existing_sync again\n\
         {calls_after_second:?}"
    );
}
