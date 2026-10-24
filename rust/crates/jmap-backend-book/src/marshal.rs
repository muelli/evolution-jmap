// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Marshaling between Rust types and `EBookMetaBackend` C representations.
//! Allocates with GLib and transfers ownership to callers per EDS contracts.

use std::ffi::CStr;
use std::ptr;

use eds_sys::compat::e_vcard_to_string_vcard_30;
use eds_sys::{
    E_CONTACT_UID, EBookMetaBackendInfo, EContact, EVCard, e_book_meta_backend_info_new,
    e_contact_get_const, e_contact_new_from_vcard,
};
use glib_sys::{GSList, g_free, g_slist_prepend, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::{dup_string, read_string};

use jmap_book_sync::{ContactInfo, edit_base};

/// Wraps `infos` as an allocated `GSList` of `EBookMetaBackendInfo`. The extra of
/// each node is its vCard: EDS stores it with the contact as the base of an edit.
/// The object carries the stamp of that base (see [`edit_base`]).
pub fn info_list(infos: &[ContactInfo]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is O(1); iterate in reverse order to preserve input sequence.
    for info in infos.iter().rev() {
        let uid = cstring_lossy(&info.uid);
        let revision = cstring_lossy(&info.revision);
        let object = cstring_lossy(&stamped(info));
        let extra = cstring_lossy(&info.vcard);
        // SAFETY: Pointers are valid for the call, which copies them.
        let node = unsafe {
            e_book_meta_backend_info_new(
                uid.as_ptr(),
                revision.as_ptr(),
                object.as_ptr(),
                extra.as_ptr(),
            )
        };
        // SAFETY: `list` is a valid GSList and `node` is a fresh allocation.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// The vCard of `info` with the stamp of its revision.
pub fn stamped(info: &ContactInfo) -> String {
    edit_base::stamp(&info.vcard, "VCARD", &info.revision)
}

/// Adds the server versions that the cache keeps to the extra of each node of `list`.
///
/// `cached` gives the cached extra for a UID. Without it, an editor that is open
/// across this refresh has no base for its save.
///
/// # Safety
///
/// `list` must be null or a valid `GSList` of `EBookMetaBackendInfo` that [`info_list`] made.
pub unsafe fn keep_versions(list: *mut GSList, cached: impl Fn(*const gchar) -> Option<String>) {
    let mut link = list;
    while !link.is_null() {
        // SAFETY: the caller guarantees a valid list of infos with GLib-allocated strings.
        unsafe {
            if let Some(info) = (*link).data.cast::<EBookMetaBackendInfo>().as_mut()
                && let Some(current) = read_string(info.extra)
                && let Some(previous) = cached(info.uid)
            {
                g_free(info.extra.cast());
                info.extra = dup_string(&edit_base::extra(&current, Some(&previous)));
            }
            link = (*link).next;
        }
    }
}

/// Wraps removed `uids` as a `GSList` of `EBookMetaBackendInfo` with only the `uid` set.
pub fn removed_info_list(uids: &[String]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for uid in uids.iter().rev() {
        let uid = cstring_lossy(uid);
        // SAFETY: `uid` is valid for the call, which copies it; NULL is allowed for other fields.
        let node = unsafe {
            e_book_meta_backend_info_new(uid.as_ptr(), ptr::null(), ptr::null(), ptr::null())
        };
        // SAFETY: As in `info_list`.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// Parses a vCard string into a new `EContact`, or NULL if invalid or missing `BEGIN:VCARD`.
pub fn contact_from_vcard(vcard: &str) -> *mut EContact {
    if !looks_like_vcard(vcard) {
        return ptr::null_mut();
    }
    let text = cstring_lossy(vcard);
    // SAFETY: `text` is a valid NUL-terminated string for the duration of the call.
    unsafe { e_contact_new_from_vcard(text.as_ptr()) }
}

fn looks_like_vcard(vcard: &str) -> bool {
    vcard
        .trim_start()
        .lines()
        .next()
        .is_some_and(|line| line.trim_end().eq_ignore_ascii_case("BEGIN:VCARD"))
}

/// Formats `contact` to a vCard 3.0 string.
///
/// # Safety
///
/// `contact` must be a valid `EContact`.
pub unsafe fn vcard_from_contact(contact: *mut EContact) -> Option<String> {
    if contact.is_null() {
        return None;
    }
    // SAFETY: EContact derives from EVCard; caller takes ownership of the returned string.
    unsafe {
        let raw = e_vcard_to_string_vcard_30(contact.cast::<EVCard>());
        if raw.is_null() {
            return None;
        }
        let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        Some(text)
    }
}

/// Extracts the contact's `UID` field, returning `None` if missing or empty.
///
/// # Safety
///
/// `contact` must be a valid `EContact`.
pub unsafe fn contact_uid(contact: *mut EContact) -> Option<String> {
    if contact.is_null() {
        return None;
    }
    // SAFETY: The returned string is valid until the contact is mutated.
    let uid = unsafe { e_contact_get_const(contact, E_CONTACT_UID).cast::<gchar>() };
    if uid.is_null() {
        return None;
    }
    let uid = unsafe { CStr::from_ptr(uid) }
        .to_string_lossy()
        .into_owned();
    (!uid.is_empty()).then_some(uid)
}

/// Drops a reference taken by [`contact_from_vcard`].
///
/// # Safety
///
/// `contact` must be NULL or a valid `EContact` reference owned by caller.
pub unsafe fn contact_unref(contact: *mut EContact) {
    if !contact.is_null() {
        // SAFETY: EContact is a GObject and caller owns the reference.
        unsafe { g_object_unref(contact.cast()) }
    }
}
