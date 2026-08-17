// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rust values ↔ the C types the `EBookMetaBackend` vfuncs traffic in.
//!
//! Every function here allocates with GLib and hands ownership to the caller,
//! because that is what the vfunc contract says: EDS frees an
//! `out_existing_objects` list with `e_book_meta_backend_info_free` and a
//! removed-uid list with `g_free`. Pointing a node at a Rust `String` instead
//! of copying it would therefore not be a leak, it would be a double free in
//! someone else's process.
//!
//! What is *not* here is the half of the boundary that has nothing to do with
//! contacts — reading and writing out-parameters, and finding the password EDS
//! fetched from libsecret. That lives in [`jmap_backend_core::marshal`], where
//! the calendar backend reaches the same code rather than a copy of it.

use std::ffi::CStr;
use std::ptr;

use eds_sys::compat::e_vcard_to_string_vcard_30;
use eds_sys::{
    E_CONTACT_UID, EContact, EVCard, e_book_meta_backend_info_new, e_contact_get_const,
    e_contact_new_from_vcard,
};
use glib_sys::{GSList, g_free, g_slist_prepend, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::dup_string;
use jmap_book_sync::ContactInfo;

/// Wraps `infos` as a `GSList` of `EBookMetaBackendInfo`, the payload
/// `list_existing_sync` and `get_changes_sync` hand back. An empty slice is
/// the NULL list, which is what EDS reads as "no objects".
///
/// The `extra` field stays NULL: it is per-object opaque state a backend can
/// park in the EDS cache, and this backend has none — the JMAP id *is* the
/// uid, and the revision already carries the change token.
pub fn info_list(infos: &[ContactInfo]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is the only O(1) GSList insertion, so walk backwards and the
    // result comes out in the order the caller gave.
    for info in infos.iter().rev() {
        let uid = cstring_lossy(&info.uid);
        let revision = cstring_lossy(&info.revision);
        let object = cstring_lossy(&info.vcard);
        // SAFETY: the three pointers are valid for the call, which copies
        // each of them; a NULL `extra` is explicitly allowed.
        let node = unsafe {
            e_book_meta_backend_info_new(
                uid.as_ptr(),
                revision.as_ptr(),
                object.as_ptr(),
                ptr::null(),
            )
        };
        // SAFETY: `list` is a valid GSList (initially the empty one) and
        // `node` is a fresh allocation ownership of which passes to it.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// Wraps `uids` as a `GSList` of freshly allocated strings — the shape of
/// `out_removed_objects`, which EDS frees with `g_free`.
pub fn uid_list(uids: &[String]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for uid in uids.iter().rev() {
        // SAFETY: as above; `dup_string` yields a GLib allocation.
        list = unsafe { g_slist_prepend(list, dup_string(uid).cast()) };
    }
    list
}

/// Parses a vCard into an `EContact`, or NULL if the text is not one.
///
/// `EVCard` parses lazily and answers garbage with an empty card rather than
/// with an error, which would reach Evolution as a contact that exists and
/// has no properties. The guard is deliberately only the RFC 6350 §6.1.1
/// envelope — this is not a second vCard parser, it is a check that the thing
/// being handed over claims to be a vCard at all.
pub fn contact_from_vcard(vcard: &str) -> *mut EContact {
    if !looks_like_vcard(vcard) {
        return ptr::null_mut();
    }
    let text = cstring_lossy(vcard);
    // SAFETY: `text` is a valid NUL-terminated string for the duration of the
    // call, which copies what it needs.
    unsafe { e_contact_new_from_vcard(text.as_ptr()) }
}

fn looks_like_vcard(vcard: &str) -> bool {
    vcard
        .trim_start()
        .lines()
        .next()
        .is_some_and(|line| line.trim_end().eq_ignore_ascii_case("BEGIN:VCARD"))
}

/// Renders `contact` back to vCard 3.0 text.
///
/// 3.0 and not 4.0 because `EVCardFormat` has exactly one member: anything
/// else would be reparsed as 3.0 on the way back in.
///
/// # Safety
///
/// `contact` must be a valid `EContact`.
pub unsafe fn vcard_from_contact(contact: *mut EContact) -> Option<String> {
    if contact.is_null() {
        return None;
    }
    // SAFETY: EContact derives from EVCard, so the cast is the C upcast, and
    // the returned string is a GLib allocation this call takes ownership of.
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

/// The contact's `UID`, or `None` if it has none.
///
/// Absent has to stay distinguishable from empty: `save_contact_sync` tells a
/// create from an edit by whether EDS gave the contact a uid, and an empty
/// string would be sent to the server as an identifier.
///
/// # Safety
///
/// `contact` must be a valid `EContact`.
pub unsafe fn contact_uid(contact: *mut EContact) -> Option<String> {
    if contact.is_null() {
        return None;
    }
    // SAFETY: the returned string is owned by the contact and is valid at
    // least until it is mutated, which nothing here does.
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
/// `contact` must be NULL or a valid `EContact` this caller owns a reference
/// to.
pub unsafe fn contact_unref(contact: *mut EContact) {
    if !contact.is_null() {
        // SAFETY: EContact is a GObject and the caller owns the reference.
        unsafe { g_object_unref(contact.cast()) }
    }
}
