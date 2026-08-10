// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That the module binds this project's gettext domain when Evolution loads it.
//!
//! Evolution's shell has of course set gettext up for *itself*; what it has not
//! done is tell gettext where the `evolution-jmap` catalogues are, because it
//! has never heard of them. Until something does, every lookup in our domain
//! goes to whatever directory gettext defaults to — which is not where CMake
//! installed the `.mo` files unless the prefix happened to be `/usr` — and
//! answers with the untranslated string. There is no diagnostic for that: a
//! missing catalogue and a catalogue with no translation for the message look
//! identical.
//!
//! Of this repository's five modules, this is the one whose strings a user
//! reads while *looking* at a dialog rather than while something goes wrong: the
//! account-setup page's labels. None of them exists yet — `insert_widgets` is
//! unwritten — which is exactly why the binding goes in now, so that the first
//! label to be added can be marked for translation and be translated, instead
//! of being marked and silently not.
//!
//! ## Why this is a test binary of its own
//!
//! It starts from a deliberately wrong binding, so that "the domain is bound
//! where the build put the catalogues" can only become true by the entry point
//! making it true. That is process-global state, and
//! [`bind`](jmap_backend_core::i18n::bind) is a `OnceLock` that a sibling test
//! reaching the entry point first would spend — leaving the decoy in place and
//! failing this one for the wrong reason. `tests/module.rs` and
//! `tests/entry_points.rs` both reach it. Cargo gives each file in `tests/` its
//! own process; being the only test in this file is the isolation.

use std::ffi::CStr;
use std::ptr;

use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_module_get_type, g_type_module_use,
};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::module::{load, unload};

/// A `GTypeModule` standing in for the `EModule` Evolution would load us as.
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
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with GTypeModuleClass, where both slots live.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module being loaded, which is what the entry
    // point wants.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// Loading the module moves the domain's binding to the installed catalogue
/// directory.
///
/// The decoy directory is what makes this an assertion about the entry point
/// rather than about the machine. On an uninstalled build [`LOCALE_DIR`] is
/// gettext's own compiled-in default, so a process that had never bound
/// anything would report it too, and the test would pass against a module that
/// did nothing at all.
///
/// Driven through `g_type_module_use` rather than by calling [`load`] directly,
/// because that is the path Evolution takes and it is no harder to write: the
/// entry point is reached through the `GTypeModuleClass.load` vfunc, exactly as
/// it would be from `EModule`.
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

    // SAFETY: `module` is a GTypeModule; the reference this takes is never
    // given back, which is what Evolution's shell does — it loads every module
    // in its directory at startup and keeps them.
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
