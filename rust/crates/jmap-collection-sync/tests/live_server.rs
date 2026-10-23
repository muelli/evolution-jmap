// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `create_collection` and `delete_collection` against a real JMAP server.
//!
//! `ECollectionBackendClass::create_resource_sync` and `delete_resource_sync`
//! call these functions. `tests/create.rs` and `tests/delete.rs` test them
//! against the mock. These tests examine the account resolution through
//! [`CollectionLayout`], the dispatch by [`ChildKind`] and the [`Child`] that a
//! create makes from the response.
//!
//! ## Running it
//!
//! Use the environment of the live-server tests of `jmap-cal-sync` and
//! `jmap-book-sync` (refer to `docs/manual-test-live-server.md`). Set
//! `JMAP_LIVE_SERVER_URL`, `_WRITE_USER` and `_WRITE_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-collection-sync -- --ignored
//! ```
//!
//! This crate has no `live-server` feature: `#[ignore]` keeps the tests out of a
//! plain `cargo test`.
//!
//! The tests skip (and do not fail) when `JMAP_LIVE_SERVER_WRITE_USER` is unset.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{
    ChildKind, Fanout, Parts, Requested, create_collection, delete_collection,
};

/// A value unique to this run, so collections of other runs do not match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `jmap_test_support::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// Creates, discovers and deletes an address book and a calendar.
///
/// RFC 8620 §5.3 lets a `created` response omit properties that the server
/// did not set. Stalwart returns only `id`, so the checks examine the fallbacks
/// of `created_resource` for `name`, `myRights`, `isDefault` and `color`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn creating_then_deleting_a_collection_round_trips_through_the_real_server() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let suffix = unique_suffix();
    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let display_name = format!(
            "agent-collectionsync-{}-{suffix}",
            match kind {
                ChildKind::AddressBook => "book",
                ChildKind::Calendar => "cal",
            }
        );
        let requested = Requested {
            kind,
            display_name: display_name.clone(),
        };

        let created = create_collection(&client, &requested).unwrap_or_else(|error| {
            panic!("{kind:?}/set create failed against the real server: {error}")
        });
        assert_eq!(
            created.display_name, display_name,
            "the created {kind:?}'s display name should be the requested one, \
             whether the server echoed it back or left it out of the response"
        );
        assert!(
            !created.read_only,
            "a {kind:?} freshly created in a writable account should not come back \
             read-only just because the create response carried no myRights"
        );
        assert!(
            created.remote_deletable,
            "a {kind:?} freshly created in a writable account should not come back \
             undeletable just because the create response carried no myRights"
        );
        assert!(
            !created.is_default,
            "a freshly created {kind:?} should not be flagged the account default"
        );
        assert_eq!(
            created.color, None,
            "a {kind:?} created without a colour should show none, not a made-up one"
        );

        let fanout = Fanout::discover(&client, Parts::ALL).expect("discovery failed after create");
        let listed = match kind {
            ChildKind::AddressBook => &fanout.address_books,
            ChildKind::Calendar => &fanout.calendars,
        };
        assert!(
            listed
                .iter()
                .any(|resource| resource.id == created.collection_id),
            "the newly created {kind:?} should be listed by a fresh discovery"
        );

        let doomed = jmap_collection_sync::Doomed {
            kind,
            collection_id: created.collection_id.clone(),
        };
        delete_collection(&client, &doomed).unwrap_or_else(|error| {
            panic!("{kind:?}/set destroy failed against the real server: {error}")
        });

        let fanout = Fanout::discover(&client, Parts::ALL).expect("discovery failed after delete");
        let listed = match kind {
            ChildKind::AddressBook => &fanout.address_books,
            ChildKind::Calendar => &fanout.calendars,
        };
        assert!(
            !listed
                .iter()
                .any(|resource| resource.id == created.collection_id),
            "the deleted {kind:?} should no longer be listed"
        );
    }
}

/// Tests the "accepted as asked" name fallback of `created_resource` with padding.
///
/// Stalwart trims a padded `Mailbox` name but keeps the padding of address book
/// and calendar names. In both cases, the `created` response omits `name`.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn creating_a_collection_with_a_padded_name_round_trips_the_padding_unchanged() {
    let Some(client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let suffix = unique_suffix();
    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        let display_name = format!(
            "  agent-collectionsync-padded-{}-{suffix}  ",
            match kind {
                ChildKind::AddressBook => "book",
                ChildKind::Calendar => "cal",
            }
        );
        let requested = Requested {
            kind,
            display_name: display_name.clone(),
        };

        let created = create_collection(&client, &requested).unwrap_or_else(|error| {
            panic!("{kind:?}/set create failed against the real server: {error}")
        });
        assert_eq!(
            created.display_name, display_name,
            "the padded name should round-trip unchanged through the create response's \
             fallback, whether the server echoed it or left it out"
        );

        let fanout = Fanout::discover(&client, Parts::ALL).expect("discovery failed after create");
        let listed = match kind {
            ChildKind::AddressBook => &fanout.address_books,
            ChildKind::Calendar => &fanout.calendars,
        };
        let resource = listed
            .iter()
            .find(|resource| resource.id == created.collection_id)
            .unwrap_or_else(|| {
                panic!("the newly created {kind:?} should be listed by a fresh discovery")
            });
        assert_eq!(
            resource.name, display_name,
            "a fresh discovery should show the server's own stored name still padded, \
             confirming Stalwart does not trim {kind:?} names the way it trims Mailbox names"
        );

        let doomed = jmap_collection_sync::Doomed {
            kind,
            collection_id: created.collection_id.clone(),
        };
        delete_collection(&client, &doomed).unwrap_or_else(|error| {
            panic!("{kind:?}/set destroy failed against the real server: {error}")
        });
    }
}
