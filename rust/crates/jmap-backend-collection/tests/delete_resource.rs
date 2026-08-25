// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Deleting a collection from the server, from the child `ESource` EDS hands
// `delete_resource_sync` — against real `ESource`s and a real `jmap-mockd`.
//
// `jmap-collection-sync`'s `tests/delete.rs` covers what the delete *asks the
// server*, with no `ESource` in it. This covers the two ends that need the
// headers, and they are both about the same question: **which source may be
// deleted at all**.
//
// That question is the whole safety of the feature. `remote-deletable` is what
// makes Evolution offer the menu item, and the vfunc behind it is handed
// whatever source the user clicked on. A source this backend did not write must
// answer "not mine" to both — because the alternative is not an error message,
// it is a destroy sent to a JMAP server naming an id read out of somebody else's
// keyfile.

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

/// A distinct file name per server-side source, so two of them in one test
/// process never share an `ESource` uid — EDS derives the uid from the file
/// name.
static NEXT: AtomicU32 = AtomicU32::new(0);
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The account this collection belongs to, as a child's settings were written
/// from.
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
    }
}

/// A plain `ESource` — what the *read* half needs, since `doomed_of` only ever
/// reads extensions off a source and never writes an `EServerSideSource`
/// property.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        // SAFETY: no arguments; `e_source_get_extension` cannot find an
        // extension class whose type nothing has referenced yet.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// A child of this account, written the way a populate writes one — through
    /// `child_source::apply`, so what is read back is what this backend really
    /// put there.
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
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A real `EServerSideSource`, which is what every child of a collection is in
/// `evolution-source-registry` — and the only kind of source
/// `e_server_side_source_set_remote_deletable` will write to. Built the
/// daemon-free way `tests/create_resource.rs` and `tests/recipe.rs` build one.
struct ServerSide {
    /// Held for as long as the source: `e_server_side_source_new` keeps only a
    /// weak reference to it, and a source whose server has been finalized is one
    /// every `EServerSideSource` setter refuses.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl ServerSide {
    fn new() -> Self {
        // SAFETY: no arguments; as in `Source::new`.
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

        // SAFETY: the constructor takes no arguments; the path is
        // NUL-terminated and copied; the GFile is owned here and released below.
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

    /// The same source, written as a child of this account — the state every
    /// child is in by the time `child_added` fires for it.
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
        // SAFETY: this holds the only reference to each; the source is released
        // first because it holds a weak reference to the server.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast::<GObject>());
        }
    }
}

#[test]
fn an_address_book_child_names_the_address_book_it_stands_for() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let source = Source::child_of(ChildKind::AddressBook, "AB1");

    assert_eq!(
        source.doomed(),
        Some(Doomed {
            kind: ChildKind::AddressBook,
            collection_id: Id::new("AB1"),
        })
    );
}

#[test]
fn a_calendar_child_names_the_calendar_it_stands_for() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The pair, not the id: a server that numbers its objects from one gives an
    // address book and a calendar the same id (RFC 8620 §1.2), and the kind is
    // what decides which `/set` call destroys this one.
    let source = Source::child_of(ChildKind::Calendar, "AB1");

    assert_eq!(
        source.doomed(),
        Some(Doomed {
            kind: ChildKind::Calendar,
            collection_id: Id::new("AB1"),
        })
    );
}

#[test]
fn a_source_that_is_not_a_child_of_this_backend_names_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The rule the whole feature rests on. `delete_resource_sync` is handed
    // whatever source the user clicked "Delete" on, and a source with no
    // `[Resource] Identity` of ours — a mail source of this account, a child of
    // another collection backend, a hand-edited file — has no collection to
    // destroy. Answering anything but `None` here sends a destroy naming an id
    // read out of somebody else's keyfile.
    let bare = Source::new("jmap-not-a-child");

    assert_eq!(bare.doomed(), None);
}

#[test]
fn a_child_that_names_a_kind_and_no_identity_names_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Halfway is still not ours: the address book extension alone says which
    // `/set` call would be used and nothing about which object.
    let source = Source::new("jmap-half-written");
    // SAFETY: a live source and a header constant; creating the extension is
    // exactly what puts the source in the state under test.
    unsafe {
        assert!(
            !e_source_get_extension(source.0, E_SOURCE_EXTENSION_ADDRESS_BOOK.as_ptr()).is_null()
        );
    }

    assert_eq!(source.doomed(), None);
}

#[test]
fn reading_the_doomed_collection_does_not_give_the_source_an_extension_it_lacked() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `e_source_get_extension` *creates* what it cannot find, and this vfunc is
    // handed sources belonging to other parts of Evolution. A read that reached
    // for `[Resource]` would write a group into every one of them.
    let bare = Source::new("jmap-untouched");

    assert_eq!(bare.doomed(), None);
    assert!(!bare.has_extension(E_SOURCE_EXTENSION_RESOURCE));
    assert!(!bare.has_extension(E_SOURCE_EXTENSION_ADDRESS_BOOK));
    assert!(!bare.has_extension(E_SOURCE_EXTENSION_CALENDAR));
}

#[test]
fn a_child_of_this_backend_is_offered_for_deletion() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // What makes Evolution show "Delete" on a JMAP address book at all:
    // `server_side_source_remote_delete_sync()` refuses outright unless the
    // child's own `remote-deletable` is set, so without this the vfunc below is
    // unreachable dead code.
    let child = ServerSide::child_of(ChildKind::AddressBook, "AB1");
    assert!(
        !child.offered_for_deletion(),
        "an EServerSideSource is not deletable until something says so"
    );

    // SAFETY: a live `EServerSideSource` this test holds a reference to.
    let offered = unsafe { offer_deletion(child.source) };

    assert!(offered);
    assert!(child.offered_for_deletion());
}

#[test]
fn a_source_this_backend_did_not_write_is_not_offered_for_deletion() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `child_added` fires for every source under this collection, mail sources
    // included. Offering deletion on one of those would put a "Delete" in front
    // of the user that this backend cannot honour — and `delete_resource_sync`
    // would then be asked about a source it can make no sense of.
    let stranger = ServerSide::new();

    // SAFETY: a live `EServerSideSource` this test holds a reference to.
    let offered = unsafe { offer_deletion(stranger.source) };

    assert!(!offered);
    assert!(!stranger.offered_for_deletion());
}

#[test]
fn deleting_a_child_takes_its_collection_off_the_server() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The whole vfunc minus the instance: a collection created on a real mock,
    // a child source written for it the way a fan-out writes one, and the
    // delete driven from what that source says about itself.
    let server = MockServer::builder().start();
    let target = ConnectTarget::Origin(server.origin().to_owned());
    let created = create_on_server(
        &target,
        Credentials::none(),
        &Requested {
            kind: ChildKind::AddressBook,
            display_name: "Work".to_owned(),
        },
    )
    .expect("the mock creates address books");

    let source = Source::child_of(ChildKind::AddressBook, created.collection_id.as_str());
    let doomed = source.doomed().expect("a child this backend wrote");

    delete_on_server(&target, Credentials::none(), &doomed).expect("the mock destroys them too");

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
}
