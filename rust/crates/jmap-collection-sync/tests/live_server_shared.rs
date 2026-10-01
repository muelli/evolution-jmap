// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Fanout::discover` against a *shared* collection, i.e. one owned by a
//! different real account, through an ACL grant.
//!
//! `jmap-book-sync/tests/live_server_shared_book.rs` and `jmap-cal-sync/
//! tests/live_server_shared_calendar.rs` already prove that once a child is
//! pointed at a specific (owner account id, collection id) pair, `BookSync`/
//! `CalSync` can read and then write a shared collection's contents. Neither
//! goes through this crate's own discovery at all, because a child source is
//! built by hand in those tests, naming the owner's account id directly.
//! What has never been confirmed is whether `Fanout::discover` — the thing a
//! real populate actually calls — would ever *find* a shared collection on
//! its own.
//!
//! `layout.rs`'s `CollectionLayout` reasons that it would not:
//! `primaryAccounts` aside, an account is only inferred for a capability when
//! it is the sole *personal* (`isPersonal`) candidate offering it, and a
//! shared account is never personal. A unit test
//! (`a_shared_account_is_not_inferred_but_is_honoured_when_named`) already
//! pins that reasoning against hand-written session JSON, including the
//! escape hatch (a server naming the shared account `primary` explicitly).
//! What no test here has done is grant real access to a real second account
//! and see what Stalwart's own session document and `Fanout::discover` do
//! with it — this file is that confirmation.
//!
//! ## Running it
//!
//! Same two-account recipe as `jmap-book-sync`'s and `jmap-cal-sync`'s
//! shared-collection tests — see `docs/manual-test-live-server.md`.
//!
//! ```console
//! $ cargo test -p evolution-jmap-collection-sync -- --ignored
//! ```
//!
//! Skipped, not failed, when either pair is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{Fanout, Parts};
use jmap_proto::contacts::AddressBook;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_PRINCIPALS};
use serde_json::json;

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-book-sync/tests/live_server_shared_book.rs::connect_owner`.
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

/// Mirrors `jmap-book-sync/tests/live_server_shared_book.rs::connect_recipient`.
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

/// Creates an address book and a calendar on the owner account, shares each
/// with the recipient account (`mayRead`, `mayWrite`, `mayShare` all
/// granted — the widest real grant, so a narrower one could only make the
/// shared collection *less* visible, not more), then confirms three things
/// through the recipient's own `Fanout::discover`: the recipient's own
/// personal collections are still listed (discovery is not broken
/// wholesale), the owner's account never becomes the recipient's resolved
/// contacts/calendars account (`primaryAccounts` is untouched by the grant),
/// and the shared collections themselves are absent from the listing.
/// Destroys both shared collections at the end regardless of where the
/// assertions land.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_collection_is_not_surfaced_by_the_recipients_own_discovery() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared-collection test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared-collection test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared-collection test"
        );
        return;
    }

    let owner_contacts_account = owner
        .primary_account(CAPABILITY_CONTACTS)
        .expect("the write-test account needs the contacts capability");
    let owner_calendars_account = owner
        .primary_account(CAPABILITY_CALENDARS)
        .expect("the write-test account needs the calendars capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_contacts_account,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .expect("Principal/query failed against the real server")
        .into_iter()
        .next()
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();

    let book_name = format!("agent-collectionsync-shared-book-{suffix}");
    let book = owner
        .address_book_create(&owner_contacts_account, &AddressBook::new(book_name))
        .expect("AddressBook/set create failed against the real server");
    let book_id = book.id.clone().expect("the server named the new book");

    let calendar_name = format!("agent-collectionsync-shared-cal-{suffix}");
    let calendar = owner
        .calendar_create(
            &owner_calendars_account,
            &jmap_proto::calendars::Calendar::new(calendar_name),
        )
        .expect("Calendar/set create failed against the real server");
    let calendar_id = calendar
        .id
        .clone()
        .expect("the server named the new calendar");

    owner
        .address_book_update(
            &owner_contacts_account,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {
                "mayRead": true, "mayWrite": true, "mayShare": true
            }}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");
    owner
        .calendar_update(
            &owner_calendars_account,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {
                "mayReadItems": true, "mayWriteAll": true, "mayShare": true
            }}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let before = Fanout::discover(&recipient, Parts::ALL)
        .expect("discovery failed for the recipient before asserting on it");

    assert_ne!(
        before.layout.contacts.as_ref().map(|account| &account.id),
        Some(&owner_contacts_account),
        "a shareWith grant must not retarget the recipient's own resolved \
         contacts account at the owner's — primaryAccounts is the server's \
         call, and granting a collection does not make it"
    );
    assert_ne!(
        before.layout.calendars.as_ref().map(|account| &account.id),
        Some(&owner_calendars_account),
        "a shareWith grant must not retarget the recipient's own resolved \
         calendars account at the owner's, for the same reason"
    );
    assert!(
        !before
            .address_books
            .iter()
            .any(|resource| resource.id == book_id),
        "the owner's shared address book must not be surfaced by the \
         recipient's own discovery: this crate has no path from a \
         shareWith grant to a resource id, confirmed against a real grant \
         rather than assumed"
    );
    assert!(
        !before
            .calendars
            .iter()
            .any(|resource| resource.id == calendar_id),
        "the owner's shared calendar must not be surfaced by the \
         recipient's own discovery, for the same reason"
    );
    assert!(
        before.layout.contacts.is_some() && before.layout.calendars.is_some(),
        "the recipient's own personal collections must still resolve \
         normally; a shareWith grant breaking the recipient's own \
         discovery would be a worse bug than not surfacing the share"
    );

    owner
        .address_book_destroy(&owner_contacts_account, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
    owner
        .calendar_destroy(&owner_calendars_account, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}
