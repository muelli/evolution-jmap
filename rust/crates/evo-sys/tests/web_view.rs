// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The web view surface that jmap-ui's external photo extension stands on.
//
// Evolution installs no header for `EABContactDisplay`, the address book
// preview. The extension therefore names `EWebView`, the preview's public
// parent, as its extensible type, and tests the real type by name when it is
// constructed. Three facts must hold for that to work, and none of them
// fails loudly on its own:
//
// - `e_web_view_get_type` exists in the libraries this crate links. A
//   missing entry point becomes an `undefined symbol` the moment Evolution
//   dlopens the module.
// - The type it answers is registered under the name `EWebView`. The
//   extension's `class_init` looks the extensible type up by that name.
// - An `EWebView *` really is a `WebKitWebView *`. The script call casts one
//   to the other, the way C's `WEBKIT_WEB_VIEW()` does.
//
// No test here constructs a web view. A `GtkWidget` needs a display
// connection that this machine does not have, as `tests/gtk.rs` explains.

use evo_sys::*;
use std::ffi::CStr;
use std::mem::size_of;

/// The type accessor answers, and answers under the name the extension looks
/// the extensible type up by.
#[test]
fn the_web_view_is_registered_as_e_web_view() {
    // SAFETY: no arguments, and the type initialises itself.
    let gtype = unsafe { e_web_view_get_type() };
    assert_ne!(gtype, 0, "e_web_view_get_type() answered the invalid type");
    // SAFETY: `gtype` is registered, as the code just asserted. The name
    // belongs to the type system and outlives the call.
    let name = unsafe { CStr::from_ptr(g_type_name(gtype)) };
    assert_eq!(
        name, c"EWebView",
        "the web view is no longer registered as EWebView"
    );
}

/// The cast that `e_web_view_jsc_run_script_take`'s call site makes.
#[test]
fn the_web_view_is_a_webkit_web_view() {
    // SAFETY: no arguments, and the type initialises itself.
    let gtype = unsafe { e_web_view_get_type() };
    // SAFETY: `gtype` is registered. A registered type has a parent unless
    // it is a fundamental type, and this one is not.
    let parent = unsafe { CStr::from_ptr(g_type_name(g_type_parent(gtype))) };
    assert_eq!(
        parent, c"WebKitWebView",
        "EWebView no longer derives directly from WebKitWebView"
    );
}

/// The preview is also a widget, which is what puts it in the shell at all.
#[test]
fn the_web_view_is_a_widget() {
    // SAFETY: neither call takes arguments, and each type initialises
    // itself.
    unsafe {
        assert_ne!(
            g_type_is_a(e_web_view_get_type(), gtk_widget_get_type()),
            0,
            "EWebView is no longer a GtkWidget"
        );
    }
}

/// Both classes stay opaque handles. A handle with a nonzero size means
/// somebody generated a layout for a struct whose shape belongs to WebKit.
#[test]
fn both_web_view_classes_stay_opaque() {
    assert_eq!(size_of::<EWebView>(), 0, "EWebView carries a layout now");
    assert_eq!(
        size_of::<WebKitWebView>(),
        0,
        "WebKitWebView carries a layout now"
    );
}
