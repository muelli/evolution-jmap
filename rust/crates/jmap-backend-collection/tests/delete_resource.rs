// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the deletion of a collection on the server from the child
// `ESource` that EDS gives to `delete_resource_sync`. The tests use real
// `ESource`s and a real `jmap-mockd`.
//
// `tests/delete.rs` in `jmap-collection-sync` tests the server request without
// an `ESource`. These tests cover the two ends that need the headers. Both ends
// answer one question: **which source can this backend delete**.
//
// The safety of the feature depends on that answer. `remote-deletable` makes
// Evolution show the menu item. The vfunc gets the source that the user
// clicked. For a source that this backend did not write, both ends must answer
// "not mine". Otherwise, the server gets a destroy for an id from the keyfile
// of a different owner.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceRegistryServer, e_server_side_source_new, e_source_address_book_get_type,
    e_source_calendar_get_type, e_source_get_extension, e_source_get_remote_deletable,
    e_source_has_extension, e_source_new_with_uid, e_source_registry_server_new,
    g_file_new_for_path,
};
use glib_sys::GFALSE;
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_collection::child_source::apply;
use jmap_backend_collection::create_resource::create_on_server;
use jmap_backend_collection::delete_resource::{delete_on_server, doomed_of, offer_deletion};
use jmap_backend_core::source::ConnectTarget;
use jmap_client::{Client, Credentials};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, Doomed, Fanout, Parts, Requested};
use jmap_mock::MockServer;
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

/// A different file name for each server-side source. EDS derives the
/// `ESource` uid from the file name, so two sources in one test process never
/// share a uid.
static NEXT: AtomicU32 = AtomicU32::new(0);
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The connection of the account that owns this collection. The settings of
/// each child come from it.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

fn child(kind: ChildKind, collection_id: &str) -> Child {
    Child {
        resource_id: kind.resource_id(&Id::new(collection_id)),
        kind,
        display_name: "Personal".to_owned(),
        account_id: Id::new("A1"),
        collection_id: Id::new(collection_id),
        is_default: false,
        color: None,
        read_only: false,
        remote_deletable: true,
    }
}

/// A plain `ESource`, for the *read* half. `doomed_of` only reads extensions
/// from a source and does not write `EServerSideSource` properties.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        // SAFETY: these functions take no arguments. `e_source_get_extension`
        // cannot find an extension class whose type is not yet referenced.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, the default
        // main context and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// A child of this account, written through `child_source::apply` as a
    /// populate writes it. Thus the test reads back the true output of this
    /// backend.
    fn child_of(kind: ChildKind, collection_id: &str) -> Self {
        let source = Self::new(&format!("jmap-child-{collection_id}"));
        // SAFETY: a live source.
        unsafe {
            apply(
                source.0,
                &child(kind, collection_id).settings(&connection()),
            )
        }
        .expect("a child this backend wrote");
        source
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    fn doomed(&self) -> Option<Doomed> {
        // SAFETY: a live source this test holds a reference to.
        unsafe { doomed_of(self.0) }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A real `EServerSideSource`. In `evolution-source-registry`, each child of a
/// collection is one, and `e_server_side_source_set_remote_deletable` writes only
/// to this type. It is built without a daemon, as in `tests/create_resource.rs`
/// and `tests/recipe.rs`.
struct ServerSide {
    /// Kept for the life of the source. `e_server_side_source_new` keeps only a
    /// weak reference to the server. All `EServerSideSource` setters refuse a
    /// source whose server is finalized.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl ServerSide {
    fn new() -> Self {
        // SAFETY: these functions take no arguments, as in `Source::new`.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let path = std::env::temp_dir().join(format!(
            "jmap-deletable-{}-{}.source",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("no NUL in a temp path");
        let mut error = ptr::null_mut();

        // SAFETY: the constructor takes no arguments. The path is
        // NUL-terminated and the callee copies it. This code owns the GFile and
        // releases it below.
        let (server, source) = unsafe {
            let server = e_source_registry_server_new().cast::<ESourceRegistryServer>();
            let file = g_file_new_for_path(path.as_ptr());
            let source = e_server_side_source_new(server, file, &mut error);
            g_object_unref(file.cast());
            (server, source)
        };
        assert!(
            !source.is_null(),
            "e_server_side_source_new failed: {}",
            // SAFETY: a NULL return means the GError was set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );

        Self { server, source }
    }

    /// The same source, written as a child of this account. Each child is in
    /// this state when `child_added` fires for it.
    fn child_of(kind: ChildKind, collection_id: &str) -> Self {
        let held = Self::new();
        // SAFETY: a live source.
        unsafe {
            apply(
                held.source,
                &child(kind, collection_id).settings(&connection()),
            )
        }
        .expect("a child this backend wrote");
        held
    }

    fn offered_for_deletion(&self) -> bool {
        // SAFETY: a live source this test holds a reference to.
        unsafe { e_source_get_remote_deletable(self.source) != GFALSE }
    }
}

impl Drop for ServerSide {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference to each object. The
        // source is released first because it holds a weak reference to the
        // server.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast::<GObject>());
        }
    }
}

#[test]
fn an_address_book_child_names_the_address_book_it_stands_for() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = Source::child_of(ChildKind::AddressBook, "AB1");

        assert_eq!(
            source.doomed(),
            Some(Doomed {
                kind: ChildKind::AddressBook,
                collection_id: Id::new("AB1"),
            })
        );
    });
}

