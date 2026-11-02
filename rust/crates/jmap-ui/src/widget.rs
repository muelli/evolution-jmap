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

use evo_sys::{
    AtkRole, GtkWidget, atk_object_get_name, atk_object_get_role, atk_object_set_name,
    atk_object_set_role, gtk_widget_get_accessible,
};
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

/// Sets a named plain integer property, such as a `GtkTreeViewColumn`'s
/// `fixed-width`.
///
/// # Safety
///
/// `object` must be a live GObject with a writable integer property `name`.
pub unsafe fn set_int_named(object: *mut GObject, name: &CStr, value: c_int) {
    // SAFETY: per the contract.
    unsafe {
        g_object_set(object, name.as_ptr(), value, ptr::null::<c_char>());
    }
}

/// Marks a widget's accessible object with `role`, so a screen reader
/// treats a later text or state change as an announcement rather than a
/// silent redraw (a status label wants `ATK_ROLE_STATUSBAR`). Does nothing
/// if the widget has no accessible object.
///
/// # Safety
///
/// `widget` must be a live GTK widget.
pub unsafe fn set_accessible_role(widget: *mut GObject, role: AtkRole) {
    // SAFETY: per the contract. `gtk_widget_get_accessible` never fails for
    // a realized widget type; it is NULL only for a type with no
    // accessible implementation, which this crate does not build.
    unsafe {
        let accessible = gtk_widget_get_accessible(widget.cast::<GtkWidget>());
        if !accessible.is_null() {
            atk_object_set_role(accessible, role);
        }
    }
}

/// The accessible role a widget was given, or `ATK_ROLE_INVALID` if it has
/// no accessible object.
///
/// # Safety
///
/// `widget` must be a live GTK widget.
pub unsafe fn get_accessible_role(widget: *mut GObject) -> AtkRole {
    // SAFETY: per the contract.
    unsafe {
        let accessible = gtk_widget_get_accessible(widget.cast::<GtkWidget>());
        if accessible.is_null() {
            evo_sys::ATK_ROLE_INVALID
        } else {
            atk_object_get_role(accessible)
        }
    }
}

/// Names a widget's accessible object, for one a screen reader could
/// otherwise reach with nothing to announce: a `GtkTreeView` with no
/// preceding label to supply the name through a `labelled-by` relation.
/// Does nothing if the widget has no accessible object.
///
/// # Safety
///
/// `widget` must be a live GTK widget.
pub unsafe fn set_accessible_name(widget: *mut GObject, name: &str) {
    let name = cstring_lossy(name);
    // SAFETY: per the contract. `atk_object_set_name` copies the string.
    unsafe {
        let accessible = gtk_widget_get_accessible(widget.cast::<GtkWidget>());
        if !accessible.is_null() {
            atk_object_set_name(accessible, name.as_ptr());
        }
    }
}

/// The accessible name a widget was given, or an empty string if it has
/// none or no accessible object.
///
/// # Safety
///
/// `widget` must be a live GTK widget.
pub unsafe fn get_accessible_name(widget: *mut GObject) -> String {
    // SAFETY: per the contract. The returned pointer is owned by the
    // AtkObject, not by this call, so it is read, not freed.
    unsafe {
        let accessible = gtk_widget_get_accessible(widget.cast::<GtkWidget>());
        if accessible.is_null() {
            return String::new();
        }
        let name = atk_object_get_name(accessible);
        if name.is_null() {
            String::new()
        } else {
            CStr::from_ptr(name).to_string_lossy().into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_char;

    use evo_sys::{ATK_ROLE_LABEL, ATK_ROLE_STATUSBAR, gtk_label_new, gtk_widget_destroy};

    use super::*;

    unsafe extern "C" {
        fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> gboolean;
    }

    /// A status label starts with GTK's own default role, not a screen
    /// reader's live-region role: `set_accessible_role` is what gives it
    /// one. Needs a display; skips without one.
    #[test]
    fn setting_an_accessible_role_is_visible_on_read_back() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a freshly made label, destroyed before returning.
        unsafe {
            let label = gtk_label_new(ptr::null());
            assert_eq!(
                get_accessible_role(label.cast()),
                ATK_ROLE_LABEL,
                "a plain label should start with GTK's own default role"
            );

            set_accessible_role(label.cast(), ATK_ROLE_STATUSBAR);
            assert_eq!(
                get_accessible_role(label.cast()),
                ATK_ROLE_STATUSBAR,
                "the role set should be the role read back"
            );

            gtk_widget_destroy(label);
        }
    }

    /// A fresh widget has no accessible name of its own: `set_accessible_name`
    /// is what gives one to a widget with no label to supply it through a
    /// `labelled-by` relation. Needs a display; skips without one.
    #[test]
    fn setting_an_accessible_name_is_visible_on_read_back() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a freshly made label, destroyed before returning.
        unsafe {
            let label = gtk_label_new(ptr::null());
            assert_eq!(
                get_accessible_name(label.cast()),
                "",
                "a plain label should start with no accessible name"
            );

            set_accessible_name(label.cast(), "Filter scripts");
            assert_eq!(
                get_accessible_name(label.cast()),
                "Filter scripts",
                "the name set should be the name read back"
            );

            gtk_widget_destroy(label);
        }
    }
}
