// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `Fanout::discover` and `delete_collection` on a real server with a
//! collection that another account shares through an ACL grant.
//!
//! The shared-collection tests of `jmap-book-sync` and `jmap-cal-sync` make
//! their child sources by hand. These tests make sure that:
//!
//! - The discovery finds a shared collection through the `isPersonal: false`
//!   account of the owner, and drops it when the owner withdraws the grant.
//! - A delete by the recipient goes to the account of the owner, and succeeds
//!   only when the grant includes `mayDelete`.
//!
//! ## Running it
//!
//! Use the two-account setup of the shared-collection tests of `jmap-book-sync`
//! and `jmap-cal-sync` (refer to `docs/manual-test-live-server.md`).
//!
//! ```console
//! $ cargo test -p evolution-jmap-collection-sync -- --ignored
//! ```
//!
//! The tests skip (and do not fail) when one of the two `_USER` variables is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{ChildKind, DeleteFailure, Doomed, Fanout, Parts, delete_collection};
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

/// Same as `connect_owner` in `jmap-book-sync/tests/live_server_shared_book.rs`.
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

/// Same as `connect_recipient` in `jmap-book-sync/tests/live_server_shared_book.rs`.
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

/// Shares a writable book and a read-only calendar, then withdraws both grants.
///
/// On Stalwart, new shares have `isSubscribed: false`, but both are children.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_collection_is_a_child_of_the_recipients_collection_until_unshared() {
    let Some((owner, owner_address)) = connect_owner() else {
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
        .address_book_create(
            &owner_contacts_account,
            &AddressBook::new(book_name.clone()),
        )
        .expect("AddressBook/set create failed against the real server");
    let book_id = book.id.clone().expect("the server named the new book");

    let calendar_name = format!("agent-collectionsync-shared-cal-{suffix}");
    let calendar = owner
        .calendar_create(
            &owner_calendars_account,
            &jmap_proto::calendars::Calendar::new(calendar_name.clone()),
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
                "mayRead": true, "mayWrite": true
            }}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");
    owner
        .calendar_update(
            &owner_calendars_account,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {
                "mayReadItems": true, "mayReadFreeBusy": true
            }}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let book_child = format!("addressbook:{owner_contacts_account}/{book_id}");
    let calendar_child = format!("calendar:{owner_calendars_account}/{calendar_id}");

    // A new client gets a new session document, which shows the account of the owner.
    let (recipient, _) = connect_recipient().expect("connected once already");
    let subscribed = Fanout::discover(&recipient, Parts::ALL)
        .expect("discovery failed for the recipient after the grants");
    let resource_ids = |fanout: &Fanout| {
        fanout
            .children()
            .into_iter()
            .map(|child| child.resource_id)
            .collect::<Vec<_>>()
    };
    let calendar_listing = subscribed
        .shared
        .iter()
        .find(|listing| listing.account.id == owner_calendars_account)
        .and_then(|listing| listing.calendars.as_ref())
        .expect("the owner's account is listed as shared with the recipient");
    eprintln!(
        "shared calendars as listed: {:?}",
        calendar_listing
            .iter()
            .map(|resource| &resource.id)
            .collect::<Vec<_>>()
    );
    let refused = recipient.calendar_update(
        &owner_calendars_account,
        &calendar_id,
        json!({"isSubscribed": true}),
    );
    eprintln!("the recipient subscribing to the read-only calendar: {refused:?}");

    let children = subscribed.children();
    let book_source = children
        .iter()
        .find(|child| child.resource_id == book_child)
        .unwrap_or_else(|| panic!("no child {book_child} in {:?}", resource_ids(&subscribed)));
    let calendar_source = children
        .iter()
        .find(|child| child.resource_id == calendar_child)
        .unwrap_or_else(|| {
            panic!(
                "no child {calendar_child} in {:?}",
                resource_ids(&subscribed)
            )
        });

    let owner_name = &subscribed
        .shared
        .iter()
        .find(|listing| listing.account.id == owner_calendars_account)
        .expect("listed above")
        .account
        .name;
    eprintln!("owner account name as the server states it: {owner_name:?} ({owner_address})");
    assert_eq!(
        book_source.display_name,
        format!("{book_name} ({owner_name})")
    );
    assert_eq!(
        calendar_source.display_name,
        format!("{calendar_name} ({owner_name})")
    );
    assert_eq!(
        calendar_source.identity(),
        format!("{owner_calendars_account}/{calendar_id}")
    );
    assert!(
        calendar_source.read_only,
        "the calendar was shared without mayWriteAll"
    );
    assert!(!book_source.read_only, "the book was shared with mayWrite");
    assert_ne!(
        subscribed
            .layout
            .calendars
            .as_ref()
            .map(|account| &account.id),
        Some(&owner_calendars_account),
        "a grant must not retarget the recipient's own calendars account"
    );
    assert!(
        subscribed.layout.contacts.is_some() && subscribed.layout.calendars.is_some(),
        "the recipient's own collections still resolve"
    );
    assert!(!subscribed.is_obsolete(&book_child));
    assert!(!subscribed.is_obsolete(&calendar_child));

    owner
        .address_book_update(
            &owner_contacts_account,
            &book_id,
            json!({"shareWith": null}),
        )
        .expect("AddressBook/set withdrawing the grant failed");
    owner
        .calendar_update(
            &owner_calendars_account,
            &calendar_id,
            json!({"shareWith": null}),
        )
        .expect("Calendar/set withdrawing the grant failed");

    let (recipient, _) = connect_recipient().expect("connected once already");
    let unshared = Fanout::discover(&recipient, Parts::ALL)
        .expect("discovery failed for the recipient after the grants were withdrawn");
    assert!(
        unshared.is_obsolete(&book_child),
        "the withdrawn book's child must go"
    );
    assert!(
        unshared.is_obsolete(&calendar_child),
        "the withdrawn calendar's child must go"
    );

    owner
        .address_book_destroy(&owner_contacts_account, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
    owner
        .calendar_destroy(&owner_calendars_account, &calendar_id)
        .expect("Calendar/set destroy failed against the real server (cleanup)");
}

