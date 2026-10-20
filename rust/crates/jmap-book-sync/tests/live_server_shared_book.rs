// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `BookSync` reading and writing a *shared* address book: a book that a
//! different real account owns and shares through an ACL grant.
//! `jmap-client/tests/live_server.rs`
//! (`address_book_shared_with_a_second_real_account_is_visible_only_after_the_grant`)
//! checks only that the grant changes what `AddressBook/get` *lists*
//! (visibility and `myRights`). It never calls `ContactCard/get` or `/set`
//! through the granted account. Thus it does not check that the contents are
//! readable, or writable after a `mayWrite` grant. A real Evolution session
//! takes this path when the user adds a shared address book as a source. This
//! test pins it at the `BookSync` level, as
//! `jmap-cal-sync/tests/live_server_shared_freebusy.rs` does for free/busy.
//!
//! Before any grant, the real server gave two answers: a method-level
//! `forbidden`, or (more often) a successful but empty listing.
//! `assert_recipient_cannot_see_address_book` in
//! `jmap-client/tests/live_server.rs` accepts both answers for
//! `AddressBook/get`. This test accepts both answers for `ContactCard` in the
//! same way.
//!
//! ## Running it
//!
//! This test needs two accounts from `docs/manual-test-live-server.md`.
//! `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` is the owner of the shared book.
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` is the account that gets the
//! grant.
//!
//! ```console
//! $ cargo test -p evolution-jmap-book-sync -- --ignored
//! ```
//!
//! The test skips (it does not fail) when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset.

use std::env;

use jmap_book_sync::{BookSync, SyncError};
use jmap_client::{Client, Credentials, Error as ClientError};
use jmap_proto::contacts::AddressBook;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server_shared_freebusy.rs::connect_owner`.
fn connect_owner() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the write-test account");
    Some((client, user))
}

/// Mirrors `jmap-cal-sync/tests/live_server_shared_freebusy.rs::connect_recipient`.
fn connect_recipient() -> Option<(Client, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password))
        .expect("could not fetch the session document for the recipient account");
    Some((client, user))
}

/// A missing grant shows in one of two ways. The server refuses the whole
/// method (no query or get rights, RFC 8620 §3.6.2). Or the method runs, but
/// the server rejects the record (query and get rights, but no write right, RFC
/// 8620 §5.3). The create of `save_contact` gets the second error.
fn is_forbidden(error: &SyncError) -> bool {
    matches!(
        error,
        SyncError::Client(ClientError::Method(method_error))
            if method_error.error_type == "forbidden"
    ) || matches!(
        error,
        SyncError::Client(ClientError::Set(set_error))
            if set_error.error_type == "forbidden"
    )
}

/// Asserts that the recipient cannot see the contact of the owner through
/// `recipient_sync`, which targets a book that is not shared (yet or any more).
/// Like `assert_recipient_cannot_see_address_book` in
/// `jmap-client/tests/live_server.rs`, it accepts both answers of the real
/// server: a method-level `forbidden`, or a successful listing without the
/// contact.
fn assert_owner_contact_not_visible(recipient_sync: &BookSync, owner_uid: &str, when: &str) {
    match recipient_sync.list_existing() {
        Err(error) => assert!(
            is_forbidden(&error),
            "expected forbidden {when}, got: {error:?}"
        ),
        Ok((_, contacts)) => assert!(
            contacts.iter().all(|contact| contact.uid != owner_uid),
            "the owner's contact should not be visible {when}: {:?}",
            contacts.iter().map(|c| &c.uid).collect::<Vec<_>>()
        ),
    }
}

fn vcard(name: &str) -> String {
    format!(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-not-a-server-id\r\n\
         FN:{name}\r\n\
         N:{name};;;;\r\n\
         END:VCARD\r\n"
    )
}

/// Creates a new address book and a contact on the owner account. Then the
/// `BookSync` of the recipient account targets that book (owner account id,
/// owner book id) in three stages:
///
/// - Before any grant: forbidden.
/// - After a `mayRead` grant: sees the contact of the owner, but a write fails.
/// - After a `mayWrite` grant: creates a contact that the owner also sees.
///
/// At the end, destroys the contacts and the book. A failed assertion stops the
/// test before this cleanup.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_address_books_contacts_are_readable_then_writable_as_the_grant_widens() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared book test");
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared book test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared book test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let book_name = format!("agent-booksync-shared-{suffix}");
    let book = owner
        .address_book_create(&owner_account_id, &AddressBook::new(book_name))
        .expect("AddressBook/set create failed against the real server");
    let book_id = book.id.clone().expect("the server named the new book");

    let owner_sync = BookSync::new(owner, owner_account_id.clone(), book_id.clone());

    let owner_contact_name = format!("agent-booksync-shared-owner-{suffix}");
    let owner_saved = owner_sync
        .save_contact(&vcard(&owner_contact_name), None)
        .expect("ContactCard/set create failed against the real server");

    let recipient_sync = BookSync::new(recipient, owner_account_id.clone(), book_id.clone());

    assert_owner_contact_not_visible(&recipient_sync, &owner_saved.uid, "before any grant");

    owner_sync
        .client()
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true}}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");

    let (_, listed) = recipient_sync
        .list_existing()
        .expect("a mayRead grant should let the recipient list the shared book");
    assert!(
        listed.iter().any(|contact| contact.uid == owner_saved.uid),
        "the owner's contact should be visible through the shared book: {:?}",
        listed.iter().map(|c| &c.uid).collect::<Vec<_>>()
    );

    let recipient_contact_name = format!("agent-booksync-shared-recipient-{suffix}");
    let write_without_grant = recipient_sync
        .save_contact(&vcard(&recipient_contact_name), None)
        .expect_err("a mayRead-only grant must not allow creating a contact");
    assert!(
        is_forbidden(&write_without_grant),
        "expected forbidden for a write under a read-only grant, got: {write_without_grant:?}"
    );

    owner_sync
        .client()
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true, "mayWrite": true}}}),
        )
        .expect("AddressBook/set shareWith widen failed against the real server");

    let recipient_saved = recipient_sync
        .save_contact(&vcard(&recipient_contact_name), None)
        .expect("a mayWrite grant should let the recipient create a contact in the shared book");

    let (_, listed_after_write) = owner_sync
        .list_existing()
        .expect("listing the book as its owner failed after the recipient's write");
    assert!(
        listed_after_write
            .iter()
            .any(|contact| contact.uid == recipient_saved.uid),
        "the contact the recipient created should be visible to the owner too: {:?}",
        listed_after_write
            .iter()
            .map(|c| &c.uid)
            .collect::<Vec<_>>()
    );

    owner_sync
        .remove_contact(&recipient_saved.uid)
        .expect("ContactCard/set destroy failed against the real server (cleanup)");
    owner_sync
        .remove_contact(&owner_saved.uid)
        .expect("ContactCard/set destroy failed against the real server (cleanup)");
    owner_sync
        .client()
        .address_book_destroy(&owner_account_id, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
}
