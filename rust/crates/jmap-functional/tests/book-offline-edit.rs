// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book: an offline edit that a real `evolution-addressbook-factory`
//! replays keeps the edit of a different client.
//!
//! The test runs the client three times on the on-disk cache of one
//! [`Session`]. The first run lists the book, so EDS caches the contact and its
//! extra. Then a different client edits the contact on the server. The second
//! run edits the email while the server is out of reach, so EDS keeps the edit
//! in its cache with the old extra. The third run connects again, and EDS
//! replays the edit with `E_CONFLICT_RESOLUTION_KEEP_SERVER`.
//!
//! A real factory is necessary: only EDS stores the extra in its cache and
//! gives it back on the replay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jmap_client::{Client, Credentials};
use jmap_functional::{Keyfile, Session, SourceKind, required_path, spawn_switchable_proxy};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;
use serde_json::{Value, json};

const FULL_NAME: &str = "Alice Adams";
const EMAIL: &str = "alice@example.com";
const LOCAL_EMAIL: &str = "alice@local.example";

/// `book-changes.rs`'s keyfile, with the port of the proxy.
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
            let mut card = ContactCard::simple(book_id, FULL_NAME, EMAIL);
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

    fn run(&self, arguments: &[&str]) {
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
    }

    /// Edits the contact on the server, as a different client does.
    fn edit_remotely(&self, patch: Value) {
        Client::connect(self.server.origin(), Credentials::none())
            .expect("connect to the mock")
            .contact_update(&self.account_id, &self.contact_id, patch)
            .expect("the remote edit");
    }

    /// Lists online, applies the remote `patch`, edits the email offline, and
    /// lists online again. Returns the method calls of the last list.
    fn replay_after(&self, patch: Value) -> Vec<String> {
        self.run(&["list"]);
        self.edit_remotely(patch);
        self.up.store(false, Ordering::SeqCst);
        self.run(&["edit", self.contact_id.as_str(), LOCAL_EMAIL]);
        let before = self.server.method_calls().len();
        self.up.store(true, Ordering::SeqCst);
        self.run(&["list"]);
        self.server.method_calls().split_off(before)
    }

    fn card(&self) -> ContactCard {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let account = state
            .account(&self.account_id)
            .expect("the default account");
        account
            .contact_cards
            .get(&self.contact_id)
            .expect("the contact")
            .clone()
    }
}

fn full_name(card: &ContactCard) -> String {
    card.name
        .clone()
        .and_then(|name| name.full)
        .unwrap_or_default()
}

fn email(card: &ContactCard) -> String {
    card.emails.clone().expect("an email")["e0"].address.clone()
}

#[test]
fn an_offline_edit_keeps_a_remote_edit_of_another_property() {
    let setup = Setup::start("book-offline-edit-other");
    setup.replay_after(json!({"name/full": "Alice Remote"}));

    let card = setup.card();
    assert_eq!(
        email(&card),
        LOCAL_EMAIL,
        "the offline edit was not replayed"
    );
    assert_eq!(
        full_name(&card),
        "Alice Remote",
        "the replay reverted the remote edit"
    );
}

#[test]
fn keep_server_on_replay_writes_nothing_for_a_property_both_sides_changed() {
    let setup = Setup::start("book-offline-edit-same");
    let calls = setup.replay_after(json!({"emails/e0/address": "alice@remote.example"}));

    assert_eq!(email(&setup.card()), "alice@remote.example");
    assert!(
        calls.iter().any(|call| call == "ContactCard/get"),
        "the third run never reached the server\n{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call == "ContactCard/set"),
        "the replay wrote to the server\n{calls:?}"
    );
}
