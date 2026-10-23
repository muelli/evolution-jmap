// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `E_SOURCE_PARAM_SETTING` is a `#define` that bindgen does not supply, so
// `eds-sys` writes it by hand. This test compares it with the flags that EDS
// itself sets on its own properties.

use eds_sys::*;
use std::ffi::CStr;

/// The flags of the property `name` of the class of `gtype`.
fn flags_of(gtype: GType, name: &CStr) -> GParamFlags {
    // SAFETY: `gtype` is a classed type; the class ref keeps the class alive
    // while the lookup reads it, and `name` is a NUL-terminated string.
    unsafe {
        let klass = g_type_class_ref(gtype);
        assert!(!klass.is_null(), "g_type_class_ref returned NULL");
        let pspec = g_object_class_find_property(klass.cast(), name.as_ptr());
        assert!(!pspec.is_null(), "no property {name:?}");
        let flags = (*pspec).flags;
        g_type_class_unref(klass);
        flags
    }
}

// One test, not two: two threads that register `ESource` and class-ref a
// subclass at the same time can deadlock inside GLib's type system.
#[test]
fn the_setting_flag_marks_only_the_properties_eds_saves_in_the_source_file() {
    // SAFETY: a `*_get_type` function only registers and returns the type.
    let (source, authentication) =
        unsafe { (e_source_get_type(), e_source_authentication_get_type()) };
    let host = flags_of(authentication, c"host");
    assert_ne!(host & E_SOURCE_PARAM_SETTING, 0, "host is a saved setting");
    let uid = flags_of(source, c"uid");
    assert_eq!(uid & E_SOURCE_PARAM_SETTING, 0, "uid is not a setting");
}
