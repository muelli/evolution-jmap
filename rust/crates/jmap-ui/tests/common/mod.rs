// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The check that the `*_type` test binaries share. Each binary has one test,
// because the test must reference the class before other code registers the
// extensible type.

use eds_sys::{EExtensionClass, e_extension_get_type};
use glib_sys::GType;
use gobject_sys::{g_type_class_ref, g_type_class_unref, g_type_is_a};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// Registers `T` and asserts that it is an `EExtension` of `extensible`.
/// `extensible` runs only after the class reference.
pub fn assert_extends<T: ObjectSubclass>(extensible: fn() -> GType, name: &str) {
    let gtype = register_static::<T>();
    assert_ne!(gtype, 0, "the extension type was not registered");
    // SAFETY: both are registered types.
    let is_extension = unsafe { g_type_is_a(gtype, e_extension_get_type()) };
    assert_ne!(is_extension, 0, "the type is not an EExtension");

    // SAFETY: a registered class type. The class starts with `EExtensionClass`.
    let found = unsafe {
        let class = g_type_class_ref(gtype);
        let found = (*class.cast::<EExtensionClass>()).extensible_type;
        g_type_class_unref(class);
        found
    };
    assert_eq!(
        found,
        extensible(),
        "class_init did not find {name}; class_init_types must register it first"
    );
}
