// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! GCancellable observation tests across `EBookMetaBackend` vfuncs.
//!
//! Asserts that cancelling an operation propagates through `observe` to cancel
//! the underlying client request and return `G_IO_ERROR_CANCELLED`.

use std::ffi::CString;
use std::ptr;

use eds_sys::{EBookMetaBackendClass, EContact};
use gio_sys::{
    G_IO_ERROR_CANCELLED, GCancellable, g_cancellable_cancel, g_cancellable_new, g_io_error_quark,
};
use glib_sys::{GError, GFALSE, g_error_free, gchar};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_backend_book::backend::{JmapBookBackend, JmapBookBackendClass};
use jmap_backend_book::marshal;
use jmap_backend_core::subclass::register_static;

/// The class EDS would dispatch through, kept referenced for the test's
/// duration so the vfunc pointers stay valid.
struct Class(*mut JmapBookBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapBookBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: Referencing the registered class runs class_init, returning the class struct.
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

mod common;
use common::{Detached, Fixture};

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
    /// Checks that the error domain is `G_IO_ERROR` and code is `G_IO_ERROR_CANCELLED`.
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
    let mut backend = Detached::with_sync(fixture.sync());
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
    let mut backend = Detached::with_sync(fixture.sync());
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
    let seeded = fixture.seed_default("Vera Oldenburg", "vera@example.com");
    let class = Class::get();
    let mut backend = Detached::with_sync(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(seeded.to_string()).expect("a uid with no NUL");
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

/// Verifies a cancelled contact save is not written to the server.
#[test]
fn a_contact_the_user_stopped_saving_is_not_written() {
    let fixture = Fixture::start();
    let before = fixture.contacts_on_the_server();

    let class = Class::get();
    let mut backend = Detached::with_sync(fixture.sync());
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
    let seeded = fixture.seed_default("Vera Oldenburg", "vera@example.com");
    let before = fixture.contacts_on_the_server();

    let class = Class::get();
    let mut backend = Detached::with_sync(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(seeded.to_string()).expect("a uid with no NUL");
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

/// A NULL cancellable indicates an uncancelable call and must still run to completion.
#[test]
fn an_operation_with_no_cancellable_at_all_still_runs() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::with_sync(fixture.sync());

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
