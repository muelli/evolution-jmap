// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verifies that the module binds the gettext domain when loaded.
//!
//! This test binary runs in an isolated process to check that `load` binds
//! the project locale directory even when preceded by an unconfigured state.

use std::ptr;

use glib_sys::GFALSE;
use gobject_sys::{GTypeModule, g_object_new, g_type_module_use};
use jmap_backend_book::module::{load, unload};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::register_static;
use jmap_backend_core::test_module;

// Stand-in `GTypeModule` representing the EDS `EModule`.
test_module!(
    TestModule,
    c"JmapBookTextdomainTestModule",
    load = load,
    unload = unload
);

/// Loading the module binds the gettext domain to the project locale directory.
#[test]
fn the_entry_point_binds_the_domain() {
    let decoy = c"/nonexistent/jmap-book-decoy-locale";
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

    // SAFETY: `module` is a valid `GTypeModule` pointer.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    assert_eq!(
        binding().as_c_str(),
        LOCALE_DIR,
        "the entry point did not bind the domain, so this backend's translated \
         strings would be looked for wherever the address book factory happened \
         to point"
    );
}
