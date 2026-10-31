// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Widget values read and written as GObject properties.
//!
//! `GtkEntry` and `GtkTextBuffer` both have a string `text` property, and
//! `GtkToggleButton` has a boolean `active` property. The property system
//! copies the value, so no `GtkTextIter` and no widget struct crosses the
//! FFI boundary. The account-editor pages of this crate share these four
//! calls.

use std::ffi::{CStr, c_char};
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
