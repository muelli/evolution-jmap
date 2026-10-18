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
//! This file confirms that `Fanout::discover`, which a real populate calls,
//! finds a shared collection through the owner's `isPersonal: false` account
//! in the recipient's session, and lets it go once the grant is withdrawn.
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

/// Creates an address book and a calendar on the owner account, shares the
/// book writable and the calendar read-only with the recipient account, and
/// follows them through the recipient's own `Fanout::discover`:
///
/// 1. Freshly shared, both carry `isSubscribed: false` on Stalwart (the
///    drafts' initial state for a share), and the recipient cannot subscribe
///    to the read-only calendar (`forbidden`). Both are children all the
///    same, under `<kind>:<owner account>/<collection>`, named with the
///    owner's account name, the calendar read-only, and neither retargets
///    the recipient's own resolved accounts.
/// 2. Once the owner withdraws both grants, both children are obsolete.
///
/// Destroys both collections at the end.
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

    // A fresh client: the session document is fetched once per connection,
    // and the owner's account only appears in it after the grant.
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
