// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That `e_module_load` registers `jmap_config::oauth2`'s `[JMAP OAuth2]`
//! extension type — the other half of real-server readiness's OAuth2 storage
//! that neither the storage session nor the service-registration session
//! closed.
//!
//! `jmap_config::oauth2::apply`/`read` both call `ensure_registered()` before
//! touching a source, so any caller that only goes through them never sees
//! the gap. But EDS's own `.source` keyfile parser restores an account's
//! extensions by the same name lookup those two functions use
//! (`e_source_get_extension`/`source_find_extension_classes_rec`), and it
//! runs in every process that loads a JMAP account's source from disk or
//! over D-Bus — `evolution-source-registry` first among them, which is
//! exactly the process this crate's module is loaded into. If that process
//! parses a `[JMAP OAuth2]` group before anything has called `apply`/`read`
//! on that particular source, the group is silently unrecognised rather than
//! restored — an account's discovered endpoints and client id, gone on the
//! next restart, with nothing to say why.
//!
//! This test drives the actual failure mode: register nothing but the
//! module, then ask `e_source_get_extension` for `[JMAP OAuth2]` directly —
//! the same call EDS's own parser makes — without ever calling
//! `jmap_config::oauth2::apply`/`read` first. Its own process (this is a
//! separate `tests/*.rs` binary, like `tests/textdomain.rs`), so nothing else
//! in this binary can have registered the extension type first and made the
//! assertion trivially true.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{ESource, e_source_get_extension, e_source_new_with_uid};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use gobject_sys::{
    GTypeModule, GTypeModuleClass, g_object_new, g_object_unref, g_type_module_get_type,
    g_type_module_use,
};
use jmap_backend_collection::module::{load, unload};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::oauth2::EXTENSION_NAME;

/// A `GTypeModule` standing in for the `EModule` the registry would load us
/// as — the same stand-in `tests/factory.rs`, `tests/textdomain.rs` and
/// `tests/oauth2_service.rs` all use.
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
    const NAME: &'static CStr = c"JmapCollectionOAuth2ExtensionTestModule";
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
/// raw EDS entry point EDS's own parser would call.
fn fresh_source() -> *mut ESource {
    let uid = std::ffi::CString::new("jmap-collection-oauth2-extension-test")
        .expect("no NUL in a literal");
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
    // given back, which is what the registry does for as long as the backend
    // is wanted.
    assert_ne!(
        unsafe { g_type_module_use(module) },
        GFALSE,
        "the module would not load at all"
    );

    let source = fresh_source();
    // SAFETY: `source` is a live, freshly created source; `EXTENSION_NAME` is
    // a 'static NUL-terminated string. This is the same call EDS's own
    // `.source` parser makes to restore a group — no `jmap_config::oauth2`
    // function is called first, so a NULL here means the module load itself
    // did not register the type.
    let extension = unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) };
    assert!(
        !extension.is_null(),
        "e_module_load did not register the [JMAP OAuth2] extension type — \
         EDS's own keyfile parser would silently drop that group in any \
         process that loads this module before anything calls \
         jmap_config::oauth2::apply/read on the same source"
    );

    // SAFETY: a live source this test created.
    unsafe { g_object_unref(source.cast()) };
}
