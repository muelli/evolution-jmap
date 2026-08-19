// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Reading a child source's resource id back off the source, against real
// `ESource`s built with the EDS setters rather than against a hand-made struct.
//
// This is the one vfunc in the collection backend whose wrong answer is not a
// failed operation: `collection_backend_load_resources()` deletes the cache
// file of every child that answers `NULL`, and of every child whose answer a
// previously-loaded child already gave. So the cases below are not edge cases,
// they are the list of ways this backend can lose a user's offline data.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_has_extension, e_source_new_with_uid,
    e_source_resource_set_identity,
};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_collection::resource_id::{KIND_EXTENSIONS, resource_id_of};
use jmap_collection_sync::child_source::{Connection, EXTENSION_ADDRESS_BOOK, EXTENSION_CALENDAR};
use jmap_collection_sync::{Child, ChildKind};
use jmap_proto::Id;

/// An `ESource` that is not backed by the registry — `e_source_new_with_uid`
/// with a NULL D-Bus object is what EDS itself uses for a source read from a
/// keyfile, so the extension machinery behaves as it does in a backend.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` finds an extension class by walking the
        // children of `E_TYPE_SOURCE_EXTENSION`, so a type nothing has
        // referenced yet is one the source will not find. Under EDS the
        // libraries are loaded and their types registered long before any
        // source exists; in a test binary that only names the constants,
        // touching the two accessors is what stands in for it.
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let uid = CString::new("jmap-collection-child").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// Gives the source the extension named by `group` — which is what makes it
    /// an address book or a calendar to EDS, and what a `.source` file spells as
    /// a keyfile group.
    fn with_extension(self, group: &str) -> Self {
        let group = CString::new(group).expect("no NUL in an extension name");
        // SAFETY: a live source and a NUL-terminated name; the extension is
        // created on demand and owned by the source.
        let extension = unsafe { e_source_get_extension(self.0, group.as_ptr()) };
        assert!(!extension.is_null(), "EDS knows no such extension");
        self
    }

    fn with_identity(self, identity: &str) -> Self {
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: as above, and the setter copies the string.
        unsafe {
            let resource: *mut ESourceResource =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast();
            e_source_resource_set_identity(resource, identity.as_ptr());
        }
        self
    }

    fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    fn resource_id(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { resource_id_of(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: the reference `e_source_new_with_uid` returned is given back
        // exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: None,
        user: None,
        auth_method: None,
        secure: true,
    }
}

fn child(kind: ChildKind, collection: &str) -> Child {
    Child {
        resource_id: kind.resource_id(&Id::new(collection)),
        kind,
        display_name: "Whatever".to_owned(),
        account_id: Id::new("A1"),
        collection_id: Id::new(collection),
        is_default: false,
        color: None,
        read_only: false,
    }
}

#[test]
fn the_extension_names_are_the_ones_eds_defines() {
    // `jmap-collection-sync` spells them as string literals, because it must
    // build on a machine with no EDS headers. They are the argument to
    // `e_source_get_extension` and the group of a `.source` file, and a typo in
    // one is not an error — it is a second, empty extension nothing reads. This
    // crate is the only place both spellings are in scope, so it is the only
    // place the two can be held against each other.
    assert_eq!(
        E_SOURCE_EXTENSION_ADDRESS_BOOK.to_str(),
        Ok(EXTENSION_ADDRESS_BOOK)
    );
    assert_eq!(E_SOURCE_EXTENSION_CALENDAR.to_str(), Ok(EXTENSION_CALENDAR));

    // And the table this crate dispatches on says the same, pair by pair, so a
    // kind added later cannot bring a third spelling with it.
    assert_eq!(
        KIND_EXTENSIONS
            .iter()
            .map(|(defined, ours)| (defined.to_str().expect("ASCII"), *ours))
            .collect::<Vec<_>>(),
        [
            (EXTENSION_ADDRESS_BOOK, EXTENSION_ADDRESS_BOOK),
            (EXTENSION_CALENDAR, EXTENSION_CALENDAR),
        ]
    );
}

#[test]
fn an_address_book_child_is_read_back_as_the_resource_id_it_was_created_under() {
    let source = TestSource::new()
        .with_extension(EXTENSION_ADDRESS_BOOK)
        .with_identity("AB1");

    assert_eq!(source.resource_id().as_deref(), Some("addressbook:AB1"));
}

#[test]
fn a_calendar_child_is_read_back_as_a_calendar() {
    let source = TestSource::new()
        .with_extension(EXTENSION_CALENDAR)
        .with_identity("Cal1");

    assert_eq!(source.resource_id().as_deref(), Some("calendar:Cal1"));
}

#[test]
fn the_two_kinds_of_child_of_one_jmap_id_do_not_share_a_resource_id() {
    // The reason this vfunc is overridden at all. EDS's own implementation
    // returns `[Resource] Identity` verbatim, and a JMAP account may hand out
    // the same id for an address book and a calendar — RFC 8620 ids are unique
    // per data type, not per account. Two children with one resource id is one
    // child: `collection_backend_load_resources()` keeps the first and deletes
    // the second's cache file as redundant.
    let book = TestSource::new()
        .with_extension(EXTENSION_ADDRESS_BOOK)
        .with_identity("X1");
    let calendar = TestSource::new()
        .with_extension(EXTENSION_CALENDAR)
        .with_identity("X1");

    assert_ne!(book.resource_id(), calendar.resource_id());
    assert!(book.resource_id().is_some());
    assert!(calendar.resource_id().is_some());
}

#[test]
fn every_child_this_backend_describes_reads_back_as_itself() {
    // The round trip that has to hold for every child `populate` will create:
    // what `Child::settings` writes is what this reads. A child of ours that
    // did not round-trip would be deleted from the cache on the next start.
    for (kind, collection) in [
        (ChildKind::AddressBook, "AB1"),
        (ChildKind::Calendar, "Cal1"),
        (ChildKind::AddressBook, "Shared"),
        (ChildKind::Calendar, "Shared"),
    ] {
        let child = child(kind, collection);
        let settings = child.settings(&connection());
        let identity = settings
            .iter()
            .find(|setting| (setting.group, setting.key) == ("Resource", "Identity"))
            .expect("every child is written with an identity");

        let source = TestSource::new()
            .with_extension(kind.extension())
            .with_identity(&identity.value);

        assert_eq!(source.resource_id().as_deref(), Some(&*child.resource_id));
    }
}

#[test]
fn a_source_of_no_kind_this_backend_creates_is_not_claimed() {
    // `dup_resource_id` is asked about every `.source` in the backend's cache
    // directory. Claiming one that is not ours would put a foreign source in
    // EDS's unclaimed-resources table under a name a real child may also
    // answer to — and the loser of that collision is deleted.
    let source = TestSource::new()
        .with_extension("Mail Account")
        .with_identity("A1");

    assert_eq!(source.resource_id(), None);
}

#[test]
fn a_child_with_no_identity_is_not_claimed_and_is_not_given_one() {
    // Two things at once, and the second is the subtle one:
    // `e_source_get_extension` *creates* the extension it is asked for. Reading
    // the identity that way would give every source that reached this vfunc an
    // empty `[Resource]` group — including sources belonging to other backends
    // — so the extension has to be tested for before it is read.
    let source = TestSource::new().with_extension(EXTENSION_ADDRESS_BOOK);

    assert_eq!(source.resource_id(), None);
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_RESOURCE),
        "reading the identity added a [Resource] extension to a source that had none"
    );
}

#[test]
fn an_empty_identity_is_no_identity() {
    // A `.source` carrying `Identity=` reads back as the empty string, and an
    // empty JMAP id is not an id. Answering `"addressbook:"` would be a name
    // every such child shares.
    let source = TestSource::new()
        .with_extension(EXTENSION_ADDRESS_BOOK)
        .with_identity("");

    assert_eq!(source.resource_id(), None);
}

#[test]
fn a_null_source_is_not_claimed_rather_than_dereferenced() {
    // EDS never passes one; the vfunc is a C entry point and this is the
    // cheapest half of not trusting the caller.
    // SAFETY: NULL is an explicitly permitted argument.
    assert_eq!(unsafe { resource_id_of(ptr::null_mut()) }, None);
}
