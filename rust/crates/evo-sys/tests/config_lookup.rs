// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `EConfigLookupWorker` is the interface the real-server-readiness item
// (M7) will implement. M7 registers an `EExtension` against Evolution's
// `EConfigLookup` (`extensible_type = E_TYPE_CONFIG_LOOKUP`). This is the
// same idiom `jmap-config/src/module.rs` documents for
// `JmapConfigServiceBackend`: putting the type in the type system is the
// registration.
//
// The extension runs `jmap_config::oauth2_setup::discover_and_register`
// when Evolution's account assistant tries to auto-configure a JMAP
// account. evolution-ews uses the same pattern for Exchange autodiscovery,
// in `src/EWS/evolution/e-ews-config-lookup.c`
// (gitlab.gnome.org/GNOME/evolution-ews, `master`).
//
// This file provides the FFI foundation for that work. It does not do the
// work itself.
//
// As in `eds-sys/tests/oauth2.rs`: an interface has no `g_type_query` size.
// The checkable part is the vtable's offsets. If bindgen writes a function
// pointer into the wrong slot, a call through that slot jumps into
// whatever else occupies that location, when EDS's wrapper dispatches
// through it.
//
// This file proves one dispatch short of that file's proof. Here is the
// gap, stated plainly instead of glossed over: `e_config_lookup_worker_run`
// (`e-config-lookup-worker.c`, checked against the real source) calls
// `g_return_if_fail (E_IS_CONFIG_LOOKUP (config_lookup))`. A real
// `EConfigLookup` comes only from `e_config_lookup_new (ESourceRegistry
// *)`. That function needs a live D-Bus session and an activatable
// `evolution-source-registry`. `docs/functional-tests.md` gates M9's
// Layer 1 behind that dependency (`dbus-run-session`), which the plain
// `rust-test-eds` target used by this crate's tests does not provide.
// This file does not take on that dependency.
//
// This file proves `get_display_name`'s offset, by real dispatch. `run`'s
// slot is the next field in the struct, both in source order and in
// `size_of` terms. A wrong offset for `run` would already have shifted
// `get_display_name`, which would fail this test. `run` is not dispatched
// independently here, though. This comment states that limit instead of
// letting the test name imply more than it checks.
//
// `jmap-functional/tests/config-lookup.rs` registers `JmapConfigLookup`
// against a real `EConfigLookup` under M9's harness, and calls `run` for
// the first time.

use evo_sys::*;
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

/// EDS's own default leaves both slots empty: `iface->get_display_name =
/// NULL; iface->run = NULL;`, in `e_config_lookup_worker_default_init`
/// (`e-config-lookup-worker.c`). This crate's own implementation must fill
/// both slots.
#[test]
fn the_worker_contract_is_an_interface_with_only_a_gobject_behind_it() {
    // SAFETY: These calls only read the type system. GLib allocates the
    // prerequisite array with g_malloc, and the caller must free it.
    unsafe {
        let worker = e_config_lookup_worker_get_type();
        assert_eq!(g_type_fundamental(worker), gobject_sys::G_TYPE_INTERFACE);

        let mut q = std::mem::zeroed::<GTypeQuery>();
        g_type_query(worker, &mut q);
        assert_eq!(q.type_, 0, "g_type_query knows an interface after all");

        let mut n = 0;
        let prerequisites = gobject_sys::g_type_interface_prerequisites(worker, &mut n);
        let required = std::slice::from_raw_parts(prerequisites, n as usize).to_vec();
        glib_sys::g_free(prerequisites.cast());
        assert_eq!(
            required,
            vec![gobject_sys::G_TYPE_OBJECT],
            "EConfigLookupWorker requires more of an implementer than a GObject"
        );
    }
}

#[test]
fn neither_slot_has_a_default_implementation() {
    // SAFETY: The code refs the default vtable for the reads, then unrefs
    // it again.
    unsafe {
        let vtable = gobject_sys::g_type_default_interface_ref(e_config_lookup_worker_get_type())
            .cast::<EConfigLookupWorkerInterface>();
        assert!(!vtable.is_null(), "the interface has no default vtable");

        assert!(
            (*vtable).get_display_name.is_none(),
            "get_display_name has a default"
        );
        assert!((*vtable).run.is_none(), "run has a default");

        gobject_sys::g_type_default_interface_unref(vtable.cast());
    }
}