#[test]
fn a_calendar_child_names_the_calendar_it_stands_for() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The result is the pair, not only the id. An address book and a
        // calendar can have the same id (RFC 8620 §1.2). The kind selects the
        // `/set` call that destroys the collection.
        let source = Source::child_of(ChildKind::Calendar, "AB1");

        assert_eq!(
            source.doomed(),
            Some(Doomed {
                kind: ChildKind::Calendar,
                collection_id: Id::new("AB1"),
            })
        );
    });
}

#[test]
fn a_source_that_is_not_a_child_of_this_backend_names_nothing() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The feature depends on this rule. `delete_resource_sync` gets the
        // source that the user clicked "Delete" on. A source without our
        // `[Resource] Identity` has no collection to destroy. Examples are a mail
        // source of this account, a child of a different collection backend, or
        // a file that a person edited. A value other than `None` sends a destroy
        // for an id from the keyfile of a different owner.
        let bare = Source::new("jmap-not-a-child");

        assert_eq!(bare.doomed(), None);
    });
}

#[test]
fn a_child_that_names_a_kind_and_no_identity_names_nothing() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A half-written source is not ours. The address book extension
        // identifies the `/set` call, but not the object.
        let source = Source::new("jmap-half-written");
        // SAFETY: a live source and a header constant. The creation of the
        // extension puts the source in the state under test.
        unsafe {
            assert!(
                !e_source_get_extension(source.0, E_SOURCE_EXTENSION_ADDRESS_BOOK.as_ptr())
                    .is_null()
            );
        }

        assert_eq!(source.doomed(), None);
    });
}

#[test]
fn reading_the_doomed_collection_does_not_give_the_source_an_extension_it_lacked() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_source_get_extension` *creates* an extension that it cannot find.
        // This vfunc gets sources that other parts of Evolution own. A read of
        // `[Resource]` would write a group into each of them.
        let bare = Source::new("jmap-untouched");

        assert_eq!(bare.doomed(), None);
        assert!(!bare.has_extension(E_SOURCE_EXTENSION_RESOURCE));
        assert!(!bare.has_extension(E_SOURCE_EXTENSION_ADDRESS_BOOK));
        assert!(!bare.has_extension(E_SOURCE_EXTENSION_CALENDAR));
    });
}

#[test]
fn a_child_of_this_backend_is_offered_for_deletion() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This flag makes Evolution show "Delete" on a JMAP address book.
        // `server_side_source_remote_delete_sync()` refuses unless the child has
        // `remote-deletable` set. Without the flag, the vfunc is unreachable.
        let child = ServerSide::child_of(ChildKind::AddressBook, "AB1");
        assert!(
            !child.offered_for_deletion(),
            "an EServerSideSource is not deletable until something says so"
        );

        // SAFETY: a live `EServerSideSource` this test holds a reference to.
        let offered = unsafe { offer_deletion(child.source) };

        assert!(offered);
        assert!(child.offered_for_deletion());
    });
}

#[test]
fn a_source_this_backend_did_not_write_is_not_offered_for_deletion() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `child_added` fires for each source under this collection, mail
        // sources included. This backend cannot delete those sources, so it must
        // not show "Delete" for them. Otherwise, `delete_resource_sync` gets a
        // source that it cannot interpret.
        let stranger = ServerSide::new();

        // SAFETY: a live `EServerSideSource` this test holds a reference to.
        let offered = unsafe { offer_deletion(stranger.source) };

        assert!(!offered);
        assert!(!stranger.offered_for_deletion());
    });
}

#[test]
fn a_shared_child_is_left_to_its_rights_rather_than_offered_for_deletion() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The deletion of a shared collection deletes the collection of the
        // owner for all users. A cached shared child gets to `child_added` before
        // contact with the server. Thus the permissive default of
        // `offer_deletion` must not apply to it. Only a fan-out, which reads
        // `myRights.mayDelete`, can offer the deletion.
        let held = ServerSide::new();
        let shared = Child {
            resource_id: ChildKind::Calendar.shared_resource_id(&Id::new("A2"), &Id::new("C1")),
            ..child(ChildKind::Calendar, "C1")
        };
        // SAFETY: a live source.
        unsafe { apply(held.source, &shared.settings(&connection())) }
            .expect("a shared child this backend wrote");

        // SAFETY: a live `EServerSideSource` this test holds a reference to.
        let offered = unsafe { offer_deletion(held.source) };

        assert!(!offered);
        assert!(!held.offered_for_deletion());
        // The child is still ours. After a fan-out offers the deletion, the
        // delete knows the collection and the account.
        // SAFETY: as above.
        let doomed = unsafe { doomed_of(held.source) }.expect("a child this backend wrote");
        assert_eq!(doomed.collection_id, Id::new("A2/C1"));
    });
}

#[test]
fn deleting_a_child_takes_its_collection_off_the_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The full vfunc without the instance. The test creates a collection on
        // a real mock and writes a child source for it as a fan-out does. Then
        // it deletes the collection from the data in that source.
        let server = MockServer::builder().start();
        let target = ConnectTarget::Origin(server.origin().to_owned());
        let created = create_on_server(
            &target,
            false,
            Credentials::none(),
            &Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            },
        )
        .expect("the mock creates address books");

        let source = Source::child_of(ChildKind::AddressBook, created.collection_id.as_str());
        let doomed = source.doomed().expect("a child this backend wrote");

        delete_on_server(&target, false, Credentials::none(), &doomed)
            .expect("the mock destroys them too");

        let client = Client::connect(server.origin(), Credentials::none())
            .expect("the mock serves a session document");
        let fanout = Fanout::discover(&client, Parts::ALL).expect("the mock answers");
        assert!(
            !fanout
                .address_books
                .iter()
                .any(|book| book.id == created.collection_id),
            "the address book the child stood for is still on the server"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_delete_resource_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
