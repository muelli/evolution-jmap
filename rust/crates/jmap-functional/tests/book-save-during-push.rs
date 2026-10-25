// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book: each save gets an answer, also when the push of that save
//! starts a refresh while the factory still works on the save.
//!
//! The mock pushes a state change after each write. The client saves one
//! contact many times in a row on one connection. A save that the factory
//! never answers fails with the D-Bus timeout.

use jmap_functional::{Keyfile, Session, SourceKind, required_path};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;

const SAVES: u32 = 300;

#[test]
fn each_save_gets_an_answer_while_the_server_pushes() {
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
        let mut card = ContactCard::simple(book_id, "Alice Adams", "alice@example.com");
        card.id = Some(id.clone());
        card.uid = Some(format!("urn:example:card:{}", id.as_str()));
        account.contact_cards.seed_with_id(id.clone(), card);
        id
    };
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");
    let mut session = Session::new(format!(
        "{}/book-save-during-push",
        env!("CARGO_TARGET_TMPDIR")
    ));
    session.write_source(
        "jmap-functional",
        &Keyfile::new(SourceKind::AddressBook, port).to_string(),
    );
    session.stage_address_book_backend(&module);

    let count = SAVES.to_string();
    let output = session.run(
        &client,
        &["jmap-functional", "resave", contact_id.as_str(), &count],
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n--- stdout ---\n{}--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("saves={SAVES}\n")), "{stdout}");
}
