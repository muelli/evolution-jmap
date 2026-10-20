// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One name for each call that has a different shape in the EDS releases that
//! this crate supports.
//!
//! Elsewhere, `eds-sys` exports only what the installed headers declare, and a
//! caller writes each call as that EDS spells it. This fails when two supported
//! releases spell one operation differently. EDS 3.60 removed the format
//! argument of `e_vcard_to_string` and added a version argument to
//! `e_contact_date_to_string`. Without a wrapper, each call site needs its own
//! `#[cfg]`, and a build-script cfg does not reach dependent crates.
//!
//! Thus this module resolves each difference once, and the tree calls the
//! wrapper. `build.rs` feature-detects each `#[cfg]` from the generated
//! bindings (see its `EDS_FEATURES`), not from a version comparison. The two
//! arms are written so that a wrong detection is a compile error on one leg,
//! not a silently different vCard.
//!
//! This module does not hide an API that EDS removed. `CamelFolderSearch` is
//! not in 3.60 because the base class now does all of its work. A fake object
//! here would hide that, so its call site keeps a `#[cfg]` gate and
//! `docs/eds-version-matrix.md` records it. This module contains only
//! operations that both releases have with different spellings.

use crate::{EContactDate, EVCard, GPtrArray, gchar};

/// The vCard 3.0 text of `evc`, as a new string that the caller frees with
/// `g_free`.
///
/// The mapping in `jmap-vcard` expects vCard 3.0, not the default of this EDS.
/// On 3.60, `e_vcard_to_string` with one argument emits the version of the
/// card. An explicit 3.0 request makes the two legs produce the same bytes.
///
/// # Safety
///
/// `evc` must be a valid, non-NULL pointer to a live `EVCard` (or to an object
/// that derives from it, such as an `EContact`).
pub unsafe fn e_vcard_to_string_vcard_30(evc: *mut EVCard) -> *mut gchar {
    #[cfg(eds_vcard_version_enum)]
    // SAFETY: the caller guarantees `evc`. `E_VCARD_VERSION_30` is the name
    // that this EDS uses for vCard 3.0.
    unsafe {
        crate::e_vcard_convert_to_string(evc, crate::E_VCARD_VERSION_30)
    }

    #[cfg(not(eds_vcard_version_enum))]
    // SAFETY: as above, with the spelling before 3.60 of the same request.
    unsafe {
        crate::e_vcard_to_string(evc, crate::EVC_FORMAT_VCARD_30)
    }
}

/// `dt` in the format of a vCard 3.0 `BDAY` or `ANNIVERSARY` line, as a new
/// string that the caller frees with `g_free`.
///
/// As for [`e_vcard_to_string_vcard_30`]: on 3.60 the call takes the version.
/// A default version can make the two legs write different date formats.
///
/// # Safety
///
/// `dt` must be a valid, non-NULL pointer to an `EContactDate`.
pub unsafe fn e_contact_date_to_string_vcard_30(dt: *mut EContactDate) -> *mut gchar {
    #[cfg(eds_vcard_version_enum)]
    // SAFETY: the caller guarantees `dt`.
    unsafe {
        crate::e_contact_date_to_string(dt, crate::E_VCARD_VERSION_30)
    }

    #[cfg(not(eds_vcard_version_enum))]
    // SAFETY: as above. Before 3.60, the call has no version argument.
    unsafe {
        crate::e_contact_date_to_string(dt)
    }
}

/// One row of the summary database: the struct that the `load` and `save`
/// vfuncs of a `CamelMessageInfo` subclass receive.
///
/// 3.52 calls it `CamelMIRecord`. 3.58 changed the name to
/// `CamelStoreDBMessageRecord` and moved it into `camel-store-db.h`. Both
/// structs are public and have the `bdata` field for the column of the
/// provider. The vfunc signatures are otherwise identical, so an alias is
/// sufficient.
#[cfg(camel_summary_records)]
pub type CamelSummaryMessageRecord = crate::CamelMIRecord;
#[cfg(not(camel_summary_records))]
pub type CamelSummaryMessageRecord = crate::CamelStoreDBMessageRecord;

/// The one header record that Camel keeps for each folder, beside those rows.
///
/// `CamelFIRecord` before 3.58, `CamelStoreDBFolderRecord` after it. One of
/// the two vfuncs that take it also changed shape (see `summary_header_save`
/// in the `summary` module of `jmap-mail`). Thus this alias is necessary but
/// not sufficient.
#[cfg(camel_summary_records)]
pub type CamelSummaryFolderRecord = crate::CamelFIRecord;
#[cfg(not(camel_summary_records))]
pub type CamelSummaryFolderRecord = crate::CamelStoreDBFolderRecord;

/// Every uid that has a row in the summary, as an array that the caller owns
/// and frees with [`summary_free_uids`].
///
/// `camel_folder_summary_get_array` up to 3.52, `camel_folder_summary_dup_uids`
/// from 3.58. In both, the array holds its own reference to each uid string.
/// Thus a caller can remove a row from the summary during a walk of the
/// snapshot: the string being read stays valid and the walk is not disturbed.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn summary_dup_uids(summary: *mut crate::CamelFolderSummary) -> *mut GPtrArray {
    #[cfg(camel_summary_records)]
    // SAFETY: the caller guarantees the summary. The accessor takes the lock
    // of the summary.
    unsafe {
        crate::camel_folder_summary_get_array(summary)
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above, with the spelling from 3.58.
    unsafe {
        crate::camel_folder_summary_dup_uids(summary)
    }
}

