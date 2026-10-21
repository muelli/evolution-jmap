// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests that `e_module_load` registers the `[JMAP OAuth2]` extension type of
//! `jmap_config::oauth2`.
//!
//! `jmap_config::oauth2::apply` and `read` call `ensure_registered()` before
//! they use a source. A caller that uses only these functions does not see the
//! problem. But the `.source` keyfile parser of EDS restores the extensions of
//! an account with the same name lookup
//! (`e_source_get_extension`/`source_find_extension_classes_rec`). The parser
//! runs in each process that loads the source of a JMAP account from disk or
//! over D-Bus. One such process is `evolution-source-registry`, which loads
//! the module of this crate. If that process parses a `[JMAP OAuth2]` group
//! before a call to `apply` or `read` on that source, the parser ignores the
//! group. Then the discovered endpoints and the client id of the account are
//! lost at the next restart, and no message shows the cause.
//!
//! This test causes that failure mode. It registers only the module. Then it
//! requests `[JMAP OAuth2]` from `e_source_get_extension` directly, as the EDS
//! parser does, with no earlier call to `apply` or `read`. This file is a
//! separate test binary (as `tests/textdomain.rs` is). Thus no other test can
//! register the extension type first and make the assertion pass without
//! cause.

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
use jmap_config::pending_delete::EXTENSION_NAME as PENDING_DELETE;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A `GTypeModule` in place of the `EModule` that the registry loads. Also
/// used in `tests/factory.rs`, `tests/textdomain.rs` and
/// `tests/oauth2_service.rs`.
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
        // SAFETY: the function takes no arguments, and the type initialises
        // itself.
        unsafe { g_type_module_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to our new class struct. It starts with
        // GTypeModuleClass, which contains both slots.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.load = Some(module_load);
        vfuncs.unload = Some(module_unload);
    }
}

unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
    // SAFETY: GLib passes the module that it loads, as the entry point
    // requires.
    unsafe { load(module) };
    GTRUE
}

unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
    // SAFETY: as `module_load`.
    unsafe { unload(module) };
}

/// A source that `e_source_get_extension` has not yet seen. This test does not
/// call `jmap_config::oauth2::apply` or `read`. It calls only the EDS entry
/// point that the EDS parser calls.
fn fresh_source() -> *mut ESource {
    let uid = std::ffi::CString::new("jmap-collection-oauth2-extension-test")
        .expect("no NUL in a literal");
    let mut error = ptr::null_mut();
    // SAFETY: a NUL-terminated uid, a NULL main context (the default context)
    // and a GError out-parameter are the documented arguments.
    let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null(), "e_source_new_with_uid failed");
    source
}

#[test]
fn the_module_load_registers_the_oauth2_extension_type() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        let source = fresh_source();
        // SAFETY: `source` is a live, new source. `EXTENSION_NAME` is a
        // 'static NUL-terminated string. The `.source` parser of EDS makes this
        // call to restore a group. No `jmap_config::oauth2` function runs first.
        // Thus NULL means that the module load did not register the type.
        let extension = unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) };
        assert!(
            !extension.is_null(),
            "e_module_load did not register the [JMAP OAuth2] extension type — \
             EDS's own keyfile parser would silently drop that group in any \
             process that loads this module before anything calls \
             jmap_config::oauth2::apply/read on the same source"
        );
        // SAFETY: as above. The module load also registers the pending-delete type.
        let pending = unsafe { e_source_get_extension(source, PENDING_DELETE.as_ptr()) };
        assert!(
            !pending.is_null(),
            "e_module_load did not register the [JMAP Pending Delete] extension type"
        );

        // SAFETY: a live source that this test created.
        unsafe { g_object_unref(source.cast()) };
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_oauth2_extension_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