const PROBE_DISPLAY_NAME: &CStr = c"JMAP OAuth 2.0 discovery (probe)";

static REACHED_GET_DISPLAY_NAME: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn probe_get_display_name(
    _lookup_worker: *mut EConfigLookupWorker,
) -> *const gchar {
    REACHED_GET_DISPLAY_NAME.fetch_add(1, Ordering::SeqCst);
    PROBE_DISPLAY_NAME.as_ptr()
}

/// This file fills this slot but does not dispatch it. See the module
/// comment for why `run` needs a real `EConfigLookup`, which this file
/// does not construct. `eds-sys/tests/oauth2.rs` fills two `SoupMessage`
/// slots it cannot call either, for the same reason: an unfilled slot
/// here would look the same as a slot bindgen sized wrong.
unsafe extern "C" fn probe_run(
    _lookup_worker: *mut EConfigLookupWorker,
    _config_lookup: *mut EConfigLookup,
    _params: *const ENamedParameters,
    _out_restart_params: *mut *mut ENamedParameters,
    _cancellable: *mut GCancellable,
    _error: *mut *mut GError,
) {
    unreachable!("not dispatched by this test file; see the module doc");
}

unsafe extern "C" fn probe_interface_init(iface: *mut c_void, _data: *mut c_void) {
    // SAFETY: GLib passes the interface struct for the type this
    // initialiser registers against: `EConfigLookupWorker`.
    let iface = unsafe { &mut *iface.cast::<EConfigLookupWorkerInterface>() };
    iface.get_display_name = Some(probe_get_display_name);
    iface.run = Some(probe_run);
}

/// A `GObject` that implements the interface and nothing else. The
/// process registers it once, like `eds-sys/tests/oauth2.rs`'s
/// `probe_service`.
fn probe_worker() -> *mut EConfigLookupWorker {
    // SAFETY: This uses a fresh type name. The sizes come from the structs
    // GLib allocates. GLib copies the `GInterfaceInfo` before it returns.
    unsafe {
        let gtype = g_type_register_static_simple(
            gobject_sys::G_TYPE_OBJECT,
            c"JmapEvoSysProbeConfigLookupWorker".as_ptr(),
            size_of::<GObjectClass>() as u32,
            None,
            size_of::<GObject>() as u32,
            None,
            0,
        );
        assert_ne!(gtype, 0, "registering the probe type failed");
        let info = GInterfaceInfo {
            interface_init: Some(probe_interface_init),
            interface_finalize: None,
            interface_data: std::ptr::null_mut(),
        };
        g_type_add_interface_static(gtype, e_config_lookup_worker_get_type(), &info);

        let object = gobject_sys::g_object_new_with_properties(
            gtype,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!object.is_null(), "instantiating the probe type failed");
        object.cast()
    }
}

/// The one dispatch this file can make without a live D-Bus session. EDS's
/// `e_config_lookup_worker_get_display_name` wrapper reads the slot this
/// crate wrote, then calls it. This proves both sides agree on the first
/// slot's location. Every later slot's offset is computed relative to that
/// location.
#[test]
fn get_display_name_dispatches_to_the_function_this_crate_wrote_into_it() {
    let worker = probe_worker();
    REACHED_GET_DISPLAY_NAME.store(0, Ordering::SeqCst);

    // SAFETY: This uses a live implementer. The returned string is
    // `'static` because the probe never allocates it, so nothing here
    // frees it.
    unsafe {
        assert_eq!(
            CStr::from_ptr(e_config_lookup_worker_get_display_name(worker)),
            PROBE_DISPLAY_NAME
        );
        g_object_unref(worker.cast());
    }

    assert_eq!(
        REACHED_GET_DISPLAY_NAME.load(Ordering::SeqCst),
        1,
        "e_config_lookup_worker_get_display_name answered without reaching the slot this crate wrote"
    );
}
