// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, address book: whether `EBookMetaBackendClass::get_changes_sync`
//! is actually reached through a real, running `evolution-addressbook-factory`,
//! not just through `jmap-backend-book`'s own tests, which link the crate
//! directly and call `ops::get_changes` as a plain function.
//!
//! `address-book.rs`'s eleven legs each open a book exactly once, so every one
//! of them drives `list_existing_sync` (a fresh meta-backend cache has no
//! stored revision to diff from) and none reaches `get_changes_sync` at all —
//! confirmed by reading every phase in `book-client.c` before writing this
//! file. This test opens the same book *twice*, reusing one [`Session`]'s
//! on-disk cache across two separate `session.run()` calls (each its own
//! process, its own private bus, its own freshly started factory): the first
//! connect has nothing cached and lists in full; the second connect, against
//! the warm cache the first left behind, is where EDS's own post-connect
//! refresh has a stored sync tag to hand `get_changes_sync` instead.
//!
//! A contact seeded between the two connects — straight into the mock's
//! store via [`jmap_mock::state::Store::transaction`], not
//! [`jmap_mock::state::Store::seed`], since only a transaction bumps the
//! state counter and logs a `Change` for `ContactCard/changes` to report —
//! is what makes the second connect's answer observably different from the
//! first, rather than merely asserting the right method name was called on
//! nothing.

use jmap_functional::{Session, observations, required_path};
use jmap_proto::contacts::ContactCard;

const FIRST_CONTACT: &str = "Alice Adams";
const SECOND_CONTACT: &str = "Bob Newman";

/// `docs/examples/jmap-mock.source`, with the mock's ephemeral port filled
/// in — the same keyfile shape `address-book.rs`'s own `keyfile` writes.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
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

/// Runs the `list` phase and hands back its sorted `contact-<i>` names.
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

        // The baseline contact: a plain fixture, not a logged change — the
        // first connect is expected to see it via `list_existing_sync`'s
        // full query, which reads the store directly rather than the change
        // log `Store::seed` deliberately does not touch.
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

    // A real, logged change: `Store::transaction` bumps the state counter,
    // which is what makes it visible to a later `ContactCard/changes` at
    // all — `Store::seed` above deliberately does not do this.
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

    // Reuses `session`'s own on-disk cache from the first connect — a fresh
    // process and a fresh private bus, but the same `XDG_CACHE_HOME`, so
    // EDS's own stored sync tag is what the second connect's post-connect
    // refresh has to work with.
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
