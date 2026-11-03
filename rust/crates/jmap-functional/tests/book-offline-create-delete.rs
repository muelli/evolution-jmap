// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book: a contact created or deleted while offline reaches the
//! server once a real `evolution-addressbook-factory` reconnects.
//! `book-offline-edit.rs` does the same check for an edit of an existing
//! contact; this file covers the two remaining write shapes ROADMAP item
//! 176(a) asks about.
//!
//! Both tests run the client three times on the on-disk cache of one
//! [`Session`]: once online (so EDS caches the book and, for the delete
//! test, the seeded contact), once with the proxy down (the write that
//! `EBookMetaBackend` queues as an offline change instead of sending), and
//! once online again, which is when EDS uploads the queued change.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jmap_functional::{
    Keyfile, Session, SourceKind, observations, required_path, spawn_switchable_proxy,
};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;

const SEEDED_FULL_NAME: &str = "Alice Adams";
const SEEDED_EMAIL: &str = "alice@example.com";
const NEW_FULL_NAME: &str = "Gillian Anderson";
/// A real 69-byte PNG (one red pixel), the same bytes `address-book.rs` uses
/// for its `write` leg. `book-client.c`'s `write` phase needs a photo
/// argument; the bytes themselves are not this test's concern.
const PHOTO_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::AddressBook, port).to_string()
}

/// One mock with one seeded contact, a session on a switchable proxy to it,
/// and the paths of the client.
struct Setup {
    server: MockServer,
    account_id: Id,
    contact_id: Id,
    up: Arc<AtomicBool>,
    session: Session,
    client: std::path::PathBuf,
}

impl Setup {
    fn start(name: &str) -> Self {
        let client = required_path("JMAP_FUNCTIONAL_BOOK_CLIENT");
        let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let contact_id = {
            let state = server.state();
            let mut state = state.lock().expect("mock state lock");
            let account = state.account_mut(&account_id).expect("the default account");
            let book_id = account.seed_address_book("Personal", true);
            let id = account.contact_cards.alloc_id();
            let mut card = ContactCard::simple(book_id, SEEDED_FULL_NAME, SEEDED_EMAIL);
            card.id = Some(id.clone());
            card.uid = Some(format!("urn:example:card:{}", id.as_str()));
            account.contact_cards.seed_with_id(id.clone(), card);
            id
        };
        let authority = server
            .origin()
            .strip_prefix("http://")
            .expect("the mock serves plain http")
            .to_owned();
        let up = Arc::new(AtomicBool::new(true));
        let port = spawn_switchable_proxy(authority, up.clone());
        let mut session = Session::new(format!("{}/{name}", env!("CARGO_TARGET_TMPDIR")));
        session.write_source("jmap-functional", &keyfile(port));
        session.stage_address_book_backend(&module);
        Self {
            server,
            account_id,
            contact_id,
            up,
            session,
            client,
        }
    }

    fn run(&self, arguments: &[&str]) -> String {
        let mut all = vec!["jmap-functional"];
        all.extend_from_slice(arguments);
        let output = self.session.run(&self.client, &all);
        assert!(
            output.status.success(),
            "the client failed with {}\n--- stdout ---\n{}--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn full_names_on_server(&self) -> Vec<String> {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account
            .contact_cards
            .iter()
            .map(|(_, card)| {
                card.name
                    .clone()
                    .and_then(|name| name.full)
                    .unwrap_or_default()
            })
            .collect()
    }

    fn has_contact_on_server(&self, id: &Id) -> bool {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account.contact_cards.contains(id)
    }
}

#[test]
fn a_contact_created_offline_reaches_the_server_on_reconnect() {
    let setup = Setup::start("book-offline-create");

    // Online: caches the book and sets `ever_connected`, the condition
    // `EBookMetaBackend` needs before it will open with no connection at all.
    setup.run(&["list"]);

    setup.up.store(false, Ordering::SeqCst);
    let stdout = setup.run(&["write", NEW_FULL_NAME, PHOTO_BASE64]);
    let seen = observations(&stdout);
    assert_eq!(
        seen.get("readonly"),
        Some(&"0"),
        "the book connected as read-only while offline"
    );
    let added = seen
        .get("added")
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| panic!("the offline create reported no UID\n{stdout}"));
    assert!(
        !setup
            .full_names_on_server()
            .contains(&NEW_FULL_NAME.to_owned()),
        "the create reached the server while the proxy was down"
    );

    setup.up.store(true, Ordering::SeqCst);
    // Reconnecting uploads the queued create; `list` is enough to trigger it,
    // the same way `book-offline-edit.rs`'s third run triggers the replay of
    // a queued edit.
    setup.run(&["list"]);

    assert!(
        setup
            .full_names_on_server()
            .contains(&NEW_FULL_NAME.to_owned()),
        "the offline create never reached the server after reconnecting, EDS-assigned uid {added}"
    );
}

#[test]
fn a_contact_deleted_offline_reaches_the_server_on_reconnect() {
    let setup = Setup::start("book-offline-delete");

    setup.run(&["list"]);

    setup.up.store(false, Ordering::SeqCst);
    let stdout = setup.run(&["remove", setup.contact_id.as_str()]);
    let seen = observations(&stdout);
    assert_eq!(seen.get("removed"), Some(&"1"));
    assert!(
        setup.has_contact_on_server(&setup.contact_id),
        "the delete reached the server while the proxy was down"
    );

    setup.up.store(true, Ordering::SeqCst);
    setup.run(&["list"]);

    assert!(
        !setup.has_contact_on_server(&setup.contact_id),
        "the offline delete never reached the server after reconnecting"
    );
}
