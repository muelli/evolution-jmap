// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that the module binds the gettext domain of this project when the
//! registry loads it.
//!
//! `evolution-source-registry` does not know this project. Thus nothing in that
//! process tells gettext the location of our catalogues. Until a bind occurs,
//! each lookup in the `evolution-jmap` domain uses the default directory of
//! gettext. That is not the CMake install directory of the `.mo` files, unless
//! the prefix is `/usr`. The lookup then returns the untranslated string. No
//! diagnostic shows this: a missing catalogue and a missing translation look
//! the same.
//!
//! This module shows text to the user, for example the error messages of
//! `create_resource` and `delete_resource`. These messages go through gettext,
//! so they need the binding.
//!
//! ## Why this file is a separate test binary
//!
//! The test starts from an intentionally incorrect binding. Thus only the entry
//! point can bind the domain to the catalogue directory of the build. The
//! binding is process-global state. [`bind`](jmap_backend_core::i18n::bind)
//! uses a `OnceLock`. If a different test calls the entry point first, it uses
//! the `OnceLock`, the decoy stays, and this test fails for an incorrect reason.
//! Cargo runs each file in `tests/` in its own process. In this file, only one
//! test calls the entry point, and that gives the isolation.

use std::ptr;

use glib_sys::GFALSE;
use gobject_sys::{GTypeModule, g_object_new, g_type_module_use};
use jmap_backend_collection::module::{load, unload};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::register_static;
use jmap_backend_core::test_module;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// A `GTypeModule` in place of the `EModule` that the registry loads.
test_module!(
    TestModule,
    c"JmapCollectionTextdomainTestModule",
    load = load,
    unload = unload
);

/// The module load changes the binding of the domain to the installed
/// catalogue directory.
///
/// The decoy directory makes this a test of the entry point, not of the
/// machine. On an uninstalled build, [`LOCALE_DIR`] is the compiled-in default
/// of gettext. Without the decoy, a module that does nothing also passes.
///
/// The test calls `g_type_module_use`, not [`load`] directly, because the
/// registry uses that path. The entry point runs through the
/// `GTypeModuleClass.load` vfunc, as from `EModule`.
#[test]
fn the_entry_point_binds_the_domain() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let decoy = c"/nonexistent/jmap-collection-decoy-locale";
        assert_eq!(
            bind_to(decoy).as_c_str(),
            decoy,
            "the decoy binding did not take, so the assertion below cannot fail"
        );

        let gtype = register_static::<TestModule>();
        assert_ne!(gtype, 0, "the stand-in module type did not register");

        // SAFETY: the type is registered, and GTypeModule has no construct
        // properties.
        let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
        assert!(!module.is_null(), "g_object_new returned NULL");

        // SAFETY: `module` is a GTypeModule. The test does not release this
        // use, as the registry does while it needs the backend.
        assert_ne!(
            unsafe { g_type_module_use(module) },
            GFALSE,
            "the module would not load at all"
        );

        assert_eq!(
            binding().as_c_str(),
            LOCALE_DIR,
            "the entry point did not bind the domain, so the child sources this \
             backend names would be looked up wherever the source registry happened \
             to point"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_textdomain_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
