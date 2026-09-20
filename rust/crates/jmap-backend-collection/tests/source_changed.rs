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

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, ESource,
    ESourceAuthentication, ESourceCollection, e_source_authentication_get_type,
    e_source_authentication_set_host, e_source_collection_get_type,
    e_source_collection_set_allow_sources_rename, e_source_get_extension, e_source_get_type,
    e_source_new_with_uid,
};
use glib_sys::{
    GFALSE, GMainContext, GTRUE, g_main_context_iteration, g_main_context_new,
    g_main_context_unref, gpointer,
};
use gobject_sys::{
    G_CONNECT_DEFAULT, G_TYPE_NONE, G_TYPE_OBJECT, GSignalQuery, g_object_new_with_properties,
    g_object_unref, g_signal_connect_object, g_signal_emit_by_name, g_signal_lookup,
    g_signal_query, g_type_class_ref, g_type_class_unref,
};
use jmap_backend_collection::backend::JmapCollectionBackend;
use jmap_backend_collection::source_changed::login_fingerprint;

mod common;
use common::with_timeout;

/// How many times the probe handler below ran.
static FIRED: AtomicUsize = AtomicUsize::new(0);
/// The same, for the account-write probe, which runs on its own source in its
/// own main context and so may not share a counter with tests running beside it.
static HEARD_OWN_WRITE: AtomicUsize = AtomicUsize::new(0);
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
        Self::in_context(uid, ptr::null_mut())
    }

    /// The same, emitting into a main context of the caller's own. `ESource`
    /// coalesces a burst of edits into one idle emission on the context it was
    /// created with, and a test that has to *run* that idle needs one nothing
    /// else is iterating.
    fn in_context(uid: &str, main_context: *mut GMainContext) -> Self {
        // `e_source_get_extension` finds an extension class by walking the
        // registered children of `E_TYPE_SOURCE_EXTENSION`, so a type nothing
        // has referenced yet is one it cannot find.
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe {
            e_source_collection_get_type();
            e_source_authentication_get_type();
        }

        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, a main context or NULL for the
        // default one, and a pointer to a NULL `GError`.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), main_context, &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// `[Authentication] Host`: the one account setting a login cannot do
    /// without, and so the one an owner fixing a broken account edits.
    fn set_host(&self, host: &str) {
        let host = CString::new(host).expect("no NUL in a test host");
        // SAFETY: a live source and a header constant; the extension is
        // created on demand and owned by the source, and the setter copies the
        // string.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_host(auth, host.as_ptr());
        }
    }

    /// `[Collection] AllowSourcesRename`, which is what `populate` writes on
    /// every account it runs for.
    fn set_allow_sources_rename(&self, allow: bool) {
        // SAFETY: as above.
        unsafe {
            let collection: *mut ESourceCollection =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
            e_source_collection_set_allow_sources_rename(
                collection,
                if allow { GTRUE } else { GFALSE },
            );
        }
    }

    fn login_fingerprint(&self) -> u64 {
        // SAFETY: a live source, only read from.
        unsafe { login_fingerprint(self.0) }
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
            watch.wants_repopulate(0x5eed),
            "an account whose fan-out has never succeeded is worth another login"
        );
    });
}

/// Shaped like the real handler, counting the `"changed"` emissions caused by
/// a write `populate` itself makes.
unsafe extern "C" fn own_write_probe(_source: *mut ESource, _data: gpointer) {
    HEARD_OWN_WRITE.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn the_flag_populate_writes_comes_back_as_an_edit_of_the_account() {
    with_timeout(|| {
        HEARD_OWN_WRITE.store(0, Ordering::SeqCst);

        // SAFETY: no arguments; the context is unreferenced at the end.
        let context = unsafe { g_main_context_new() };
        let source = Source::in_context("jmap-watch-own-write", context);
        // SAFETY: no properties, so the count is zero and both arrays NULL.
        let watched =
            unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
        assert!(!watched.is_null(), "a plain GObject could not be created");

        // SAFETY: a live source, `"changed"` on its own type, a handler whose
        // signature matches the signal's marshaller, and a live `GObject`.
        unsafe {
            g_signal_connect_object(
                source.0.cast(),
                c"changed".as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut ESource, gpointer),
                    unsafe extern "C" fn(),
                >(own_write_probe)),
                watched,
                G_CONNECT_DEFAULT,
            );
        }

        source.set_allow_sources_rename(true);
        // The emission is an idle on the source's own context, so it lands
        // after the write returns -- which under EDS means after the populate
        // that made it has connected this very handler.
        for _ in 0..100 {
            if HEARD_OWN_WRITE.load(Ordering::SeqCst) > 0 {
                break;
            }
            // SAFETY: a context this thread created and nothing else iterates.
            unsafe { g_main_context_iteration(context, GFALSE) };
        }

        assert_eq!(
            HEARD_OWN_WRITE.load(Ordering::SeqCst),
            1,
            "allow-sources-rename carries E_SOURCE_PARAM_SETTING, so writing it \
             emits \"changed\" on the account the populate is running for"
        );

        // SAFETY: the references this test took.
        unsafe {
            g_object_unref(watched);
            g_main_context_unref(context);
        }
    });
}

#[test]
fn a_setting_the_populate_wrote_leaves_the_login_fingerprint_alone() {
    with_timeout(|| {
        let source = Source::new("jmap-watch-fingerprint-own");
        source.set_host("jmap.example.com");

        let before = source.login_fingerprint();
        source.set_allow_sources_rename(true);

        assert_eq!(
            source.login_fingerprint(),
            before,
            "nothing about the login changed, so the edit populate caused is \
             not one worth a second login"
        );
    });
}

#[test]
fn a_corrected_host_changes_the_login_fingerprint() {
    with_timeout(|| {
        let source = Source::new("jmap-watch-fingerprint-edit");
        source.set_host("jmpa.example.com");

        let mistyped = source.login_fingerprint();
        source.set_host("jmap.example.com");

        assert_ne!(
            source.login_fingerprint(),
            mistyped,
            "a fixed host is the edit this whole handler exists for"
        );
    });
}
