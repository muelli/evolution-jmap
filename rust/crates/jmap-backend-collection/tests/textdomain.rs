// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That the module binds this project's gettext domain when the registry loads
//! it.
//!
//! `evolution-source-registry` has never heard of this project, so nothing in
//! that process has told gettext where our catalogues are. Until something
//! does, every lookup in the `evolution-jmap` domain goes to whatever directory
//! gettext defaults to — which is not where CMake installed the `.mo` files
//! unless the prefix happened to be `/usr` — and answers with the untranslated
//! string. There is no diagnostic for that: a missing catalogue and a catalogue
//! with no translation for the message look identical.
//!
//! This module has the most to gain from the binding of the four, because the
//! strings it originates are the ones the user reads first: an account's child
//! sources are created here, and their display names — "Contacts", "Calendar",
//! the mail folder tree's root — are what appear in Evolution's sidebar under
//! the account. None of those is marked for translation yet; the binding is
//! what has to be in place before the first one can be.
//!
//! ## Why this is a test binary of its own
//!
//! It starts from a deliberately wrong binding, so that "the domain is bound
//! where the build put the catalogues" can only become true by the entry point
//! making it true. That is process-global state, and
//! [`bind`](jmap_backend_core::i18n::bind) is a `OnceLock` that a sibling test
//! reaching the entry point first would spend — leaving the decoy in place and
//! failing this one for the wrong reason. Cargo gives each file in `tests/` its
//! own process; being the only test in this file is the isolation.

use std::ffi::CStr;
use std::ptr;

use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_type_module_get_type, g_type_module_use,
};
use jmap_backend_collection::module::{load, unload};
use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A `GTypeModule` standing in for the `EModule` the registry would load us as.
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
    const NAME: &'static CStr = c"JmapCollectionTextdomainTestModule";
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
/// because that is the path the registry takes and it is no harder to write:
/// the entry point is reached through the `GTypeModuleClass.load` vfunc,
/// exactly as it would be from `EModule`.
#[test]
fn the_entry_point_binds_the_domain() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let decoy = c"/nonexistent/jmap-collection-decoy-locale";
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
        // given back, which is what the registry does for as long as the backend is
        // wanted.
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
