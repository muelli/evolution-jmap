// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! ROADMAP item 176(a): an address book re-walk of the offline matrix.
//! `jmap-mail` needed its own on-disk folder-tree cache (176(b),
//! `jmap-mail/src/folder.rs`) for a restart with no network reachable,
//! because a plain `CamelStore` has no such concept on its own.
//! `jmap-backend-book` is instead a plain `EBookMetaBackend` subclass, and
//! EDS's own `ebmb_open_sync` (`e-book-meta-backend.c`) skips
//! `ensure_connected_sync` entirely once `e_book_meta_backend_get_ever_
//! connected` is true, a flag EDS itself persists in the on-disk
//! `EBookCache`. This test confirms that holds for real, the same way
//! `jmap-mail`'s restart-offline tests confirmed the folder-tree and
//! message-list cases rather than trusting a doc comment.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_proto::contacts::ContactCard;

const FULL_NAME: &str = "Alice Adams";
const EMAIL: &str = "alice@example.com";

/// `book-changes.rs`'s keyfile, with the port of the mock.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::AddressBook, port).to_string()
}

#[test]
fn a_restarted_book_with_no_network_still_lists_a_previously_cached_contact() {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state.account_mut(&account_id).expect("the default account");
        let book_id = account.seed_address_book("Personal", true);
        let id = account.contact_cards.alloc_id();
        let mut card = ContactCard::simple(book_id, FULL_NAME, EMAIL);
        card.id = Some(id.clone());
        card.uid = Some(format!("urn:example:card:{}", id.as_str()));
        account.contact_cards.seed_with_id(id, card);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/book-restart-offline"
    ));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_address_book_backend(&module);

    let prime = session.run(&client, &["jmap-functional", "list"]);
    let prime_stdout = String::from_utf8_lossy(&prime.stdout);
    let prime_stderr = String::from_utf8_lossy(&prime.stderr);
    let prime_report =
        format!("--- client stdout ---\n{prime_stdout}--- client stderr ---\n{prime_stderr}");
    assert!(
        prime.status.success(),
        "the priming run failed with {}\n{prime_report}",
        prime.status
    );
    let prime_seen = observations(&prime_stdout);
    assert_eq!(
        prime_seen.get("contacts"),
        Some(&"1"),
        "the priming run did not see the seeded contact\n{prime_report}"
    );

    // Nothing listens on the mock's port from here on. Dropping it joins its
    // serve thread, which only returns once the listening socket is closed,
    // so the port is free before the restarted run below ever starts.
    drop(server);

    let restart = session.run(&client, &["jmap-functional", "list"]);
    let restart_stdout = String::from_utf8_lossy(&restart.stdout);
    let restart_stderr = String::from_utf8_lossy(&restart.stderr);
    let restart_report =
        format!("--- client stdout ---\n{restart_stdout}--- client stderr ---\n{restart_stderr}");
    assert!(
        restart.status.success(),
        "the restarted, offline run failed with {}\n{restart_report}",
        restart.status
    );
    let restart_seen = observations(&restart_stdout);
    assert_eq!(
        restart_seen.get("contacts"),
        Some(&"1"),
        "the restarted, offline run did not see the previously cached contact\n{restart_report}"
    );
    assert_eq!(
        restart_seen.get("contact-0"),
        Some(&FULL_NAME),
        "the restarted, offline run's contact does not match the seeded one\n{restart_report}"
    );
}
