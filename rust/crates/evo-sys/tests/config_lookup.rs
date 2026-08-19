// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `EConfigLookupWorker` is the interface M7's real-server-readiness item will
// implement: an `EExtension` registered against Evolution's `EConfigLookup`
// (`extensible_type = E_TYPE_CONFIG_LOOKUP`, the same "putting the type in the
// type system is the registration" idiom `jmap-config/src/module.rs` already
// documents for `JmapConfigServiceBackend`) that runs
// `jmap_config::oauth2_setup::discover_and_register` the moment Evolution's
// account assistant tries to auto-configure a JMAP account — the pattern
// evolution-ews's own `src/EWS/evolution/e-ews-config-lookup.c`
// (gitlab.gnome.org/GNOME/evolution-ews, `master`) already uses for Exchange
// autodiscovery. See `docs/NIGHT-LOG.md`'s three-hundred-and-fourth session
// for the full reasoning; this file is the FFI foundation that work needs,
// not the work itself.
//
// As in `eds-sys/tests/oauth2.rs`: an interface has no `g_type_query` size, so
// what is checkable is the vtable's *offsets* — a slot bindgen wrote a
// function pointer into at the wrong place is a jump into whatever else
// happens to sit there when EDS's own wrapper dispatches through it.
//
// One dispatch short of that file's proof, and said plainly rather than
// glossed over: `e_config_lookup_worker_run` (`e-config-lookup-worker.c`,
// checked against the real source rather than assumed) itself calls
// `g_return_if_fail (E_IS_CONFIG_LOOKUP (config_lookup))`, and a real
// `EConfigLookup` only comes from `e_config_lookup_new (ESourceRegistry *)` —
// which means a live D-Bus session and an activatable
// `evolution-source-registry`. That is exactly the dependency
// `docs/functional-tests.md` gates M9's Layer 1 behind (`dbus-run-session`,
// not available to the plain `rust-test-eds` target this crate's tests run
// under), so it is not something this file takes on. What is proven here is
// `get_display_name`'s offset, by real dispatch; `run`'s is the very next
// field in the struct, textually and in `size_of` terms, so a wrong offset
// for it would already have to have shifted `get_display_name` for this test
// to still pass — but it is not independently dispatched, and this comment
// says so rather than letting the test's name imply more than it checked.
// `jmap-functional/tests/config-lookup.rs` is the test that registers
// `JmapConfigLookup` against a real `EConfigLookup` under M9's harness and
// calls `run` for the first time.

use evo_sys::*;
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

/// EDS's own answer for which slots have no default — `iface->get_display_name
/// = NULL; iface->run = NULL;` in `e_config_lookup_worker_default_init`
/// (`e-config-lookup-worker.c`), so both are one this crate's own
/// implementation must fill.
#[test]
fn the_worker_contract_is_an_interface_with_only_a_gobject_behind_it() {
    // SAFETY: plain type-system reads; the prerequisite array is g_malloc'd
    // for the caller to free.
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
    // SAFETY: the default vtable is ref'd for the length of the reads and
    // unref'd again.
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

/// Filled but not dispatched here — see the module doc on why `run` needs a
/// real `EConfigLookup` this file does not construct. Still written, the way
/// `eds-sys/tests/oauth2.rs` still fills the two `SoupMessage` slots it
/// cannot call either: an unfilled slot here would be indistinguishable from
/// a slot bindgen mis-sized.
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
    // SAFETY: GLib passes the interface struct for the type we registered this
    // initialiser against, which is `EConfigLookupWorker`'s.
    let iface = unsafe { &mut *iface.cast::<EConfigLookupWorkerInterface>() };
    iface.get_display_name = Some(probe_get_display_name);
    iface.run = Some(probe_run);
}

/// A `GObject` that implements the interface and nothing else, registered
/// once for the process — as `eds-sys/tests/oauth2.rs`'s `probe_service`.
fn probe_worker() -> *mut EConfigLookupWorker {
    // SAFETY: a fresh type name, sizes taken from the structs GLib will
    // allocate, and a `GInterfaceInfo` GLib copies before it returns.
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

/// The one dispatch this file can make without a live D-Bus session: EDS's
/// own `e_config_lookup_worker_get_display_name` wrapper reads the slot this
/// crate wrote and calls it — proof the two sides agree about where the first
/// slot is, which is also the offset every slot after it is computed relative
/// to.
#[test]
fn get_display_name_dispatches_to_the_function_this_crate_wrote_into_it() {
    let worker = probe_worker();
    REACHED_GET_DISPLAY_NAME.store(0, Ordering::SeqCst);

    // SAFETY: a live implementer; the returned string is `'static` (the probe
    // never allocates), so nothing here frees it.
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
