// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book backend reading and writing a shared address book against
//! a real JMAP server.
//!
//! Verifies that `connect::open_book_with_account` correctly connects to an
//! address book in another account shared via RFC 9670 `shareWith`, respects
//! `myRights` to mark the backend read-only (`writable: false`) or writable
//! (`writable: true`), and that the underlying backend sync engine reads and,
//! where allowed, writes contact records.
//!
//! ## Running it
//!
//! Needs both `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` pointing to a real Stalwart
//! server:
//!
//! ```console
//! $ cargo test -p jmap-backend-book --test live_server_shared -- --ignored
//! ```
//!
//! Skipped, not failed, when either account pair is unset.

use std::env;

use jmap_backend_book::connect;
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
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

fn connect_owner() -> Option<(Client, String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch session document for write-test account");
    Some((client, user, password, origin))
}

fn connect_recipient() -> Option<(Client, String, String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user.clone(), password.clone()))
        .expect("could not fetch session document for recipient account");
    Some((client, user, password, origin))
}

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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn shared_address_book_opens_in_backend_as_read_only_then_writable_as_grant_widens() {
    let Some((owner, _owner_address, _owner_password, origin)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set: skipping the shared address book test"
        );
        return;
    };
    let Some((recipient, recipient_address, recipient_password, _)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set: skipping the shared address book test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}: skipping the shared address book test"
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
        .expect("the owner cannot resolve the recipient principal id by email");

    let suffix = unique_suffix();
    let book_name = format!("agent-backend-book-shared-{suffix}");
    let book = owner
        .address_book_create(&owner_account_id, &AddressBook::new(book_name))
        .expect("AddressBook/set create failed against the real server");
    let book_id = book.id.clone().expect("the server named the new book");

    let owner_sync = BookSync::new(owner, owner_account_id.clone(), book_id.clone());

    let owner_contact_name = format!("agent-backend-book-shared-owner-{suffix}");
    let owner_saved = owner_sync
        .save_contact(&vcard(&owner_contact_name), None)
        .expect("ContactCard/set create failed on owner account");

    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let config = SourceConfig {
        target: ConnectTarget::Origin(origin),
        user: Some(recipient_address.clone()),
        resource_id: Some(book_id.to_string()),
        rebase_urls: rebase,
    };

    // Stage 1: Before any grant, opening the shared book through recipient backend must fail.
    let before_grant = connect::open_book_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password.clone()),
        Some(owner_account_id.as_str()),
    );
    assert!(
        before_grant.is_err(),
        "opening shared address book before grant must fail"
    );

    // Stage 2: Grant read-only access (mayRead: true).
    owner_sync
        .client()
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true}}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");

    let open_book_ro = connect::open_book_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password.clone()),
        Some(owner_account_id.as_str()),
    )
    .expect("opening shared address book with mayRead grant should succeed");

    assert!(
        !open_book_ro.writable,
        "shared address book with mayRead-only grant must open as read-only"
    );

    let (_, listed) = open_book_ro
        .list_existing()
        .expect("listing contacts through shared address book should succeed under mayRead");
    assert!(
        listed.iter().any(|c| c.uid == owner_saved.uid),
        "owner contact must be visible to recipient in shared address book"
    );

    let recipient_contact_name = format!("agent-backend-book-shared-recipient-{suffix}");
    let write_err = open_book_ro
        .save_contact(&vcard(&recipient_contact_name), None)
        .expect_err("saving contact under mayRead-only grant must fail");
    assert!(
        is_forbidden(&write_err),
        "saving contact under mayRead-only grant must be forbidden, got: {write_err:?}"
    );

    // Stage 3: Widen grant to read-write (mayRead: true, mayWrite: true).
    owner_sync
        .client()
        .address_book_update(
            &owner_account_id,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayRead": true, "mayWrite": true}}}),
        )
        .expect("AddressBook/set shareWith widen failed against the real server");

    let open_book_rw = connect::open_book_with_account(
        &config,
        Credentials::basic(recipient_address.clone(), recipient_password),
        Some(owner_account_id.as_str()),
    )
    .expect("opening shared address book with mayWrite grant should succeed");

    assert!(
        open_book_rw.writable,
        "shared address book with mayWrite grant must open as writable"
    );

    let recipient_saved = open_book_rw
        .save_contact(&vcard(&recipient_contact_name), None)
        .expect("saving contact under mayWrite grant should succeed");

    let (_, owner_listed) = owner_sync
        .list_existing()
        .expect("owner listing contacts after recipient write should succeed");
    assert!(
        owner_listed.iter().any(|c| c.uid == recipient_saved.uid),
        "contact created by recipient must be visible to owner in shared address book"
    );

    // Cleanup.
    let _ = owner_sync.remove_contact(&recipient_saved.uid);
    let _ = owner_sync.remove_contact(&owner_saved.uid);
    let _ = owner_sync
        .client()
        .address_book_destroy(&owner_account_id, &book_id);
}
