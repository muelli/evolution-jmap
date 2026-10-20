// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the removal half of a populate: which existing children of this
// collection are removed. The tests use real `ESource`s that this backend
// writes.
//
// `jmap-collection-sync` tests `Fanout::is_obsolete` with resource id
// *strings*. Tests in this crate examine the properties that
// `child_source::apply` writes. Neither covers the join of the two, and errors
// in the join cause data loss. `e_source_remove_sync()` removes the uid, the
// `.source` file and the offline cache of the child. If this backend cannot
// read a resource id from its own source, a populate deletes the offline copy
// of an address book of the user.
//
// Thus `apply` builds each source below from a `Child`, as this backend writes
// the source after `e_collection_backend_new_child`. A source made by hand
// shows only that the rule is correct for sources of the expected shape.

use std::ffi::CString;
use std::ptr;

use eds_sys::{ESource, e_source_new_with_uid};
use gobject_sys::g_object_unref;
use jmap_backend_collection::child_source::apply;
use jmap_backend_collection::removal::{obsolete, remove_obsolete};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{
    Child, ChildKind, CollectionLayout, Fanout, Parts, Resource, ServiceAccount,
};
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A child source in the state after a populate. EDS creates it under a
/// resource id, then [`apply`] writes it. `e_source_new_with_uid` makes it
/// without the registry.
struct TestSource(*mut ESource);

impl TestSource {
    fn child(kind: ChildKind, collection: &str) -> Self {
        let child = child(kind, collection);
        let uid =
            CString::new(format!("jmap-{}", child.resource_id)).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, the default
        // main context and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");

        // SAFETY: a live source that this process owns.
        unsafe { apply(source, &child.settings(&connection())) }
            .expect("a child this backend wrote");
        Self(source)
    }

    /// A source with no kind extension and no `[Resource]`. A child of a
    /// *different* collection backend has this appearance. EDS gives sources of
    /// this type to `dup_resource_id` for files in the cache.
    fn foreign() -> Self {
        let uid = CString::new("not-ours").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: as above.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: None,
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

fn child(kind: ChildKind, collection: &str) -> Child {
    Child {
        resource_id: kind.resource_id(&Id::new(collection)),
        kind,
        display_name: format!("Collection {collection}"),
        account_id: Id::new("A1"),
        collection_id: Id::new(collection),
        is_default: false,
        color: None,
        read_only: false,
        remote_deletable: true,
    }
}

fn account() -> ServiceAccount {
    ServiceAccount {
        id: Id::new("A1"),
        name: "Vera Vibes".to_owned(),
        read_only: false,
    }
}

fn resource(id: &str) -> Resource {
    Resource {
        id: Id::new(id),
        name: format!("Collection {id}"),
        is_default: false,
        color: None,
        writable: None,
        deletable: None,
    }
}

/// A login that serves both collection kinds, discovered under `parts`. The
/// vectors are filtered as [`Fanout::discover`] filters them.
fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
    Fanout {
        shared: Vec::new(),
        parts,
        layout: CollectionLayout {
            mail: None,
            contacts: Some(account()),
            calendars: Some(account()),
        },
        address_books: address_books
            .iter()
            .copied()
            .filter(|_| parts.contacts)
            .map(resource)
            .collect(),
        calendars: calendars
            .iter()
            .copied()
            .filter(|_| parts.calendars)
            .map(resource)
            .collect(),
    }
}

/// The sources that [`obsolete`] selected, as their resource ids. The function
/// returns pointers, but a failed assertion that shows two addresses gives no
/// useful data.
fn picked(fanout: &Fanout, children: &[&TestSource]) -> Vec<String> {
    let pointers: Vec<*mut ESource> = children.iter().map(|child| child.0).collect();
    // SAFETY: every source is alive for the call.
    let obsolete = unsafe { obsolete(fanout, &pointers) };
    obsolete
        .into_iter()
        .map(|source| {
            // SAFETY: one of the input pointers, still alive.
            unsafe { jmap_backend_collection::resource_id::resource_id_of(source) }
                .expect("a source with no resource id was picked for removal")
        })
        .collect()
}

/// [`remove_obsolete`] over the same children.
fn removals(
    fanout: &Fanout,
    children: &[&TestSource],
) -> Vec<jmap_backend_collection::removal::NotRemoved> {
    let pointers: Vec<*mut ESource> = children.iter().map(|child| child.0).collect();
    // SAFETY: every source is alive for the call.
    unsafe { remove_obsolete(fanout, &pointers) }
}

#[test]
fn a_child_whose_collection_the_server_no_longer_lists_is_the_one_removed() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is the purpose of the removal half. An address book that the web
        // UI deleted must leave the sidebar. The other child must stay.
        let gone = TestSource::child(ChildKind::AddressBook, "AB2");
        let still_there = TestSource::child(ChildKind::AddressBook, "AB1");