/// The recipient deletes a calendar shared with `mayDelete` and a book shared without it.
///
/// Only the calendar delete succeeds. The server refuses the book delete.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_collection_is_deleted_in_the_owners_account_only_with_may_delete() {
    let Some((owner, _)) = connect_owner() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared-delete test");
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared-delete test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared-delete test"
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
    let book_id = owner
        .address_book_create(
            &owner_contacts_account,
            &AddressBook::new(format!("agent-collectionsync-keep-book-{suffix}")),
        )
        .expect("AddressBook/set create failed against the real server")
        .id
        .expect("the server named the new book");
    let calendar_id = owner
        .calendar_create(
            &owner_calendars_account,
            &jmap_proto::calendars::Calendar::new(format!(
                "agent-collectionsync-doomed-cal-{suffix}"
            )),
        )
        .expect("Calendar/set create failed against the real server")
        .id
        .expect("the server named the new calendar");

    owner
        .address_book_update(
            &owner_contacts_account,
            &book_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {
                "mayRead": true, "mayWrite": true
            }}}),
        )
        .expect("AddressBook/set shareWith failed against the real server");
    owner
        .calendar_update(
            &owner_calendars_account,
            &calendar_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {
                "mayReadItems": true, "mayReadFreeBusy": true, "mayDelete": true
            }}}),
        )
        .expect("Calendar/set shareWith failed against the real server");

    let (recipient, _) = connect_recipient().expect("connected once already");
    let fanout = Fanout::discover(&recipient, Parts::ALL)
        .expect("discovery failed for the recipient after the grants");
    let children = fanout.children();
    let child = |resource_id: String| {
        children
            .iter()
            .find(|child| child.resource_id == resource_id)
            .unwrap_or_else(|| panic!("no child {resource_id}"))
            .clone()
    };
    let book = child(format!("addressbook:{owner_contacts_account}/{book_id}"));
    let calendar = child(format!("calendar:{owner_calendars_account}/{calendar_id}"));
    eprintln!(
        "remote-deletable: book {}, calendar {}",
        book.remote_deletable, calendar.remote_deletable
    );

    let refused = delete_collection(
        &recipient,
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: jmap_proto::Id::new(book.identity()),
        },
    );
    eprintln!("the recipient deleting the book shared without mayDelete: {refused:?}");
    let deleted = delete_collection(
        &recipient,
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: jmap_proto::Id::new(calendar.identity()),
        },
    );
    eprintln!("the recipient deleting the calendar shared with mayDelete: {deleted:?}");

    // Not a discovery: it drops the collections that Stalwart creates unsubscribed.
    let calendar_left = owner
        .calendars(&owner_calendars_account)
        .expect("Calendar/get failed for the owner")
        .iter()
        .any(|held| held.id.as_ref() == Some(&calendar_id));
    let book_left = owner
        .address_books(&owner_contacts_account)
        .expect("AddressBook/get failed for the owner")
        .iter()
        .any(|held| held.id.as_ref() == Some(&book_id));

    // Clean up before the asserts, so that a failure leaves no collections.
    owner
        .address_book_destroy(&owner_contacts_account, &book_id)
        .expect("AddressBook/set destroy failed against the real server (cleanup)");
    if calendar_left {
        owner
            .calendar_destroy(&owner_calendars_account, &calendar_id)
            .expect("Calendar/set destroy failed against the real server (cleanup)");
    }

    assert!(
        calendar.remote_deletable,
        "the calendar was shared with mayDelete"
    );
    assert!(
        !book.remote_deletable,
        "the book was shared without mayDelete"
    );
    assert!(
        matches!(refused, Err(DeleteFailure::Client(_))),
        "the server must refuse deleting a book shared without mayDelete: {refused:?}"
    );
    assert!(book_left, "the book shared without mayDelete was destroyed");
    deleted.expect("deleting the calendar shared with mayDelete failed");
    assert!(
        !calendar_left,
        "the shared calendar is still in the owner's account"
    );
}
