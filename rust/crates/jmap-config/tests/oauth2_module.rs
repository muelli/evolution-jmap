// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That `e_module_load` registers this crate's own `[JMAP OAuth2]` extension
//! type — see `jmap-backend-collection/tests/oauth2_extension.rs` for the
//! same test against the other process this project's account sources are
//! parsed in.
//!
//! Evolution's own shell process (which loads this module) parses every
//! `ESource` it receives from the registry over D-Bus into its own local
//! `ESource` objects, using the same `e_source_get_extension` name lookup
//! `evolution-source-registry` does. If Evolution's shell has not registered
//! `[JMAP OAuth2]`'s extension type by the time it parses an OAuth2 account's
//! source, the group is silently unrecognised in this process too — and this
//! is the process the account editor (`insert_widgets`, `commit_changes`)
//! runs in, so a `read`/`apply` call would usually have already registered it
//! by the time anything asks. But an account opened for *editing* is read by
//! Evolution's own generic source machinery before this crate's
//! `check_complete`/`insert_widgets` ever run — the registration cannot wait
//! for those.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{ESource, e_source_get_extension, e_source_new_with_uid};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_object_unref, g_type_module_get_type,
    g_type_module_use,
};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::module::{load, unload};
use jmap_config::oauth2::EXTENSION_NAME;

/// A `GTypeModule` standing in for the `EModule` Evolution would load us as —
/// the same stand-in `tests/textdomain.rs`/`tests/module.rs` use.
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
    const NAME: &'static CStr = c"JmapConfigOAuth2ExtensionTestModule";
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

/// A source `e_source_get_extension` has never been asked about before —
/// nothing in this test calls `jmap_config::oauth2::apply`/`read`, only the
/// raw EDS entry point Evolution's own source parsing would call.
fn fresh_source() -> *mut ESource {
    let uid =
        std::ffi::CString::new("jmap-config-oauth2-extension-test").expect("no NUL in a literal");
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
    // out-parameter are the documented arguments.
    let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null(), "e_source_new_with_uid failed");
    source
}

#[test]
fn the_module_load_registers_the_oauth2_extension_type() {
    let gtype = register_static::<TestModule>();
    assert_ne!(gtype, 0, "the stand-in module type did not register");

    // SAFETY: the type is registered and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule; the reference this takes is never
    // given back, which is what Evolution's shell does for as long as the
    // module is wanted.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    let source = fresh_source();
    // SAFETY: `source` is a live, freshly created source; `EXTENSION_NAME` is
    // a 'static NUL-terminated string. No `jmap_config::oauth2` function is
    // called first, so a NULL here means the module load itself did not
    // register the type.
    let extension = unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) };
    assert!(
        !extension.is_null(),
        "e_module_load did not register the [JMAP OAuth2] extension type — \
         Evolution's own source parsing would silently drop that group in \
         this process before anything calls jmap_config::oauth2::apply/read \
         on the same source"
    );

    // SAFETY: a live source this test created.
    unsafe { g_object_unref(source.cast()) };
}
