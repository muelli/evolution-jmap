// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book: an edit of a copy that an editor kept open keeps the edit of a
//! different client, also after a refresh replaced the cached contact.
//!
//! The first run gets the contact and writes it to a file, as an editor keeps it
//! open. Then a different client changes the note on the server. The second run
//! waits until the refresh replaces the cached contact, then saves the old copy
//! with a new email.

use jmap_client::{Client, Credentials};
use jmap_functional::{Keyfile, Session, SourceKind, required_path};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;
use serde_json::json;

const LOCAL_EMAIL: &str = "alice@local.example";

#[test]
fn a_stale_editor_keeps_a_remote_edit_of_another_property() {
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
    let mut session = Session::new(format!("{}/book-stale-edit", env!("CARGO_TARGET_TMPDIR")));
    session.write_source(
        "jmap-functional",
        &Keyfile::new(SourceKind::AddressBook, port).to_string(),
    );
    session.stage_address_book_backend(&module);
    let copy = format!("{}/book-stale-edit.vcf", env!("CARGO_TARGET_TMPDIR"));
    let run = |arguments: &[&str]| {
        let mut all = vec!["jmap-functional"];
        all.extend_from_slice(arguments);
        let output = session.run(&client, &all);
        assert!(
            output.status.success(),
            "the client failed with {}\n--- stdout ---\n{}--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&["save", contact_id.as_str(), &copy]);
    Client::connect(server.origin(), Credentials::none())
        .expect("connect to the mock")
        .contact_update(
            &account_id,
            &contact_id,
            json!({"notes": {"n1": {"note": "Remote notes"}}}),
        )
        .expect("the remote edit");
    run(&["modify-from", &copy, LOCAL_EMAIL]);

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let card = state
        .account(&account_id)
        .expect("the default account")
        .contact_cards
        .get(&contact_id)
        .expect("the contact")
        .clone();
    assert_eq!(
        card.emails.clone().expect("an email")["e0"].address,
        LOCAL_EMAIL,
        "the edit was not saved"
    );
    let notes: Vec<String> = card
        .notes
        .unwrap_or_default()
        .into_values()
        .map(|note| note.note)
        .collect();
    assert_eq!(
        notes,
        ["Remote notes"],
        "the stale copy reverted the remote edit"
    );
}
