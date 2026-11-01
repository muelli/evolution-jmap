// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Widget values read and written as GObject properties.
//!
//! `GtkEntry` and `GtkTextBuffer` both have a string `text` property, and
//! `GtkToggleButton` has a boolean `active` property. The property system
//! copies the value, so no `GtkTextIter` and no widget struct crosses the
//! FFI boundary. The account-editor pages of this crate share these calls,
//! plus two more that take the property name directly, for a label or text
//! view whose wrapping or scrolling property is not `active`.

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use glib_sys::{GFALSE, GTRUE, gboolean};
use gobject_sys::{GObject, g_object_get, g_object_set};
use jmap_backend_core::error::cstring_lossy;

/// Reads the string `text` property.
///
/// # Safety
///
/// `object` must be a live GObject with a readable string `text` property.
pub unsafe fn get_text(object: *mut GObject) -> String {
    let mut value: *mut c_char = ptr::null_mut();
    // SAFETY: per the contract. g_object_get copies the property value, and
    // the NULL sentinel ends the varargs.
    unsafe {
        g_object_get(
            object,
            c"text".as_ptr(),
            &mut value as *mut *mut c_char,
            ptr::null::<c_char>(),
        );
    }
    if value.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated copy that this call owns. It is freed below.
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the copy from g_object_get.
    unsafe { glib_sys::g_free(value.cast()) };
    text
}

/// # Safety
///
/// `object` must be a live GObject with a writable string `text` property.
pub unsafe fn set_text(object: *mut GObject, text: &str) {
    let text = cstring_lossy(text);
    // SAFETY: per the contract. The property system copies the string.
    unsafe {
        g_object_set(
            object,
            c"text".as_ptr(),
            text.as_ptr(),
            ptr::null::<c_char>(),
        );
    }
}

/// # Safety
///
/// `object` must be a live GObject with a readable boolean `active` property.
pub unsafe fn get_bool(object: *mut GObject) -> bool {
    let mut value: gboolean = GFALSE;
    // SAFETY: per the contract.
    unsafe {
        g_object_get(
            object,
            c"active".as_ptr(),
            &mut value as *mut gboolean,
            ptr::null::<c_char>(),
        );
    }
    value != GFALSE
}

/// # Safety
///
/// `object` must be a live GObject with a writable boolean `active` property.
pub unsafe fn set_bool(object: *mut GObject, active: bool) {
    // SAFETY: per the contract.
    unsafe {
        g_object_set(
            object,
            c"active".as_ptr(),
            if active { GTRUE } else { GFALSE },
            ptr::null::<c_char>(),
        );
    }
}

/// `gboolean` for a Rust `bool`, for the many GTK setters that take one.
pub fn flag(value: bool) -> gboolean {
    if value { GTRUE } else { GFALSE }
}

/// Pango's `wrap-mode` value that breaks a line with no space, read by
/// `GtkLabel`'s `wrap-mode` property (`pango-layout.h`: WORD = 0, CHAR = 1,
/// WORD_CHAR = 2). A plain integer, so this needs no new FFI type.
pub const PANGO_WRAP_WORD_CHAR: c_int = 2;

/// Sets a named boolean property, for a widget whose property is not
/// `active`.
///
/// # Safety
///
/// `object` must be a live GObject with a writable boolean property `name`.
pub unsafe fn set_bool_named(object: *mut GObject, name: &CStr, value: bool) {
    // SAFETY: per the contract. The property system copies the value.
    unsafe {
        g_object_set(object, name.as_ptr(), flag(value), ptr::null::<c_char>());
    }
}

/// Sets a named enum property to a plain C integer value, such as
/// `wrap-mode` or `hscrollbar-policy`. No enum type crosses the FFI
/// boundary: the caller passes the header's own integer constant.
///
/// # Safety
///
/// `object` must be a live GObject with a writable enum property `name`.
pub unsafe fn set_enum_named(object: *mut GObject, name: &CStr, value: c_int) {
    // SAFETY: per the contract.
    unsafe {
        g_object_set(object, name.as_ptr(), value, ptr::null::<c_char>());
    }
}
