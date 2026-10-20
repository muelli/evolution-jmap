// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the creation of a collection on the server from the scratch
// `ESource` that EDS gives to `create_resource_sync`. The tests use a real
// `EServerSideSource` and a real `jmap-mockd`.
//
// `tests/create.rs` in `jmap-collection-sync` tests the server request without
// an `ESource`. These tests cover the two ends that need the headers. The first
// end is the kind and name that this backend reads from the scratch source. The
// second end is the source that the create leaves. `resource_id_of` must read
// that source back as this child, or the next populate creates a second source
// for the same collection.
//
// The scratch source comes from `e_server_side_source_new` on a `GFile`, as in
// EDS. This is the daemon-free half of `evolution-source-registry` that
// `tests/recipe.rs` uses. Two of the four things that `adopt_created` writes
// are `EServerSideSource` properties. A plain `e_source_new_with_uid` source
// gives a `g_return_if_fail` critical for each of them, not a value.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceRegistryServer, e_server_side_source_get_write_directory,
    e_server_side_source_new, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_get_parent, e_source_get_uid, e_source_get_writable,
    e_source_has_extension, e_source_registry_server_new, e_source_set_display_name,
    g_file_new_for_path,
};
use glib_sys::GFALSE;
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_collection::create_resource::{
    adopt_created, create_on_server, requested_of, stored_password_of,
};
use jmap_backend_collection::resource_id::{kind_of, resource_id_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, Requested};
use jmap_mock::MockServer;

mod common;
use common::{with_timeout, with_timeout_duration};

/// A different file name for each source. EDS derives the `ESource` uid from
/// the file name, so two sources in one test process never share a uid.
static NEXT: AtomicU32 = AtomicU32::new(0);
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The connection of the account that owns this collection, as
/// `create_resource_sync` reads it from the account `ESource`.
///
/// This is intentionally not the address of the mock. The child must use the
/// connection of the account. If the two addresses were equal, a test could not
/// find a child that used the URL of the create.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// A scratch `EServerSideSource`, as EDS's `remote_create` handler builds one.
///
/// `server_side_source_remote_create_cb` calls `e_server_side_source_new` on a
/// file in the user directory of the registry that does not exist yet. Then it
/// applies the keyfile that Evolution sent. This builds the same object and sets
/// the two values of that keyfile directly: the kind extension and the display
/// name. The test is about this backend, not about the keyfile parser of EDS.
struct Scratch {
    /// Kept for the life of the source. `e_server_side_source_new` keeps only a
    /// weak reference to the server. All `EServerSideSource` setters refuse a
    /// source whose server is finalized.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl Scratch {
    fn new(kind: Option<ChildKind>, display_name: &str) -> Self {
        // SAFETY: these functions take no arguments. `e_source_get_extension`
        // cannot find an extension class whose type is not yet referenced.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let path = std::env::temp_dir().join(format!(
            "jmap-scratch-{}-{}.source",
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
            // `e_source_registry_server_new` returns its base class,
            // `EDBusServer *`. The cast is the implicit cast that C callers do.
            // The constructor owns no bus name and reads no sources. That occurs
            // only when the server runs.
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

        if let Some(kind) = kind {
            let extension = match kind {
                ChildKind::AddressBook => E_SOURCE_EXTENSION_ADDRESS_BOOK,
                ChildKind::Calendar => E_SOURCE_EXTENSION_CALENDAR,
            };
            // SAFETY: a live source and a header constant. The source creates
            // the extension on demand and owns it. Here the creation is the
            // purpose: the extension gives the scratch source its kind for
            // Evolution and for `kind_of`.
            unsafe { assert!(!e_source_get_extension(source, extension.as_ptr()).is_null()) };
        }

        let display_name = CString::new(display_name).expect("no NUL in a test name");
        // SAFETY: a live source and a NUL-terminated string the setter copies.
        unsafe { e_source_set_display_name(source, display_name.as_ptr()) };

        Self { server, source }
    }

    fn parent(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns NULL or a string it owns.
        unsafe { read_string(e_source_get_parent(self.source)) }
    }

    fn writable(&self) -> bool {
        // SAFETY: as above.
        unsafe { e_source_get_writable(self.source) != GFALSE }
    }

    fn write_directory(&self) -> Option<String> {
        // SAFETY: as above, and the source is an `EServerSideSource`.
        unsafe { read_string(e_server_side_source_get_write_directory(self.source.cast())) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.source, name.as_ptr()) != GFALSE }
    }
}

impl Drop for Scratch {
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

fn requested_kind(scratch: &Scratch) -> Option<Requested> {
    // SAFETY: a live source this test holds a reference to.
    unsafe { requested_of(scratch.source) }
}

/// The full create, as the vfunc runs it. It makes the collection on the mock,
/// then it changes the scratch source into the child for that collection.
fn create(server: &MockServer, scratch: &Scratch) -> Child {
    let requested = requested_kind(scratch).expect("the scratch source names a kind");
    let child = create_on_server(
        &ConnectTarget::Origin(server.origin().to_owned()),
        false,
        Credentials::none(),
        &requested,
    )
    .expect("the mock creates collections");

    // SAFETY: a live `EServerSideSource` this test holds a reference to.
    unsafe {
        adopt_created(
            scratch.source,
            &child,
            &connection(),
            "jmap-test-account",
            Some("/var/lib/jmap-test-cache"),
        )
    }
    .expect("every setting of a child this backend describes is writable");

    child
}

#[test]
fn a_scratch_address_book_asks_for_an_address_book_under_its_display_name() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        assert_eq!(
            requested_kind(&scratch),
            Some(Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            })
        );
    });
}

