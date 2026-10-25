// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `BookSync::get_changes` against a real JMAP server. This is the
//! state-token delta path of the `get_changes_sync` vfunc, which EDS uses for
//! incremental sync. A full `list_existing` download is the fallback, not the
//! usual case.
//!
//! `live_server.rs` proves create, list, load, update and remove against real
//! Stalwart, but it checks through `list_existing` and `load_contact`, never
//! `get_changes`. `tests/sync.rs` proves the classification logic (`ChangeSet`
//! → `Changes`) against `jmap-mockd`. This file is its live-server counterpart
//! and follows the recipe of `live_server.rs`.
//!
//! ## Running it
//!
//! Use the same environment as `live_server.rs` (see
//! `docs/manual-test-live-server.md`).
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! A test skips (it does not fail) when `JMAP_LIVE_SERVER_WRITE_USER` is unset.
//! The cross-account test also needs `JMAP_LIVE_SERVER_RECIPIENT_USER` and
//! `JMAP_LIVE_SERVER_RECIPIENT_PASSWORD` for a second account in the same
//! domain. It skips when `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset.

use jmap_book_sync::BookSync;
use jmap_proto::contacts::AddressBook;
use jmap_proto::session::CAPABILITY_CONTACTS;
use jmap_test_support::{connect_for_write, connect_recipient, unique_suffix};
use serde_json::json;

/// Creates, edits and removes a contact. After each step, confirms that
/// `get_changes` from the state before that step reports it. Mirrors the
/// assertions of
/// `tests/sync.rs::get_changes_reports_creations_updates_and_destructions`
/// against the real server.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_changes_reports_a_create_an_edit_and_a_removal_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let address_book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let sync = BookSync::new(client, account_id, address_book_id);

    let (state_before_create, _) = sync
        .list_existing()
        .expect("listing the book failed before the create");

    let name = format!("agent-booksync-changes-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");

    let after_create = sync
        .get_changes(&state_before_create)
        .expect("get_changes after the create failed against the real server");
    assert!(
        after_create.changed.iter().any(|c| c.uid == saved.uid),
        "the created card should be reported as changed since before it existed: {:?}",
        after_create
            .changed
            .iter()
            .map(|c| &c.uid)
            .collect::<Vec<_>>()
    );
    assert!(
        !after_create.removed.contains(&saved.uid),
        "a brand-new card must not also be reported as removed"
    );

    let new_name = format!("{name}-renamed");
    let edited_vcard = vcard.replacen(&name, &new_name, 2);
    sync.save_contact(&edited_vcard, Some(&saved.uid))
        .expect("ContactCard/set update failed against the real server");

    let after_edit = sync
        .get_changes(&after_create.new_state)
        .expect("get_changes after the edit failed against the real server");
    let edited = after_edit
        .changed
        .iter()
        .find(|c| c.uid == saved.uid)
        .unwrap_or_else(|| {
            panic!(
                "the edited card should be reported as changed since right after its creation: {:?}",
                after_edit.changed.iter().map(|c| &c.uid).collect::<Vec<_>>()
            )
        });
    assert!(
        edited.vcard.contains(&new_name),
        "the changed card get_changes reports should carry the edit: {}",
        edited.vcard
    );

    sync.remove_contact(&saved.uid)
        .expect("ContactCard/set destroy failed against the real server");

    let after_remove = sync
        .get_changes(&after_edit.new_state)
        .expect("get_changes after the removal failed against the real server");
    assert!(
        after_remove.removed.contains(&saved.uid),
        "the removed card should be reported as removed since right after its edit: {:?}",
        after_remove.removed
    );
    assert!(
        !after_remove.changed.iter().any(|c| c.uid == saved.uid),
        "a removed card must not also be reported as changed"
    );
}