/// The uids of the rows that still have changes for the server (the
/// `FOLDER_FLAGGED` work list of Camel). Ownership is as for
/// [`summary_dup_uids`].
///
/// `camel_folder_summary_get_changed` up to 3.52,
/// `camel_folder_summary_dup_changed` from 3.58.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn summary_dup_changed(summary: *mut crate::CamelFolderSummary) -> *mut GPtrArray {
    #[cfg(camel_summary_records)]
    // SAFETY: the caller guarantees the summary. The accessor takes the lock
    // of the summary.
    unsafe {
        crate::camel_folder_summary_get_changed(summary)
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above, with the spelling from 3.58.
    unsafe {
        crate::camel_folder_summary_dup_changed(summary)
    }
}

/// The 64-bit hash of a `Message-ID` that Camel uses for threads. Camel
/// computes it, and its summary stores the same value.
///
/// `camel_folder_search_util_hash_message_id` in `camel-folder-search.h`. From
/// 3.58 that header is gone, and the identical function is
/// `camel_search_util_hash_message_id` in `camel-search-utils.h`. Only the name
/// changed: the two arguments and the `guint64` result are the same.
///
/// The `camel_folder_search_object` probe selects the arm, because EDS removed
/// these helpers together with that header. A wrong selection is not silent:
/// each arm names a function, and an EDS without that name fails to compile.
///
/// # Safety
///
/// `message_id` must be NULL or a valid NUL-terminated string.
pub unsafe fn search_hash_message_id(
    message_id: *const gchar,
    needs_decode: glib_sys::gboolean,
) -> u64 {
    #[cfg(camel_folder_search_object)]
    // SAFETY: the caller guarantees the string. The call only reads it.
    unsafe {
        crate::camel_folder_search_util_hash_message_id(message_id, needs_decode)
    }

    #[cfg(not(camel_folder_search_object))]
    // SAFETY: as above, with the name from 3.58.
    unsafe {
        crate::camel_search_util_hash_message_id(message_id, needs_decode)
    }
}

/// Every uid that has a message in the folder, as an array that the caller owns
/// and frees with [`folder_free_uids`].
///
/// Up to 3.52, `camel_folder_get_uids` returns an array of the folder, which
/// `camel_folder_free_uids` gives back. From 3.58, `camel_folder_dup_uids`
/// returns a reference-counted `GPtrArray`. The old pair borrows and the new
/// call copies. Thus they are interchangeable only for a caller that reads the
/// array and then releases it. The wrapper contract requires this, and all
/// callers in this repository do this.
///
/// [`summary_dup_uids`] gives the same data through the summary. Camel changed
/// both names in the same release, but kept two calls.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
pub unsafe fn folder_dup_uids(folder: *mut crate::CamelFolder) -> *mut GPtrArray {
    #[cfg(camel_folder_get_uids)]
    // SAFETY: the caller guarantees the folder. `folder_free_uids` releases the
    // array, and on this release gives it back to the folder.
    unsafe {
        crate::camel_folder_get_uids(folder)
    }

    #[cfg(not(camel_folder_get_uids))]
    // SAFETY: as above, with the spelling from 3.58.
    unsafe {
        crate::camel_folder_dup_uids(folder)
    }
}

/// Releases an array that [`folder_dup_uids`] returned.
///
/// Up to 3.52, the release is a method of the folder,
/// `camel_folder_free_uids(folder, array)`, so a class can get back what its
/// `get_uids` lent. From 3.58, it is an unref. Thus the wrapper also takes the
/// folder.
///
/// # Safety
///
/// `array` must be an array that [`folder_dup_uids`] returned for `folder`,
/// and that is not yet freed.
pub unsafe fn folder_free_uids(folder: *mut crate::CamelFolder, array: *mut GPtrArray) {
    #[cfg(camel_folder_get_uids)]
    // SAFETY: the caller contract makes sure that the array and folder match.
    unsafe {
        crate::camel_folder_free_uids(folder, array);
    }

    #[cfg(not(camel_folder_get_uids))]
    // SAFETY: as above. From 3.58, the caller owns the array, and the release
    // does not use the folder.
    unsafe {
        let _ = folder;
        glib_sys::g_ptr_array_unref(array);
    }
}

/// Releases an array that one of the two summary accessors above returned.
///
/// 3.52 has a dedicated `camel_folder_summary_free_array`. From 3.58, the
/// array is a reference-counted `GPtrArray` with a free function for its
/// elements, so `g_ptr_array_unref` frees the array and its uids. The wrong
/// call causes a silent leak of the strings, thus this wrapper.
///
/// # Safety
///
/// `array` must be an array returned by [`summary_dup_uids`] or
/// [`summary_dup_changed`] and not yet freed.
pub unsafe fn summary_free_uids(array: *mut GPtrArray) {
    #[cfg(camel_summary_records)]
    // SAFETY: the caller guarantees that the array came from the matching
    // accessor.
    unsafe {
        crate::camel_folder_summary_free_array(array);
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above. From 3.58, the array owns its elements, and one unref
    // frees the array and the elements.
    unsafe {
        glib_sys::g_ptr_array_unref(array);
    }
}
