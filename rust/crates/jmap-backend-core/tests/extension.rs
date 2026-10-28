// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `ExtensionObject` against a probe extensible type: the class names the
// extensible, `e_extensible_load_extensions` runs the hook once per object,
// and a panic in the hook stays inside the guard.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    EExtension, EExtensionClass, e_extensible_get_type, e_extensible_list_extensions,
    e_extensible_load_extensions, e_extension_get_extensible, e_extension_get_type,
};
use glib_sys::{GType, g_list_free};
use gobject_sys::{
    G_TYPE_OBJECT, GObject, GObjectClass, g_object_new_with_properties, g_object_unref,
    g_type_class_ref, g_type_class_unref, g_type_is_a, g_type_plugin_get_type,
};
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::subclass::{InterfaceDecl, ObjectSubclass, register_static};

/// A plain `GObject` that implements `EExtensible`.
#[repr(C)]
struct Extensible {
    parent: GObject,
}

#[repr(C)]
struct ExtensibleClass {
    parent_class: GObjectClass,
}

// SAFETY: both structs are #[repr(C)] and start with the GObject structs.
unsafe impl ObjectSubclass for Extensible {
    const NAME: &'static CStr = c"JmapTestExtensionHost";
    type Instance = Extensible;
    type Class = ExtensibleClass;

    fn parent_type() -> GType {
        G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // SAFETY: no arguments. The type initialises itself.
        vec![InterfaceDecl::defaults(unsafe { e_extensible_get_type() })]
    }
}

jmap_backend_core::initialise_parents_before_main!(
    Extensible,
    PanickerHost,
    InterfaceHost,
    ExtensionObject<Recorder>,
    ExtensionObject<Panicker>,
    ExtensionObject<InterfacedExtension>
);

/// The last extensible that the hook saw, and the number of hook calls.
static SEEN: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

struct Recorder;

impl Extension for Recorder {
    const TYPE_NAME: &'static CStr = c"JmapTestRecorderExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"JmapTestExtensionHost";

    fn extensible_type() -> GType {
        register_static::<Extensible>()
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: a live extension in construction, by the trait contract.
        let extensible = unsafe { e_extension_get_extensible(extension) };
        SEEN.store(extensible as usize, Ordering::SeqCst);
        CALLS.fetch_add(1, Ordering::SeqCst);
    }
}

/// The class reads the extensible type through `class_init_types`.
fn class_extensible_type(gtype: GType) -> GType {
    // SAFETY: a registered class type. The class starts with `EExtensionClass`.
    unsafe {
        let class = g_type_class_ref(gtype);
        let found = (*class.cast::<EExtensionClass>()).extensible_type;
        g_type_class_unref(class);
        found
    }
}

/// A new extensible object with its extensions loaded, and the number of them.
fn load() -> (*mut GObject, usize) {
    // SAFETY: a registered GObject type, no properties. The list holds borrowed
    // pointers; only the list is freed.
    unsafe {
        let object = g_object_new_with_properties(
            register_static::<Extensible>(),
            0,
            ptr::null_mut(),
            ptr::null(),
        );
        e_extensible_load_extensions(object.cast());
        let list = e_extensible_list_extensions(object.cast(), e_extension_get_type());
        let mut count = 0;
        let mut item = list;
        while !item.is_null() {
            count += 1;
            item = (*item).next;
        }
        g_list_free(list);
        (object, count)
    }
}

#[test]
fn the_class_extends_the_extensible_and_the_hook_sees_it() {
    let gtype = register_static::<ExtensionObject<Recorder>>();
    // SAFETY: both are registered types.
    assert_ne!(unsafe { g_type_is_a(gtype, e_extension_get_type()) }, 0);
    assert_eq!(class_extensible_type(gtype), Recorder::extensible_type());

    let before = CALLS.load(Ordering::SeqCst);
    let (object, count) = load();
    assert_eq!(count, 1, "the extension did not load");
    assert_eq!(CALLS.load(Ordering::SeqCst), before + 1);
    assert_eq!(SEEN.load(Ordering::SeqCst), object as usize);
    // SAFETY: the reference that `load` returned.
    unsafe { g_object_unref(object.cast()) };
}

struct Panicker;

impl Extension for Panicker {
    const TYPE_NAME: &'static CStr = c"JmapTestPanickerExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"JmapTestPanickerHost";

    fn extensible_type() -> GType {
        register_static::<PanickerHost>()
    }

    unsafe fn constructed(_extension: *mut EExtension) {
        panic!("the hook panics");
    }
}

#[repr(C)]
struct PanickerHost {
    parent: GObject,
}

#[repr(C)]
struct PanickerHostClass {
    parent_class: GObjectClass,
}

// SAFETY: both structs are #[repr(C)] and start with the GObject structs.
unsafe impl ObjectSubclass for PanickerHost {
    const NAME: &'static CStr = c"JmapTestPanickerHost";
    type Instance = PanickerHost;
    type Class = PanickerHostClass;

    fn parent_type() -> GType {
        G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // SAFETY: no arguments. The type initialises itself.
        vec![InterfaceDecl::defaults(unsafe { e_extensible_get_type() })]
    }
}

/// A second extensible host, kept apart from `Extensible` so
/// `InterfacedExtension` does not change the extension count the other
/// tests in this file assert on.
#[repr(C)]
struct InterfaceHost {
    parent: GObject,
}

#[repr(C)]
struct InterfaceHostClass {
    parent_class: GObjectClass,
}

// SAFETY: both structs are #[repr(C)] and start with the GObject structs.
unsafe impl ObjectSubclass for InterfaceHost {
    const NAME: &'static CStr = c"JmapTestInterfaceHost";
    type Instance = InterfaceHost;
    type Class = InterfaceHostClass;

    fn parent_type() -> GType {
        G_TYPE_OBJECT
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // SAFETY: no arguments. The type initialises itself.
        vec![InterfaceDecl::defaults(unsafe { e_extensible_get_type() })]
    }
}

/// An extension that declares an extra interface, to prove `ExtensionObject`
/// carries it through: the class must implement every interface the
/// `Extension` impl declares, not just the ones `Extensible` does.
struct InterfacedExtension;

impl Extension for InterfacedExtension {
    const TYPE_NAME: &'static CStr = c"JmapTestInterfacedExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"JmapTestInterfaceHost";

    fn extensible_type() -> GType {
        register_static::<InterfaceHost>()
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // SAFETY: no arguments. The type initialises itself.
        vec![InterfaceDecl::defaults(unsafe { g_type_plugin_get_type() })]
    }

    unsafe fn constructed(_extension: *mut EExtension) {}
}

#[test]
fn the_class_implements_the_interfaces_the_extension_declares() {
    let gtype = register_static::<ExtensionObject<InterfacedExtension>>();
    // SAFETY: both are registered types.
    assert_ne!(unsafe { g_type_is_a(gtype, g_type_plugin_get_type()) }, 0);
}

#[test]
fn a_panic_in_the_hook_does_not_leave_constructed() {
    register_static::<ExtensionObject<Panicker>>();
    // SAFETY: a registered GObject type, no properties.
    let object = unsafe {
        g_object_new_with_properties(
            register_static::<PanickerHost>(),
            0,
            ptr::null_mut(),
            ptr::null(),
        )
    };
    // SAFETY: a live extensible object. The guard catches the panic.
    unsafe { e_extensible_load_extensions(object.cast()) };
    // SAFETY: the reference from `g_object_new_with_properties`.
    unsafe { g_object_unref(object.cast()) };
}
