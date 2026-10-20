// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that the module binds the gettext domain of this project when
//! Evolution loads it.
//!
//! The Evolution shell configures gettext for *itself*. But it does not tell
//! gettext the location of the `evolution-jmap` catalogues, because it does not
//! know them. Until some code does this, each lookup in the domain of this
//! project uses the gettext default directory. CMake installs the `.mo` files
//! there only if the prefix is `/usr`. Thus the lookup returns the
//! untranslated string. There is no diagnostic for this: a missing catalogue
//! and a catalogue with no translation for the message look the same.
//!
//! Of the five modules of this repository, this is the one whose strings a
//! user reads in a dialog in normal operation, not only when an error occurs:
//! the labels of the account setup page, which `insert_widgets` builds. The
//! binding must occur before all of these labels. Then the first label is
//! marked for translation and also translated, not marked and silently
//! untranslated.
//!
//! ## Why this is a separate test binary
//!
//! The test starts from a deliberately wrong binding. Thus "the domain is bound
//! where the build put the catalogues" can become true only if the entry point
//! makes it true. That is process-global state, and
//! [`bind`](jmap_backend_core::i18n::bind) is a `OnceLock`. If a different test
//! reaches the entry point first, it uses the `OnceLock`: the decoy stays, and
//! this test fails for the wrong reason. `tests/module.rs` and
//! `tests/entry_points.rs` both reach the entry point. Cargo gives each file in
//! `tests/` its own process, so the only test in this file is isolated.

use std::ffi::CStr;
use std::ptr;

use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_module_get_type, g_type_module_use,
};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::module::{load, unload};

/// A `GTypeModule` in place of the `EModule` that Evolution loads this code as.
#[repr(C)]
struct TestModule {
    parent: GTypeModule,
}

#[repr(C)]
struct TestModuleClass {
    parent_class: GTypeModuleClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the GTypeModule instance
// and class structs, and GTypeModule derives from GObject.
unsafe impl ObjectSubclass for TestModule {
    const NAME: &'static CStr = c"JmapConfigTextdomainTestModule";
    type Instance = TestModule;
    type Class = TestModuleClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a newly allocated class struct of this type,
        // which starts with GTypeModuleClass, where both slots are.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib gives the module that loads, which is what the entry
    // point needs.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// When the module loads, the binding of the domain changes to the installed
/// catalogue directory.
///
/// The decoy directory makes this an assertion about the entry point, not
/// about the machine. On an uninstalled build, [`LOCALE_DIR`] is the
/// compiled-in default of gettext. A process with no binding reports it too,
/// and the test would pass with a module that did nothing.
///
/// The test uses `g_type_module_use`, not a direct call to [`load`], because
/// Evolution uses that path and it is equally easy to write. The entry point
/// is reached through the `GTypeModuleClass.load` vfunc, exactly as from
/// `EModule`.
#[test]
fn the_entry_point_binds_the_domain() {
    let decoy = c"/nonexistent/jmap-config-decoy-locale";
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

    // SAFETY: `module` is a GTypeModule. The reference that this takes is never
    // released, as in the Evolution shell: it loads each module in its
    // directory at startup and keeps them.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    assert_eq!(
        binding().as_c_str(),
        LOCALE_DIR,
        "the entry point did not bind the domain, so the account setup page's \
         labels would be looked up wherever Evolution's shell happened to point"
    );
}
