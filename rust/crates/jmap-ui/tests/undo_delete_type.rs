// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the info bar extension: its parent, and the shell view that
// it extends. This binary has one test, because the test must reference the
// class before any other code registers `EShellView`.

use eds_sys::{EExtensionClass, e_extension_get_type};
use evo_sys::e_shell_view_get_type;
use gobject_sys::{g_type_class_ref, g_type_class_unref, g_type_is_a};
use jmap_backend_core::subclass::register_static;
use jmap_ui::undo_delete::shell_ext::JmapUndoDeleteShellExtension;

#[test]
fn the_undo_extension_is_an_extension_of_the_shell_view() {
    let gtype = register_static::<JmapUndoDeleteShellExtension>();
    assert_ne!(gtype, 0, "the extension type was not registered");
    // SAFETY: both are registered types.
    let is_extension = unsafe { g_type_is_a(gtype, e_extension_get_type()) };
    assert_ne!(is_extension, 0, "the type is not an EExtension");

    // SAFETY: a registered class type. The class starts with `EExtensionClass`.
    let extensible = unsafe {
        let class = g_type_class_ref(gtype);
        let extensible = (*class.cast::<EExtensionClass>()).extensible_type;
        g_type_class_unref(class);
        extensible
    };
    // SAFETY: no arguments, and the type initialises itself.
    let shell_view = unsafe { e_shell_view_get_type() };
    assert_eq!(
        extensible, shell_view,
        "class_init did not find EShellView; class_init_types must register it first"
    );
}