        let fanout = fanout(Parts::ALL, &["AB1"], &[]);

        assert_eq!(picked(&fanout, &[&gone, &still_there]), ["addressbook:AB2"]);
    });
}

#[test]
fn a_child_of_a_part_the_user_switched_off_is_kept() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A disabled part lists nothing. Thus each child of that kind seems to
        // be a collection that the server does not have. The WebDAV backend of
        // EDS removes these children, but this backend must not. The cache and
        // the uid belong to the user. When the user enables contacts again, the
        // same source must come back.
        let dormant = TestSource::child(ChildKind::AddressBook, "AB1");

        let contacts_off = fanout(
            Parts {
                contacts: false,
                ..Parts::ALL
            },
            &["AB1"],
            &[],
        );

        assert!(picked(&contacts_off, &[&dormant]).is_empty());
    });
}

#[test]
fn a_source_this_backend_did_not_write_is_never_removed() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_list_*_sources` returns all sources in the
        // collection. This backend makes no decision about a source without our
        // resource id. A source that this backend cannot read is not obsolete.
        // Otherwise, a removal occurs only because the source is unknown.
        let foreign = TestSource::foreign();

        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        assert!(picked(&fanout, &[&foreign]).is_empty());
    });
}

#[test]
fn an_address_book_and_a_calendar_with_one_id_are_judged_apart() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ids are unique for each account *and* each object type (RFC 8620
        // §1.2). Thus one id for both kinds is normal on a server that counts
        // from one. The kind comes from the extension of the source. Without the
        // kind, the decision removes the child of the remaining collection.
        let book = TestSource::child(ChildKind::AddressBook, "X1");
        let calendar = TestSource::child(ChildKind::Calendar, "X1");

        let calendars_only = fanout(Parts::ALL, &[], &["X1"]);

        assert_eq!(
            picked(&calendars_only, &[&book, &calendar]),
            ["addressbook:X1"]
        );
    });
}

#[test]
fn every_obsolete_child_is_picked_and_in_the_order_it_was_given_in() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // If a populate stops at the first child, the others stay until the next
        // populate. Until then, the sidebar is only partly correct.
        let first = TestSource::child(ChildKind::AddressBook, "AB1");
        let kept = TestSource::child(ChildKind::AddressBook, "AB2");
        let second = TestSource::child(ChildKind::Calendar, "Cal1");

        let fanout = fanout(Parts::ALL, &["AB2"], &[]);

        assert_eq!(
            picked(&fanout, &[&first, &kept, &second]),
            ["addressbook:AB1", "calendar:Cal1"]
        );
    });
}

#[test]
fn a_child_that_cannot_be_removed_is_reported_with_what_eds_said() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // These sources have no D-Bus object, so EDS refuses the removal. This
        // test can only run this branch of `e_source_remove_sync`. A populate
        // must continue after this refusal. The vfunc returns void, so the
        // backend can only report the failed removal, not return an error.
        let gone = TestSource::child(ChildKind::AddressBook, "AB2");
        let still_there = TestSource::child(ChildKind::AddressBook, "AB1");

        let fanout = fanout(Parts::ALL, &["AB1"], &[]);

        let failures = removals(&fanout, &[&gone, &still_there]);
        assert_eq!(failures.len(), 1, "only the obsolete child is removed");
        assert_eq!(failures[0].resource_id, "addressbook:AB2");
        assert!(
            !failures[0].message.is_empty(),
            "a refusal with no message leaves nothing to log"
        );
    });
}

#[test]
fn a_removal_that_failed_does_not_stop_the_ones_after_it() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // If EDS refuses to remove one child, the removal of the others must
        // continue.
        let first = TestSource::child(ChildKind::AddressBook, "AB1");
        let second = TestSource::child(ChildKind::Calendar, "Cal1");

        let fanout = fanout(Parts::ALL, &[], &[]);

        let failures = removals(&fanout, &[&first, &second]);
        let attempted: Vec<&str> = failures
            .iter()
            .map(|failure| failure.resource_id.as_str())
            .collect();
        assert_eq!(attempted, ["addressbook:AB1", "calendar:Cal1"]);
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_removal_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
