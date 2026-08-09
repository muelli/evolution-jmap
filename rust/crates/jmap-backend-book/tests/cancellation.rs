// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Stop button, and the vfuncs that had never looked at it.
//!
//! Every sync vfunc on this backend is handed a `GCancellable`, and until now
//! every one of them but `connect_sync` named it `_cancellable` and ignored it.
//! The only cancellation that reached anything was the connect's, through a
//! `CancelBridge` whose flag was built into the [`Client`] — and that flag then
//! belonged to the *account*, not to the operation, for the rest of the
//! session. So a user stopping a first sync of a large address book was
//! pressing a button attached to nothing, and a connect they *did* manage to
//! stop left a client that refused everything afterwards.
//!
//! `jmap-backend-core`'s `observe` is what the vfuncs now hold for the length
//! of their call, and these tests are the proof that holding it is enough: a
//! cancellable this test cancels stops a request made several layers below, by
//! a client built before the cancellable existed.
//!
//! ## Why the calls go through the class struct
//!
//! For the same reason `tests/backend.rs` does: EDS dispatches through the
//! class, and a vfunc that is correct but not installed is a backend that
//! silently uses `EBookMetaBackend`'s defaults. The instance is
//! [`JmapBookBackend::detached`] — a real one needs an `ESourceRegistry` and so
//! a session bus — and nothing but its session slot is touched.
//!
//! ## One cancellable, cancelled before the call
//!
//! Rather than a thread racing to stop a fetch in flight. What is under test is
//! that the vfunc *observes*, and `g_cancellable_connect` fires its callback
//! immediately for an already-cancelled cancellable — which is exactly what EDS
//! produces for an operation the user stopped while it was still queued. The
//! in-flight case is a property of the transport, not of these vfuncs.
//!
//! [`Client`]: jmap_client::Client

use std::ffi::CString;
use std::ptr;

use eds_sys::{EBookMetaBackend, EBookMetaBackendClass, EContact};
use gio_sys::{
    G_IO_ERROR_CANCELLED, GCancellable, g_cancellable_cancel, g_cancellable_new, g_io_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_free, gchar};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_backend_book::backend::{JmapBookBackend, JmapBookBackendClass};
use jmap_backend_book::marshal;
use jmap_backend_core::subclass::register_static;
use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;

/// The class EDS would dispatch through, kept referenced for the test's
/// duration so the vfunc pointers stay valid.
struct Class(*mut JmapBookBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapBookBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init and hands back a class struct of our own layout.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &EBookMetaBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// An instance the GObject type system knows nothing about — see the module
/// comment. Only the session slot may be touched through it.
struct Detached(Box<JmapBookBackend>);

impl Detached {
    fn new(sync: BookSync) -> Self {
        let backend = JmapBookBackend::detached();
        backend.store_connection(sync);
        Self(backend)
    }

    fn as_ptr(&mut self) -> *mut EBookMetaBackend {
        ptr::from_mut(&mut *self.0).cast()
    }
}

/// A mock server with one contact in its default address book.
struct Fixture {
    server: MockServer,
    account_id: Id,
    book: Id,
    seeded: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let book = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_address_book("Personal", true)
        };
        let mut fixture = Self {
            server,
            account_id,
            book,
            seeded: Id::new("unseeded"),
        };
        fixture.seeded = fixture.seed("Vera Oldenburg", "vera@example.com");
        fixture
    }

    fn sync(&self) -> BookSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        BookSync::new(client, self.account_id.clone(), self.book.clone())
    }

    fn seed(&self, full_name: &str, email: &str) -> Id {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        client
            .contact_create(
                &self.account_id,
                &ContactCard::simple(self.book.clone(), full_name, email),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// What the server holds now, asked over a connection of its own — the
    /// vfunc's scope is gone by the time this runs, so this call is not
    /// cancelled by it.
    fn contacts_on_the_server(&self) -> usize {
        self.sync().list_existing().expect("listed").1.len()
    }
}

/// A `GCancellable` the user has already stopped.
struct Stopped(*mut GCancellable);

impl Stopped {
    fn new() -> Self {
        // SAFETY: constructing a GCancellable and cancelling it; both take
        // ownership of nothing.
        unsafe {
            let cancellable = g_cancellable_new();
            g_cancellable_cancel(cancellable);
            Self(cancellable)
        }
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        // SAFETY: the one reference `g_cancellable_new` handed over.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// What a vfunc that refused set, freed the way EDS frees it.
struct Refusal(*mut GError);

impl Refusal {
    /// The failure is `G_IO_ERROR_CANCELLED` — GLib's own domain, which is
    /// what EDS tests for before it decides an address book is broken and puts
    /// an alert in front of the user. Reported as `E_CLIENT_ERROR` instead, a
    /// user pressing Stop gets an error dialog for doing what they asked.
    fn is_the_stop_the_user_pressed(&self, what: &str) {
        assert!(!self.0.is_null(), "{what} did not report the cancellation");
        // SAFETY: a live GError this struct owns.
        unsafe {
            assert_eq!(
                (*self.0).domain,
                g_io_error_quark(),
                "{what} reported the cancellation in the wrong domain: {}",
                std::ffi::CStr::from_ptr((*self.0).message).to_string_lossy()
            );
            assert_eq!((*self.0).code, G_IO_ERROR_CANCELLED, "{what}");
        }
    }
}

impl Drop for Refusal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the vfunc handed ownership of it over.
            unsafe { g_error_free(self.0) };
        }
    }
}

