// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapExternalPhotoExtension`: the placeholder for an externally hosted
//! contact photo in the address book preview.
//!
//! A vCard can carry its photo as a URL (`PHOTO;VALUE=uri:https://...`)
//! instead of as bytes. Evolution's contact formatter copies that URL into an
//! `<img src>` without asking. Neither Evolution 3.52.3 nor 3.60.2 actually
//! requests it, so the user sees a broken image icon and cannot read where
//! the image claims to be. This extension puts the URL in front of the user
//! instead.
//!
//! The extensible type is `EWebView`, not `EABContactDisplay`. Evolution
//! installs a header for the parent only: `addressbook/gui/widgets/` ships
//! `eab-config.h` and nothing else, and `libeabwidgets` has no pkg-config
//! file. So the class names the public parent, and `constructed` tests the
//! real type by name. That test is sound exactly there and not in
//! `class_init`: `e_extensible_load_extensions` runs while an instance of the
//! subclass exists, so the subclass is registered by then, whereas
//! `class_init` runs for the first extensible of any type in the process.
//!
//! The extension therefore attaches to every `EWebView` and returns at once
//! for all but the address book preview. One small object per web view is the
//! price of not linking a private library.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use eds_sys::{EExtension, e_extension_get_extensible};
use evo_sys::{EWebView, e_web_view_get_type, e_web_view_jsc_run_script_take};
use glib_sys::{GType, g_strdup, gpointer};
use gobject_sys::{
    GObject, GTypeInstance, g_signal_connect_data, g_type_check_instance_is_a, g_type_from_name,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{N_, translate};
use jmap_backend_core::trampoline::guard;

use crate::external_photo::script;

/// The type name of the address book preview, the one `EWebView` subclass
/// this extension works on.
const CONTACT_DISPLAY: &CStr = c"EABContactDisplay";

/// The text above the URL. It says what the user is looking at, which is the
/// whole of the placeholder's job: no separate warning follows it.
const LABEL: &CStr = N_(c"Externally hosted image");

/// The placeholder for an externally hosted contact photo.
pub struct ExternalPhoto;

/// The registered `JmapExternalPhotoExtension` type.
pub type JmapExternalPhotoExtension = ExtensionObject<ExternalPhoto>;

impl Extension for ExternalPhoto {
    const TYPE_NAME: &'static CStr = c"JmapExternalPhotoExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EWebView";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_web_view_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// Connects the one signal this extension needs, for an address book preview
/// and for nothing else.
unsafe fn constructed(extension: *mut EExtension) {
    // SAFETY: `extension` is live, and its extensible is a live `EWebView`.
    unsafe {
        let view: *mut EWebView = e_extension_get_extensible(extension).cast();
        if !is_contact_display(view) {
            return;
        }
        g_signal_connect_data(
            view.cast(),
            c"content-loaded".as_ptr(),
            gcallback(
                on_content_loaded as unsafe extern "C" fn(*mut GObject, *const c_char, gpointer),
            ),
            ptr::null_mut(),
            None,
            0,
        );
    }
    tracing::trace!("external photo: an address book preview loaded the extension");
}

/// Whether `view` is the address book preview.
///
/// The type is looked up by name on every call. The lookup answers the
/// invalid type until `libeabwidgets` is in the process, and a cached answer
/// from before that moment would never become right.
///
/// # Safety
///
/// `view` must be a live `EWebView`.
unsafe fn is_contact_display(view: *mut EWebView) -> bool {
    // SAFETY: `view` is live per the contract. A name the type system does
    // not know answers 0, which `g_type_check_instance_is_a` rejects.
    unsafe {
        let display = g_type_from_name(CONTACT_DISPLAY.as_ptr());
        display != 0 && g_type_check_instance_is_a(view.cast::<GTypeInstance>(), display) != 0
    }
}

/// `EWebView::content-loaded` has one `G_TYPE_STRING` parameter, the id of
/// the frame that finished (`e-web-view.c`). The handler must declare it, or
/// the id goes into the user-data slot.
///
/// The script is idempotent, so a second frame in the same page costs one
/// script run and changes nothing.
unsafe extern "C" fn on_content_loaded(
    object: *mut GObject,
    _frame_id: *const c_char,
    _user_data: gpointer,
) {
    guard("JmapExternalPhotoExtension::content-loaded", (), || {
        let Ok(script) = CString::new(script::placeholder_script(&translate(LABEL))) else {
            return;
        };
        // SAFETY: `object` is the live `EWebView` that emitted the signal,
        // and an `EWebView` starts with a `WebKitWebView` (checked by
        // `evo-sys`'s `tests/web_view.rs`). The callee frees the script with
        // `g_free`, so the copy it gets comes from GLib, not from Rust.
        unsafe {
            e_web_view_jsc_run_script_take(
                object.cast(),
                g_strdup(script.as_ptr()),
                ptr::null_mut(),
            );
        }
    });
}
