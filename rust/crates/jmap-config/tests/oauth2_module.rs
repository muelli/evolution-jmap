// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that `e_module_load` registers the `[JMAP OAuth2]` extension type of
//! this crate. `jmap-backend-collection/tests/oauth2_extension.rs` has the same
//! test for the other process that parses the account sources of this project.
//!
//! Evolution's shell process (which loads this module) parses each `ESource`
//! that it receives from the registry over D-Bus into a local `ESource`. It
//! uses the same `e_source_get_extension` name lookup as
//! `evolution-source-registry`. If the shell has not registered the
//! `[JMAP OAuth2]` extension type before it parses the source of an OAuth2
//! account, this process also silently ignores the group. The account editor
//! (`insert_widgets`, `commit_changes`) runs in this process, thus a
//! `read`/`apply` call usually registers the type first. But the generic source
//! code of Evolution reads an account that opens for *editing* before
//! `check_complete`/`insert_widgets` of this crate run. Thus the registration
//! cannot wait for those calls.

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

/// A `GTypeModule` in place of the `EModule` that Evolution loads this crate
/// as. `tests/textdomain.rs` and `tests/module.rs` use the same substitute.
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
        // SAFETY: the function has no arguments, and the type initialises itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a new class struct of this type. It starts
        // with GTypeModuleClass, which contains the two slots.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib supplies the module that loads, and the entry point
    // requires that module.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: the same as `module_load`.
    unsafe { unload(module) };
}

/// A source that `e_source_get_extension` has not examined before. This test
/// does not call `jmap_config::oauth2::apply`/`read`. It calls only the raw EDS
/// entry point that the source parser of Evolution calls.
fn fresh_source() -> *mut ESource {
    let uid =
        std::ffi::CString::new("jmap-config-oauth2-extension-test").expect("no NUL in a literal");
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, the default main context and a GError
    // out-parameter are the documented arguments.
    let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null(), "e_source_new_with_uid failed");
    source
}

#[test]
fn the_module_load_registers_the_oauth2_extension_type() {
    let gtype = register_static::<TestModule>();
    assert_ne!(gtype, 0, "the stand-in module type did not register");

    // SAFETY: the type is registered, and GTypeModule has no construct
    // properties of its own.
    let module = unsafe { g_object_new(gtype, ptr::null()) }.cast::<GTypeModule>();
    assert!(!module.is_null(), "g_object_new returned NULL");

    // SAFETY: `module` is a GTypeModule. This call keeps its reference and
    // never releases it. Evolution's shell does the same while it needs the
    // module.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    let source = fresh_source();
    // SAFETY: `source` is a live, new source. `EXTENSION_NAME` is a 'static
    // NUL-terminated string. No `jmap_config::oauth2` function runs first,
    // thus a NULL here means that the module load did not register the type.
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