/// The first sync of an account is the longest operation this backend has, and
/// the one a user is most likely to stop.
#[test]
fn a_listing_the_user_stopped_does_not_go_to_the_server() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut glib_sys::GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: an instance of the class, a live cancellable, and out-parameters
    // that are writable and currently NULL.
    let ok = unsafe {
        let list = class.vfuncs().list_existing_sync.expect("a listing vfunc");
        list(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the listing claimed to have happened");
    assert!(tag.is_null(), "a cancelled listing produced a sync tag");
    assert!(objects.is_null(), "a cancelled listing produced contacts");
    Refusal(error).is_the_stop_the_user_pressed("list_existing_sync");
}

/// The incremental sync, which runs on every refresh and so is the one the
/// Stop button is aimed at most often.
#[test]
fn a_changes_call_the_user_stopped_does_not_go_to_the_server() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    // A tag, so the vfunc asks the server for a diff rather than answering
    // "list the whole book instead" without a request.
    let tag = CString::new("0").expect("a tag with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `tag` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let changes = class.vfuncs().get_changes_sync.expect("a changes vfunc");
        changes(
            backend.as_ptr(),
            tag.as_ptr(),
            GFALSE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the changes call claimed to have happened");
    Refusal(error).is_the_stop_the_user_pressed("get_changes_sync");
}

#[test]
fn a_contact_the_user_stopped_loading_is_not_fetched() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(fixture.seeded.to_string()).expect("a uid with no NUL");
    let mut contact: *mut EContact = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `uid` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let load = class.vfuncs().load_contact_sync.expect("a load vfunc");
        load(
            backend.as_ptr(),
            uid.as_ptr(),
            ptr::null(),
            &mut contact,
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the load claimed to have happened");
    assert!(contact.is_null(), "a cancelled load produced a contact");
    Refusal(error).is_the_stop_the_user_pressed("load_contact_sync");
}

/// A write, not a read: what the user stopped here is a change to their
/// address book, and it must not be on the server afterwards.
#[test]
fn a_contact_the_user_stopped_saving_is_not_written() {
    let fixture = Fixture::start();
    let before = fixture.contacts_on_the_server();

    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let contact = marshal::contact_from_vcard(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Never Saved\r\nEMAIL:never@example.com\r\nEND:VCARD\r\n",
    );
    assert!(!contact.is_null(), "the fixture vCard did not parse");

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `contact` is a live EContact this test owns.
    let ok = unsafe {
        let save = class.vfuncs().save_contact_sync.expect("a save vfunc");
        save(
            backend.as_ptr(),
            GFALSE,
            0,
            contact,
            ptr::null(),
            0,
            &mut new_uid,
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    // SAFETY: the reference `contact_from_vcard` handed over.
    unsafe { marshal::contact_unref(contact) };

    assert_eq!(ok, GFALSE, "the save claimed to have happened");
    assert!(new_uid.is_null(), "a cancelled save produced a uid");
    Refusal(error).is_the_stop_the_user_pressed("save_contact_sync");
    assert_eq!(
        fixture.contacts_on_the_server(),
        before,
        "the cancelled save reached the server anyway"
    );
}

/// The other write, and the one where getting it wrong loses data.
#[test]
fn a_contact_the_user_stopped_removing_is_still_there() {
    let fixture = Fixture::start();
    let before = fixture.contacts_on_the_server();

    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(fixture.seeded.to_string()).expect("a uid with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `uid` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let remove = class.vfuncs().remove_contact_sync.expect("a remove vfunc");
        remove(
            backend.as_ptr(),
            0,
            uid.as_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the removal claimed to have happened");
    Refusal(error).is_the_stop_the_user_pressed("remove_contact_sync");
    assert_eq!(
        fixture.contacts_on_the_server(),
        before,
        "the cancelled removal deleted the contact anyway"
    );
}

/// A NULL cancellable is GIO's "this call cannot be cancelled", and must not
/// be read as "already stopped" — the vfuncs receive one for every operation
/// EDS runs on its own account.
#[test]
fn an_operation_with_no_cancellable_at_all_still_runs() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut glib_sys::GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above, with the NULL cancellable EDS is entitled to pass.
    let ok = unsafe {
        let list = class.vfuncs().list_existing_sync.expect("a listing vfunc");
        list(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            ptr::null_mut(),
            &mut error,
        )
    };

    assert_eq!(
        ok,
        glib_sys::GTRUE,
        "the listing failed with no cancellable"
    );
    assert!(error.is_null());
    // SAFETY: both were allocated by GLib and ownership passed to this test.
    unsafe {
        glib_sys::g_free(tag.cast());
        glib_sys::g_slist_free_full(objects, Some(eds_sys::e_book_meta_backend_info_free));
    }
}
