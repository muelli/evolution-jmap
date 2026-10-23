// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verifies the module binds gettext domain `evolution-jmap` when loaded by EDS.

use std::ptr;

use glib_sys::GFALSE;
use gobject_sys::{GTypeModule, g_object_new, g_type_module_use};
use jmap_backend_cal::module::{load, unload};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::register_static;
use jmap_backend_core::test_module;

// GTypeModule stand-in for the EModule EDS loads.
test_module!(
    TestModule,
    c"JmapCalTextdomainTestModule",
    load = load,
    unload = unload
);

/// Verifies loading the module binds the gettext domain to the installed locale directory.
#[test]
fn the_entry_point_binds_the_domain() {
    let decoy = c"/nonexistent/jmap-cal-decoy-locale";
    assert_eq!(
        bind_to(decoy).as_c_str(),
        decoy,
        "the decoy binding did not take, so the assertion below cannot fail"
    );

    let gtype = register_static::<TestModule>();
    assert_ne!(gtype, 0, "the stand-in module type did not register");

    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule; the reference this takes is never
    // given back, which is what EDS does for as long as the backend is wanted.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    assert_eq!(
        binding().as_c_str(),
        LOCALE_DIR,
        "the entry point did not bind the domain, so this backend's translated \
         strings would be looked for wherever the calendar factory happened to \
         point"
    );
}
