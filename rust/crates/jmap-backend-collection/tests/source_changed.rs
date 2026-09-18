// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `"changed"` connection a populate leaves on the account source, and the
// two GLib facts the way it is made depends on.
//
// `crate::source_changed`'s own unit tests cover the decision -- whether an
// edit is worth a second login. What only a linked libedataserver and a real
// GObject can answer is the rest: that `ESource::changed` really carries no
// argument of its own, so the handler's second parameter is its user data and
// not a signal argument; and that a `g_signal_connect_object` connection
// really stops firing once the object it was connected against is gone, which
// is the whole reason this crate stores no handler id and overrides no
// `dispose`.
//
// The backend instance is a detached one. That is sound here for the same
// reason it is in `backend.rs`'s `dup_resource_id` tests and unlike its
// `populate` ones: reading the watch reads one field of our own half of the
// instance struct and touches none of the parent bytes.

use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{ESource, e_source_get_type, e_source_new_with_uid};
use glib_sys::gpointer;
use gobject_sys::{
    G_CONNECT_DEFAULT, G_TYPE_NONE, G_TYPE_OBJECT, GSignalQuery, g_object_new_with_properties,
    g_object_unref, g_signal_connect_object, g_signal_emit_by_name, g_signal_lookup,
    g_signal_query, g_type_class_ref, g_type_class_unref,
};
use jmap_backend_collection::backend::JmapCollectionBackend;

mod common;
use common::with_timeout;

/// How many times the probe handler below ran.
static FIRED: AtomicUsize = AtomicUsize::new(0);
/// The user-data pointer the last run was handed.
static SAW_DATA: AtomicUsize = AtomicUsize::new(0);

/// Shaped exactly like `crate::source_changed`'s own handler: the emitting
/// `ESource`, then the object the connection was made against.
unsafe extern "C" fn probe(_source: *mut ESource, data: gpointer) {
    FIRED.fetch_add(1, Ordering::SeqCst);
    SAW_DATA.store(data as usize, Ordering::SeqCst);
}

/// A live `ESource` with no registry behind it, unreferenced on drop.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context, and a
        // pointer to a NULL `GError`.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn emit_changed(&self) {
        // SAFETY: a live source, and `"changed"` takes no arguments -- which
        // is what `the_changed_signal_carries_no_argument_of_its_own` holds.
        unsafe { g_signal_emit_by_name(self.0.cast(), c"changed".as_ptr()) };
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: the one reference this struct took.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn the_changed_signal_carries_no_argument_of_its_own() {
    with_timeout(|| {
        // The class reference is what installs the signals: `g_signal_lookup`
        // answers 0 for a classed type nothing has initialised yet.
        // SAFETY: `ESource` is a registered classed type in the linked
        // libedataserver; the reference is given back after the query, and the
        // signal ids it installed outlive it.
        let query = unsafe {
            let class = g_type_class_ref(e_source_get_type());
            assert!(!class.is_null(), "ESource's class could not be referenced");
            let id = g_signal_lookup(c"changed".as_ptr(), e_source_get_type());
            assert_ne!(id, 0, "ESource has no \"changed\" signal");
            let mut query = std::mem::zeroed::<GSignalQuery>();
            g_signal_query(id, &mut query);
            g_type_class_unref(class);
            query
        };

        assert_eq!(
            query.n_params, 0,
            "a \"changed\" with parameters would arrive between the source and the user data"
        );
        assert_eq!(
            query.return_type, G_TYPE_NONE,
            "\"changed\" returns nothing"
        );
    });
}

#[test]
fn a_connection_made_against_an_object_stops_firing_once_it_is_gone() {
    with_timeout(|| {
        FIRED.store(0, Ordering::SeqCst);
        SAW_DATA.store(0, Ordering::SeqCst);

        let source = Source::new("jmap-watch-lifetime");
        // SAFETY: no properties, so the count is zero and both arrays NULL.
        let watched =
            unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
        assert!(!watched.is_null(), "a plain GObject could not be created");

        // SAFETY: a live source, `"changed"` on its own type, a handler whose
        // signature matches the signal's marshaller (held by the test above),
        // and a live `GObject` to connect against.
        unsafe {
            g_signal_connect_object(
                source.0.cast(),
                c"changed".as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut ESource, gpointer),
                    unsafe extern "C" fn(),
                >(probe)),
                watched,
                G_CONNECT_DEFAULT,
            );
        }

        source.emit_changed();
        assert_eq!(FIRED.load(Ordering::SeqCst), 1, "the handler did not run");
        assert_eq!(
            SAW_DATA.load(Ordering::SeqCst),
            watched as usize,
            "the object connected against arrives as the user data"
        );

        // SAFETY: the one reference the creation above took.
        unsafe { g_object_unref(watched) };

        source.emit_changed();
        assert_eq!(
            FIRED.load(Ordering::SeqCst),
            1,
            "the connection outlived the object it was made against, so the \
             handler would have been handed a freed backend"
        );
    });
}

#[test]
fn a_fresh_instance_has_connected_nothing_and_proved_nothing() {
    with_timeout(|| {
        let backend = JmapCollectionBackend::detached();
        let watch = backend.watch();

        assert!(
            watch.claim_connection(),
            "the first populate of an account is the one that connects"
        );
        assert!(
            !watch.claim_connection(),
            "a second populate must not add a second handler"
        );
        assert!(
            watch.wants_repopulate(),
            "an account whose fan-out has never succeeded is worth another login"
        );
    });
}
