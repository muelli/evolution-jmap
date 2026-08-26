// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The half of a populate that deletes: which of the children this collection
// already has are removed, against real `ESource`s written the way this backend
// writes them.
//
// `Fanout::is_obsolete` is tested in `jmap-collection-sync` against resource id
// *strings*, and `child_source::apply` is tested here against the properties it
// writes. Neither covers the join, and the join is where the damage is:
// `e_source_remove_sync()` takes the child's uid, its `.source` file and its
// offline cache with it, so a resource id this backend fails to read back off a
// source it wrote itself is not a mislabelled row in the sidebar — it is a
// user's offline copy of an address book, deleted because a populate could not
// recognise its own child.
//
// So every source judged below is built by `apply` from a `Child`, which is
// exactly what `e_collection_backend_new_child` hands back and this backend
// then writes. Judging a hand-shaped source instead would prove that the rule
// holds for sources of the shape we imagined writing.

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

/// A child source in the state a populate leaves it in: created by EDS under a
/// resource id, then written by [`apply`]. `e_source_new_with_uid` with a NULL
/// D-Bus object is what EDS itself uses for a source read from a keyfile.
struct TestSource(*mut ESource);

impl TestSource {
    fn child(kind: ChildKind, collection: &str) -> Self {
        let child = child(kind, collection);
        let uid =
            CString::new(format!("jmap-{}", child.resource_id)).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");

        // SAFETY: a live source of this process's own.
        unsafe { apply(source, &child.settings(&connection())) }
            .expect("a child this backend wrote");
        Self(source)
    }

    /// A source with nothing on it: neither kind extension, no `[Resource]`.
    /// What a child of some *other* collection backend looks like to this one,
    /// and what EDS hands `dup_resource_id` for every file in the cache.
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
        // SAFETY: this holds the only reference.
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
    }
}

/// A login serving both collection kinds, discovered under `parts` — with the
/// vectors gated the way [`Fanout::discover`] would have gated them.
fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
    Fanout {
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

/// The sources [`obsolete`] picked, as the resource ids they were built under —
/// pointer identity is what the function answers in, but a failed assertion
/// that prints two addresses says nothing.
fn picked(fanout: &Fanout, children: &[&TestSource]) -> Vec<String> {
    let pointers: Vec<*mut ESource> = children.iter().map(|child| child.0).collect();
    // SAFETY: every source is alive for the call.
    let obsolete = unsafe { obsolete(fanout, &pointers) };
    obsolete
        .into_iter()
        .map(|source| {
            // SAFETY: one of the pointers handed in, still alive.
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
        // The reason the removal half exists: an address book deleted in the web UI
        // has to leave the sidebar, and the child that is still there must not.
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
        // The switched-off part lists nothing, so every child of that kind looks
        // like a collection the server no longer has. EDS's WebDAV backend removes
        // them; this one must not — the cache and the uid are the user's, and
        // switching contacts back on has to bring the same source back.
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
        // `e_collection_backend_list_*_sources` answers with what is in the
        // collection, and a source with no resource id of ours is one we have no
        // opinion about. Treating "I cannot read this" as "this is obsolete" would
        // delete a child on the strength of not understanding it.
        let foreign = TestSource::foreign();

        let fanout = fanout(Parts::ALL, &["AB1"], &["Cal1"]);

        assert!(picked(&fanout, &[&foreign]).is_empty());
    });
}

#[test]
fn an_address_book_and_a_calendar_with_one_id_are_judged_apart() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ids are scoped per account *and* per object type (RFC 8620 §1.2), so one
        // id naming both is the ordinary case on a server that numbers from one.
        // The kind is read off the source's own extension, and a judgement that
        // dropped it would remove the surviving collection's child.
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
        // A populate that stopped at the first would leave the rest behind for
        // another populate to find, and the sidebar half-cleaned in between.
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
        // These sources have no D-Bus object, so EDS refuses the removal — which
        // is the one branch of `e_source_remove_sync` this machine can drive, and
        // the branch a populate has to survive: the vfunc returns void, so an
        // unremovable child can only be reported, never raised.
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
        // One child that EDS will not part with must not keep the others.
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
