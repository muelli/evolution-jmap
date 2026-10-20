// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the `"changed"` connection that a populate puts on the account
// source, and for the two GLib facts that the connection depends on.
//
// The unit tests of `crate::source_changed` cover the decision: whether an
// edit needs a second login. These tests need a linked libedataserver and a
// real GObject. They examine two facts:
// - `ESource::changed` has no argument. Thus the second parameter of the
//   handler is its user data, not a signal argument.
// - A `g_signal_connect_object` connection stops when its object is gone. For
//   this reason, this crate stores no handler id and overrides no `dispose`.
//
// The backend instance is detached. This is safe for the same reason as in the
// `dup_resource_id` tests of `backend.rs` (but not its `populate` tests). A read
// of the watch reads one field of our half of the instance struct. It does not
// touch the parent bytes.

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
use jmap_backend_core::callback::gcallback;

mod common;
use common::with_timeout;

/// How many times the probe handler below ran.
static FIRED: AtomicUsize = AtomicUsize::new(0);
/// The counter for the account-write probe. That probe runs on its own source
/// in its own main context, so it must not share a counter with parallel tests.
static HEARD_OWN_WRITE: AtomicUsize = AtomicUsize::new(0);
/// The user-data pointer of the last run.
static SAW_DATA: AtomicUsize = AtomicUsize::new(0);

/// Has the same signature as the handler of `crate::source_changed`: the
/// emitting `ESource`, then the object of the connection.
unsafe extern "C" fn probe(_source: *mut ESource, data: gpointer) {
    FIRED.fetch_add(1, Ordering::SeqCst);
    SAW_DATA.store(data as usize, Ordering::SeqCst);
}

/// A live `ESource` without a registry. Drop releases its reference.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        Self::in_context(uid, ptr::null_mut())
    }

    /// The same, but with a main context of the caller. `ESource` merges a
    /// burst of edits into one idle emission on its main context. A test that
    /// *runs* that idle needs a context that nothing else iterates.
    fn in_context(uid: &str, main_context: *mut GMainContext) -> Self {
        // `e_source_get_extension` walks the registered children of
        // `E_TYPE_SOURCE_EXTENSION` to find an extension class. It cannot find a
        // type that is not yet referenced.
        // SAFETY: these functions take no arguments, and the type system
        // initialises itself.
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

    /// `[Authentication] Host`. A login always needs this setting, so a user who
    /// repairs a broken account usually edits it.
    fn set_host(&self, host: &str) {
        let host = CString::new(host).expect("no NUL in a test host");
        // SAFETY: a live source and a header constant. The source creates the
        // extension on demand and owns it. The setter copies the string.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_host(auth, host.as_ptr());
        }
    }

    /// `[Collection] AllowSourcesRename`. `populate` writes it on each account
    /// that it runs for.
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
        // SAFETY: a live source, and `"changed"` takes no arguments
        // (`the_changed_signal_carries_no_argument_of_its_own` tests this).
        unsafe { g_signal_emit_by_name(self.0.cast(), c"changed".as_ptr()) };
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this releases the one reference of this struct.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn the_changed_signal_carries_no_argument_of_its_own() {
    with_timeout(|| {
        // The class reference installs the signals. `g_signal_lookup` returns 0
        // for a classed type that is not yet initialised.
        // SAFETY: `ESource` is a registered classed type in the linked
        // libedataserver. The reference is released after the query. The
        // signal ids that it installed stay valid after the release.
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
        // SAFETY: no properties, so the count is zero and both arrays are NULL.
        let watched =
            unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
        assert!(!watched.is_null(), "a plain GObject could not be created");

        // SAFETY: a live source, `"changed"` on its own type, a handler whose
        // signature agrees with the marshaller of the signal (the test above
        // examines this), and a live `GObject` for the connection.
        unsafe {
            g_signal_connect_object(
                source.0.cast(),
                c"changed".as_ptr(),
                gcallback(probe as unsafe extern "C" fn(*mut ESource, gpointer)),
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

        // SAFETY: this releases the one reference from the creation above.
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

/// Has the signature of the real handler. Counts the `"changed"` emissions that
/// a write by `populate` causes.
unsafe extern "C" fn own_write_probe(_source: *mut ESource, _data: gpointer) {
    HEARD_OWN_WRITE.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn the_flag_populate_writes_comes_back_as_an_edit_of_the_account() {
    with_timeout(|| {
        HEARD_OWN_WRITE.store(0, Ordering::SeqCst);

        // SAFETY: the function takes no arguments. The test releases the
        // context at the end.
        let context = unsafe { g_main_context_new() };
        let source = Source::in_context("jmap-watch-own-write", context);
        // SAFETY: no properties, so the count is zero and both arrays are NULL.
        let watched =
            unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
        assert!(!watched.is_null(), "a plain GObject could not be created");

        // SAFETY: a live source, `"changed"` on its own type, a handler whose
        // signature agrees with the marshaller of the signal, and a live
        // `GObject`.
        unsafe {
            g_signal_connect_object(
                source.0.cast(),
                c"changed".as_ptr(),
                gcallback(own_write_probe as unsafe extern "C" fn(*mut ESource, gpointer)),
                watched,
                G_CONNECT_DEFAULT,
            );
        }

        source.set_allow_sources_rename(true);
        // The emission is an idle on the context of the source, so it occurs
        // after the write returns. In EDS, this is after the populate that made
        // the write connects this handler.
        for _ in 0..100 {
            if HEARD_OWN_WRITE.load(Ordering::SeqCst) > 0 {
                break;
            }
            // SAFETY: this thread created the context, and nothing else iterates it.
            unsafe { g_main_context_iteration(context, GFALSE) };
        }

        assert_eq!(
            HEARD_OWN_WRITE.load(Ordering::SeqCst),
            1,
            "allow-sources-rename carries E_SOURCE_PARAM_SETTING, so writing it \
             emits \"changed\" on the account the populate is running for"
        );

        // SAFETY: this releases the references of this test.
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