#[test]
fn a_scratch_calendar_asks_for_a_calendar_under_its_display_name() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new(Some(ChildKind::Calendar), "Trips");

        assert_eq!(
            requested_kind(&scratch),
            Some(Requested {
                kind: ChildKind::Calendar,
                display_name: "Trips".to_owned(),
            })
        );
    });
}

#[test]
fn a_scratch_source_that_names_no_kind_asks_for_nothing() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The EDS documentation of the vfunc says: "If this cannot be determined
        // without ambiguity, the function must return an error." A guessed kind
        // can create the wrong type of collection.
        let scratch = Scratch::new(None, "Work");

        assert_eq!(requested_kind(&scratch), None);
    });
}

#[test]
fn reading_the_kind_does_not_give_the_scratch_source_an_extension_it_lacked() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_source_get_extension` *creates* an extension that it cannot find.
        // A scratch source is a real source that EDS writes to disk. A read of
        // `[Calendar]` would make each new address book a source that both
        // factories claim.
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { kind_of(scratch.source) },
            Some(ChildKind::AddressBook)
        );
        assert!(!scratch.has_extension(E_SOURCE_EXTENSION_CALENDAR));
        assert!(!scratch.has_extension(E_SOURCE_EXTENSION_RESOURCE));
    });
}

#[test]
fn a_created_address_book_leaves_a_source_this_backend_reads_back_as_its_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This property prevents a duplicate after a create. EDS uses
        // `dup_resource_id` to pair a published child with a resource id
        // (`collection_backend_ref_child_source`). If the resource id does not
        // come back, the next populate creates a *second* source. The result is
        // one address book on the server and two rows in the sidebar.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        let child = create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { resource_id_of(scratch.source) },
            Some(child.resource_id.clone())
        );
        assert_eq!(child.kind, ChildKind::AddressBook);
    });
}

#[test]
fn a_created_calendar_leaves_a_source_this_backend_reads_back_as_its_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::Calendar), "Trips");

        let child = create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { resource_id_of(scratch.source) },
            Some(child.resource_id.clone())
        );
        assert_eq!(child.kind, ChildKind::Calendar);
    });
}

#[test]
fn a_created_child_reaches_the_server_the_account_names() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The child uses the connection of the *account*, not the URL of the
        // create. A child with the discovery URL works only while the two URLs
        // agree.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        let child = create(&server, &scratch);

        // Read the child as the address book backend reads it: through
        // `SourceConfig`.
        // SAFETY: a live source this test holds a reference to.
        let config =
            unsafe { jmap_backend_core::source::SourceConfig::from_source(scratch.source) }
                .expect("a child this backend wrote is one the book backend can read");
        assert_eq!(
            config.target,
            ConnectTarget::Origin("https://jmap.example.com:8443".to_owned())
        );
        assert_eq!(
            config.resource_id.as_deref(),
            Some(child.collection_id.as_str())
        );
    });
}

#[test]
fn a_created_child_is_parented_writable_and_written_back_to_the_collections_cache() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `collection_backend_new_source()` sets these three things on a child
        // that EDS makes, so a scratch source does not have them.
        // - Without the parent, the source is not a child of this account, and
        //   `child_added` does not fire for it.
        // - Without the write directory, the `.source` file stays in the user
        //   directory of the registry. A removal of the account does not remove it.
        // - Without `writable`, the user cannot rename the new address book.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        create(&server, &scratch);

        assert_eq!(scratch.parent().as_deref(), Some("jmap-test-account"));
        assert!(scratch.writable());
        assert_eq!(
            scratch.write_directory().as_deref(),
            Some("/var/lib/jmap-test-cache")
        );
    });
}

#[test]
fn adopting_a_created_child_is_not_what_offers_it_for_deletion() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is intentional. evolution-ews sets `remote-deletable` at each of
        // the three sites that make a child. This backend sets it at one site:
        // `delete_resource::offer_deletion`, from the `child_added` that fires
        // when EDS publishes this source. Thus Evolution offers "Delete" for a
        // created child. This test makes sure that `adopt_created` does not set
        // the flag. A second site can become different from the first and offer
        // "Delete" for a child that `offer_deletion` refuses.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert!(
            unsafe { eds_sys::e_source_get_remote_deletable(scratch.source) } == GFALSE,
            "adopt_created set remote-deletable; the flag has two homes now"
        );
    });
}

#[test]
fn stored_password_of_a_gone_registry_server_names_the_account_in_a_structured_field() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `server.is_null()` is the branch for a registry server that is gone.
        // This branch needs no `ESourceRegistryServer`. Thus the test examines
        // only the `account_id` field and needs no credentials provider.
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");
        // SAFETY: a live source that this test holds a reference to. The uid is
        // `(transfer none)`.
        let account_id = unsafe { read_string(e_source_get_uid(scratch.source)) }
            .expect("every source has a uid");

        let (_, captured) = jmap_tracing_capture::capture(|| {
            // SAFETY: a NULL server (the branch under test), a live source, and no
            // cancellable. The contract of `stored_password_of` permits all three.
            let password = unsafe {
                stored_password_of(
                    ptr::null_mut(),
                    scratch.source,
                    ptr::null_mut(),
                    "create_resource_sync",
                )
            };
            assert_eq!(password, None);
        });

        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "account_id" && *value == account_id),
            "expected an account_id field naming the source's own uid, got {captured:?}"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_create_resource_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
