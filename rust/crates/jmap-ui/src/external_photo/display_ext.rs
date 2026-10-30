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
use std::os::raw::{c_char, c_void};
use std::ptr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use eds_sys::{EExtension, e_extension_get_extensible};
use evo_sys::{
    EWebView, GtkAllocation, e_web_view_get_type, e_web_view_jsc_run_script_take,
    e_web_view_register_element_clicked,
};
use glib_sys::{GType, g_strdup, gpointer};
use gobject_sys::{
    GObject, GTypeInstance, g_signal_connect_data, g_type_check_instance_is_a, g_type_from_name,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{N_, translate};
use jmap_backend_core::trampoline::guard;

use crate::dispatch;
use crate::external_photo::{fetch, script};

/// The type name of the address book preview, the one `EWebView` subclass
/// this extension works on.
const CONTACT_DISPLAY: &CStr = c"EABContactDisplay";

/// The text above the URL. It says what the user is looking at, which is the
/// whole of the placeholder's job: no separate warning follows it.
const LABEL: &CStr = N_(c"Externally hosted image");

/// The text of the button that fetches the photo.
const LOAD_ACTION: &CStr = N_(c"Load image");

/// [`script::LOAD_CLASS`], as the `CStr` the registration call needs.
/// `tests::the_load_class_matches_the_script_module` keeps the two equal.
const LOAD_CLASS: &CStr = c"__jmap-external-photo-load";

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
///
/// `e_web_view_register_element_clicked` is called here, not in
/// `constructed`: Evolution forgets every registration on each new content
/// load, so the extension must register again every time, and only after
/// the script above has had the chance to create the button.
unsafe extern "C" fn on_content_loaded(
    object: *mut GObject,
    _frame_id: *const c_char,
    _user_data: gpointer,
) {
    guard("JmapExternalPhotoExtension::content-loaded", (), || {
        let Ok(placeholder) = CString::new(script::placeholder_script(
            &translate(LABEL),
            &translate(LOAD_ACTION),
        )) else {
            return;
        };
        // SAFETY: `object` is the live `EWebView` that emitted the signal,
        // and an `EWebView` starts with a `WebKitWebView` (checked by
        // `evo-sys`'s `tests/web_view.rs`). The callee frees the script with
        // `g_free`, so the copy it gets comes from GLib, not from Rust.
        // `e_web_view_register_element_clicked` re-scans the current
        // document for `LOAD_CLASS` elements once it is called, which is
        // why it runs after the script above, not before.
        unsafe {
            e_web_view_jsc_run_script_take(
                object.cast(),
                g_strdup(placeholder.as_ptr()),
                ptr::null_mut(),
            );
            e_web_view_register_element_clicked(
                object.cast(),
                LOAD_CLASS.as_ptr(),
                Some(on_load_clicked),
                ptr::null_mut(),
            );
        }
    });
}

/// `EWebViewElementClickedFunc`: called when the "Load image" button is
/// clicked. `element_value` is the button's `value` attribute, the photo URL
/// copied verbatim from the document by the placeholder script.
///
/// # Safety
///
/// `web_view` is the live `EWebView` that is clicked in. Evolution owns it
/// for the duration of this call, but does not keep it alive beyond that, so
/// the fetch itself only touches `web_view` again through the weak reference
/// [`dispatch::spawn_for`] takes.
unsafe extern "C" fn on_load_clicked(
    web_view: *mut EWebView,
    _iframe_id: *const c_char,
    _element_id: *const c_char,
    _element_class: *const c_char,
    element_value: *const c_char,
    _element_position: *const GtkAllocation,
    _user_data: *mut c_void,
) {
    guard("JmapExternalPhotoExtension::element-clicked", (), || {
        if element_value.is_null() {
            return;
        }
        // SAFETY: GLib hands this callback a NUL-terminated string it owns;
        // this code does not keep the pointer past the call.
        let Ok(url) = unsafe { CStr::from_ptr(element_value) }.to_str() else {
            return;
        };
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            // The placeholder script only ever gives this button an http(s)
            // URL. A mismatch here means the document changed under it.
            return;
        }
        let url = url.to_owned();
        let work = move || fetch::fetch_external_photo(&url);
        let finish = |owner: *mut GObject, outcome: Result<fetch::Photo, String>| {
            let photo = match outcome {
                Ok(photo) => photo,
                Err(message) => {
                    tracing::debug!(%message, "external photo: the fetch did not produce one");
                    return;
                }
            };
            let data_uri = format!(
                "data:{};base64,{}",
                photo.media_type,
                BASE64.encode(photo.bytes)
            );
            let Ok(script) = CString::new(script::loaded_script(&data_uri)) else {
                return;
            };
            // SAFETY: `owner` is the live `EWebView` that `spawn_for`'s weak
            // reference resolved, and an `EWebView` starts with a
            // `WebKitWebView`, as above. The callee frees the script with
            // `g_free`.
            unsafe {
                e_web_view_jsc_run_script_take(
                    owner.cast(),
                    g_strdup(script.as_ptr()),
                    ptr::null_mut(),
                );
            }
        };
        // SAFETY: `web_view` is live for the duration of this call, per the
        // contract of this function.
        unsafe { dispatch::spawn_for(web_view.cast(), work, finish) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registration call and the script that creates the button must
    /// name the same class, or Evolution never binds the click.
    #[test]
    fn the_load_class_matches_the_script_module() {
        assert_eq!(LOAD_CLASS.to_str().unwrap(), script::LOAD_CLASS);
    }
}
