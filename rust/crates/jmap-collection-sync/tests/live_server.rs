// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_collection`/`delete_collection` against a real JMAP server — the
//! functions `ECollectionBackendClass::create_resource_sync`/
//! `delete_resource_sync` actually call, exercised end to end for the first
//! time.
//!
//! `jmap-client/tests/live_server.rs` already proves `AddressBook/set` and
//! `Calendar/set` round-trip through `Client` directly, but nothing there
//! drives this crate's own `create_collection`/`delete_collection` — the
//! account resolution through [`CollectionLayout`], the create/destroy
//! dispatch by [`ChildKind`], and the [`Child`] a create derives from what the
//! server answered with. Only `jmap-mockd` has ever exercised these functions
//! (`jmap-collection-sync/tests/{create,delete}.rs`); this file is their
//! live-server counterpart, following the same recipe as `jmap-cal-sync`'s and
//! `jmap-book-sync`'s.
//!
//! ## Running it
//!
//! Same environment as `jmap-cal-sync`'s and `jmap-book-sync`'s live-server
//! tests — see `docs/manual-test-live-server.md`. In short, with
//! `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/`_WRITE_PASSWORD` already set up for
//! `jmap-client`'s write-path tests:
//!
//! ```console
//! $ cargo test -p evolution-jmap-collection-sync -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like `jmap-cal-sync` and
//! `jmap-book-sync`, this crate has no such feature, and `#[ignore]` alone
//! already keeps it out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset — the same tolerance every write-path test in this repository gives
//! an unconfigured environment.

use std::env;

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{
    ChildKind, Fanout, Parts, Requested, create_collection, delete_collection,
};

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover collection can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
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

/// Creates an address book and a calendar via `create_collection`, confirms
/// each is listed by a fresh `Fanout::discover`, then destroys both via
/// `delete_collection` and confirms neither is listed anymore.
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