/// A state is opaque per RFC 8620, but not free-form. Real Stalwart rejects
/// almost any hand-written state with `invalidArguments` (a live check showed
/// this). `jmap-mockd` (`jmap-mock/src/setops.rs`) rejects only a string that
/// it cannot parse back into its internal counter. A well-formed state from a
/// *different* account reliably gets `cannotCalculateChanges` from the real
/// server. `jmap-mail-sync`, `jmap-book-sync` and `jmap-cal-sync` catch that
/// error and do a full relist (`SyncError::is_cannot_calculate_changes`). This
/// test confirms that the account mismatch gives exactly that method error, not
/// an error type that the caller's check does not recognise.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn get_changes_reports_cannot_calculate_changes_for_another_accounts_state() {
    let Some(owner) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let Some(other) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the write-path test"
        );
        return;
    };

    let owner_account_id = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let owner_book_id = owner
        .address_books(&owner_account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");
    let owner_sync = BookSync::new(owner, owner_account_id, owner_book_id);
    let (owner_state, _) = owner_sync
        .list_existing()
        .expect("listing the owner's book failed against the real server");

    let other_account_id = other
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the recipient account needs the contacts capability");
    let other_book_id = other
        .address_books(&other_account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the recipient account needs a default address book")
        .id
        .expect("the server named the address book");
    let other_sync = BookSync::new(other, other_account_id, other_book_id);

    let error = other_sync
        .get_changes(&owner_state)
        .expect_err("a state from a different account must not be answered as if it were valid");
    assert!(
        error.is_cannot_calculate_changes(),
        "expected cannotCalculateChanges for another account's state, got: {error:?}"
    );
}

/// The doc comment of `BookSync::classify` reasons that `ContactCard/changes`
/// is account-wide. Thus an **updated** card that this book no longer holds
/// moved to a different address book, and `classify` reports it as `removed`,
/// not `changed`. The other tests here edit or destroy a card only within one
/// book, so `holds()` always returns `Ok(true)` there. `tests/sync.rs`
/// (`a_card_moved_to_another_address_book_is_reported_as_removed`) checks this
/// against `jmap-mockd`. This test confirms the real server behaviour that the
/// move needs. A raw `ContactCard/set` update of `addressBookIds` away from the
/// synced book shows in `ContactCard/changes` as an updated id, not a destroy.
/// That makes the `removed` branch of `classify` reachable.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_card_moved_to_another_address_book_is_reported_as_removed_against_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let account_id = client
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let origin_book_id = client
        .address_books(&account_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the write-test account needs a default address book")
        .id
        .expect("the server named the address book");

    let other_book = client
        .address_book_create(
            &account_id,
            &AddressBook::new(format!("agent-booksync-move-target-{}", unique_suffix())),
        )
        .expect("AddressBook/set create failed against the real server");
    let other_book_id = other_book
        .id
        .clone()
        .expect("the server named the new address book");

    // A second, independent connection: `BookSync::new` takes ownership of its
    // `Client`, and the raw `address_book_create`, `contact_update`,
    // `contact_destroy` and `address_book_destroy` calls also need one.
    let sync_client =
        connect_for_write().expect("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD vanished mid-test");
    let sync = BookSync::new(sync_client, account_id.clone(), origin_book_id);

    let (state_before_create, _) = sync
        .list_existing()
        .expect("listing the book failed before the create");

    let name = format!("agent-booksync-move-{}", unique_suffix());
    let vcard = format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    );
    let saved = sync
        .save_contact(&vcard, None)
        .expect("ContactCard/set create failed against the real server");

    let state_before_move = sync
        .get_changes(&state_before_create)
        .expect("get_changes after the create failed against the real server")
        .new_state;

    let card_id = jmap_proto::Id::from(saved.uid.as_str());
    client
        .contact_update(
            &account_id,
            &card_id,
            json!({"addressBookIds": {other_book_id.to_string(): true}}),
        )
        .expect("ContactCard/set move failed against the real server");

    let after_move = sync
        .get_changes(&state_before_move)
        .expect("get_changes after the move failed against the real server");
    assert!(
        after_move.removed.contains(&saved.uid),
        "a card moved to another address book should be reported as removed: {:?}",
        after_move
    );
    assert!(
        !after_move.changed.iter().any(|c| c.uid == saved.uid),
        "a card moved out of this book must not also be reported as changed: {:?}",
        after_move
    );

    client
        .contact_destroy(&account_id, &card_id)
        .expect("cleanup: ContactCard/set destroy failed against the real server");
    client
        .address_book_destroy(&account_id, &other_book_id)
        .expect("cleanup: AddressBook/set destroy failed against the real server");
}
