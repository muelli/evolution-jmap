// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapMailVcardPhotoExtension`: the placeholder for an externally hosted
//! vCard photo in the compact, attached-contact render of a mail.
//!
//! Sibling of [`super::display_ext`], the same placeholder for the
//! standalone address book preview. The two cannot share one extension:
//! `eab-contact-formatter.c`'s compact render (used here) always gives the
//! photo a `width` attribute, while its title-block render (the standalone
//! preview) never does, and [`super::script::placeholder_script`] already
//! relies on that attribute's absence to leave this context alone. The
//! compact render also lives inside its own `<iframe>`, a separate document
//! from the one the extension's script runs against by default; see
//! [`super::mail_script`] for how it reaches in.
//!
//! The extensible type is `EWebView`, as for the address book preview, but
//! the gate is `EMailDisplay`: Evolution installs a header for this one
//! (`mail/e-mail-display.h`), so `evo-sys` binds its accessor directly and
//! the gate needs no name lookup.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use eds_sys::{EExtension, e_extension_get_extensible};
use evo_sys::{
    EWebView, GtkAllocation, e_mail_display_get_type, e_web_view_get_type,
    e_web_view_jsc_run_script_take, e_web_view_register_element_clicked,
};
use glib_sys::{GType, g_strdup, gpointer};
use gobject_sys::{GObject, GTypeInstance, g_signal_connect_data, g_type_check_instance_is_a};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{N_, translate};
use jmap_backend_core::trampoline::guard;

use crate::dispatch;
use crate::external_photo::{fetch, mail_script as script};

/// The text above the URL. Same wording as the address book placeholder: it
/// is the same kind of thing, just in a different view.
const LABEL: &CStr = N_(c"Externally hosted image");

/// The text of the button that fetches the photo.
const LOAD_ACTION: &CStr = N_(c"Load image");

/// [`script::LOAD_CLASS`], as the `CStr` the registration call needs.
/// `tests::the_load_class_matches_the_script_module` keeps the two equal.
const LOAD_CLASS: &CStr = c"__jmap-mail-vcard-photo-load";

/// The placeholder for an externally hosted vCard photo in a mail.
pub struct MailVcardPhoto;

/// The registered `JmapMailVcardPhotoExtension` type.
pub type JmapMailVcardPhotoExtension = ExtensionObject<MailVcardPhoto>;

impl Extension for MailVcardPhoto {
    const TYPE_NAME: &'static CStr = c"JmapMailVcardPhotoExtension";
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

/// Connects the one signal this extension needs, for a mail reader's web
/// view and for nothing else.
unsafe fn constructed(extension: *mut EExtension) {
    // SAFETY: `extension` is live, and its extensible is a live `EWebView`.
    unsafe {
        let view: *mut EWebView = e_extension_get_extensible(extension).cast();
        if !is_mail_display(view) {
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
    tracing::trace!("external photo: a mail reader view loaded the extension");
}

/// Whether `view` is the mail reader's own `EWebView` subclass.
///
/// # Safety
///
/// `view` must be a live `EWebView`.
unsafe fn is_mail_display(view: *mut EWebView) -> bool {
    // SAFETY: `view` is live per the contract, and `e_mail_display_get_type`
    // takes no arguments and initialises itself.
    unsafe {
        g_type_check_instance_is_a(view.cast::<GTypeInstance>(), e_mail_display_get_type()) != 0
    }
}

/// `EWebView::content-loaded` has one `G_TYPE_STRING` parameter, the id of
/// the frame that finished (`e-web-view.c`). The handler must declare it, or
/// the id goes into the user-data slot.
///
/// Ignored here: [`script::placeholder_script`] walks every frame itself, so
/// which one just finished loading does not change what the script does.
/// Evolution emits this signal once per frame, including the compact vCard's
/// own iframe, so the script reruns (idempotently) at least once after that
/// iframe's content exists.
unsafe extern "C" fn on_content_loaded(
    object: *mut GObject,
    _frame_id: *const c_char,
    _user_data: gpointer,
) {
    guard("JmapMailVcardPhotoExtension::content-loaded", (), || {
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
        // `e_web_view_register_element_clicked` re-scans every frame for
        // `LOAD_CLASS` elements as each one loads, independent of which
        // frame's `content-loaded` triggered this call.
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
/// clicked, in the compact vCard's own iframe or any other frame. The
/// iframe id is not needed: [`script::loaded_script`] walks every frame to
/// find whichever one still shows the fetched URL.
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
    guard("JmapMailVcardPhotoExtension::element-clicked", (), || {
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
        let fetched = url.clone();
        let work = move || fetch::fetch_external_photo(&fetched);
        let finish = move |owner: *mut GObject, outcome: Result<fetch::Photo, String>| {
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
            let Ok(script) = CString::new(script::loaded_script(&url, &data_uri)) else {
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
