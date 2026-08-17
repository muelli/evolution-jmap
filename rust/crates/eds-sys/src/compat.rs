// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The calls whose *shape* differs between the EDS releases this crate is
//! generated against, behind one name each.
//!
//! `eds-sys` is otherwise a pure bindgen surface: whatever the installed
//! headers declare is what it exports, and a caller writes the call the way
//! that EDS spells it. That stops working the moment two supported releases
//! spell the same operation differently — EDS 3.60 dropped
//! `e_vcard_to_string`'s format argument and gave
//! `e_contact_date_to_string` a version one — because then every call site
//! would need its own `#[cfg]`, and a build-script cfg does not reach the
//! crates that depend on this one anyway.
//!
//! So the difference is resolved here, once, and the rest of the tree calls
//! the wrapper. Each wrapper is chosen by a `#[cfg]` that `build.rs`
//! feature-detected from the generated bindings (see its `EDS_FEATURES`), not
//! from a version comparison — and the two arms are written so that a wrong
//! detection is a *compile* error on one leg rather than a silently different
//! vCard on it.
//!
//! What this module is deliberately not: a place to paper over an API EDS
//! genuinely removed. `CamelFolderSearch` is gone from 3.60 because the whole
//! job moved into the base class, and faking the object here would hide that;
//! that one stays `#[cfg]`-gated at its call site and recorded in
//! `docs/eds-version-matrix.md`. What *is* here is the narrower case: an
//! operation both releases still have, spelled differently.

use crate::{EContactDate, EVCard, GPtrArray, gchar};

/// The vCard 3.0 text of `evc`, as a newly allocated string the caller frees
/// with `g_free`.
///
/// vCard 3.0 rather than whatever this EDS defaults to, because 3.0 is the
/// version the whole mapping in `jmap-vcard` is written against — its parser
/// documents itself as reading what `EVCard` emits — and on 3.60 the
/// one-argument `e_vcard_to_string` emits the card's *own* version instead.
/// Asking for 3.0 explicitly is what keeps the two legs producing the same
/// bytes.
///
/// # Safety
///
/// `evc` must be a valid, non-NULL pointer to a live `EVCard` (or to an object
/// that derives from one, such as an `EContact`).
pub unsafe fn e_vcard_to_string_vcard_30(evc: *mut EVCard) -> *mut gchar {
    #[cfg(eds_vcard_version_enum)]
    // SAFETY: the caller guarantees `evc`; `E_VCARD_VERSION_30` is this EDS's
    // own name for vCard 3.0.
    unsafe {
        crate::e_vcard_convert_to_string(evc, crate::E_VCARD_VERSION_30)
    }

    #[cfg(not(eds_vcard_version_enum))]
    // SAFETY: as above, with the pre-3.60 spelling of the same request.
    unsafe {
        crate::e_vcard_to_string(evc, crate::EVC_FORMAT_VCARD_30)
    }
}

/// `dt` written the way a vCard 3.0 `BDAY`/`ANNIVERSARY` line states it, as a
/// newly allocated string the caller frees with `g_free`.
///
/// Same reasoning as [`e_vcard_to_string_vcard_30`]: on 3.60 the call takes
/// the version to write for, and leaving it to a default would let the two
/// legs disagree about the format of a date.
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
    // SAFETY: as above; pre-3.60 there is no version to state.
    unsafe {
        crate::e_contact_date_to_string(dt)
    }
}

/// One row of the summary database, as the struct a `CamelMessageInfo`
/// subclass's `load`/`save` vfuncs are handed.
///
/// 3.52 calls it `CamelMIRecord`; 3.58 renamed it `CamelStoreDBMessageRecord`
/// and moved it into `camel-store-db.h`. Both are public structs with the same
/// `bdata` field a provider keeps its own column in, and both vfunc signatures
/// are otherwise identical — so the difference really is just the name, and an
/// alias is the whole of it.
#[cfg(camel_summary_records)]
pub type CamelSummaryMessageRecord = crate::CamelMIRecord;
#[cfg(not(camel_summary_records))]
pub type CamelSummaryMessageRecord = crate::CamelStoreDBMessageRecord;

/// The one header record Camel keeps per folder beside those rows.
///
/// `CamelFIRecord` before 3.58, `CamelStoreDBFolderRecord` after it. Unlike the
/// message record, the *shape* of one of the two vfuncs taking it changed as
/// well — see `summary_header_save` in `jmap-mail`'s `summary` module — so this
/// alias is necessary but not sufficient.
#[cfg(camel_summary_records)]
pub type CamelSummaryFolderRecord = crate::CamelFIRecord;
#[cfg(not(camel_summary_records))]
pub type CamelSummaryFolderRecord = crate::CamelStoreDBFolderRecord;

/// Every uid the summary holds a row for, as an array the caller owns and frees
/// with [`summary_free_uids`].
///
/// `camel_folder_summary_get_array` up to 3.52, `camel_folder_summary_dup_uids`
/// from 3.58. The two agree on the thing that makes a caller's walk safe, which
/// is why one wrapper can stand for both: the array holds a reference of its own
/// to every uid string in it, so removing a row from the summary while walking
/// the snapshot neither frees the string being read nor disturbs the walk.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn summary_dup_uids(summary: *mut crate::CamelFolderSummary) -> *mut GPtrArray {
    #[cfg(camel_summary_records)]
    // SAFETY: the caller guarantees the summary; the accessor takes the
    // summary's own lock.
    unsafe {
        crate::camel_folder_summary_get_array(summary)
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above, with the post-3.58 spelling.
    unsafe {
        crate::camel_folder_summary_dup_uids(summary)
    }
}

/// The uids of the rows the summary holds as still owing something to the
/// server — Camel's `FOLDER_FLAGGED` work list — same ownership as
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
    // SAFETY: the caller guarantees the summary; the accessor takes the
    // summary's own lock.
    unsafe {
        crate::camel_folder_summary_get_changed(summary)
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above, with the post-3.58 spelling.
    unsafe {
        crate::camel_folder_summary_dup_changed(summary)
    }
}

/// The 64 bits Camel threads on, hashed out of a `Message-ID` — the same value
/// its own summary stores, computed by Camel rather than reimplemented.
///
/// `camel_folder_search_util_hash_message_id` while it lived in
/// `camel-folder-search.h`; from 3.58 that header is gone and the identical
/// function is `camel_search_util_hash_message_id` in `camel-search-utils.h`.
/// Only the name moved: same two arguments, same `guint64` out.
///
/// Chosen by the same `camel_folder_search_object` probe as the object itself,
/// because these helpers were removed *with* that header rather than
/// independently of it. A wrong guess there cannot be silent: whichever arm is
/// selected names a function, and an EDS that has neither under that name fails
/// to compile.
///
/// # Safety
///
/// `message_id` must be NULL or a valid NUL-terminated string.
pub unsafe fn search_hash_message_id(
    message_id: *const gchar,
    needs_decode: glib_sys::gboolean,
) -> u64 {
    #[cfg(camel_folder_search_object)]
    // SAFETY: the caller guarantees the string; the call only reads it.
    unsafe {
        crate::camel_folder_search_util_hash_message_id(message_id, needs_decode)
    }

    #[cfg(not(camel_folder_search_object))]
    // SAFETY: as above, under the name 3.58 moved it to.
    unsafe {
        crate::camel_search_util_hash_message_id(message_id, needs_decode)
    }
}

/// Every uid the *folder* has a message for, as an array the caller owns and
/// frees with [`folder_free_uids`].
///
/// `camel_folder_get_uids` up to 3.52 — where the array is the folder's own and
/// is handed back to it with `camel_folder_free_uids` — and
/// `camel_folder_dup_uids` from 3.58, where it is an ordinary
/// reference-counted `GPtrArray`. Because the older pair borrows and the newer
/// one copies, the two are only interchangeable for a caller that reads the
/// array and then releases it, which is what the wrapper's contract says and
/// what every caller in this repository does.
///
/// This is the folder's view; [`summary_dup_uids`] is the summary's. They answer
/// the same question through different objects, and Camel renamed both in the
/// same release without making them one call.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
pub unsafe fn folder_dup_uids(folder: *mut crate::CamelFolder) -> *mut GPtrArray {
    #[cfg(camel_folder_get_uids)]
    // SAFETY: the caller guarantees the folder; the array is released through
    // `folder_free_uids`, which on this release hands it back to the folder.
    unsafe {
        crate::camel_folder_get_uids(folder)
    }

    #[cfg(not(camel_folder_get_uids))]
    // SAFETY: as above, with the post-3.58 spelling.
    unsafe {
        crate::camel_folder_dup_uids(folder)
    }
}

/// Releases an array [`folder_dup_uids`] handed back.
///
/// Takes the folder as well as the array because up to 3.52 the release is a
/// method *on the folder* — `camel_folder_free_uids(folder, array)`, which lets
/// a class reclaim what its `get_uids` lent out — while from 3.58 it is a plain
/// unref. Keeping the folder in the signature is what lets one wrapper stand for
/// both.
///
/// # Safety
///
/// `array` must be an array [`folder_dup_uids`] returned for `folder`, and not
/// yet freed.
pub unsafe fn folder_free_uids(folder: *mut crate::CamelFolder, array: *mut GPtrArray) {
    #[cfg(camel_folder_get_uids)]
    // SAFETY: the pair is matched by the caller's contract.
    unsafe {
        crate::camel_folder_free_uids(folder, array);
    }

    #[cfg(not(camel_folder_get_uids))]
    // SAFETY: as above; from 3.58 the array is the caller's own and the folder
    // has no part in releasing it.
    unsafe {
        let _ = folder;
        glib_sys::g_ptr_array_unref(array);
    }
}

/// Releases an array either summary accessor above handed back.
///
/// The two releases differ in more than a name here: 3.52 has a dedicated
/// `camel_folder_summary_free_array`, while from 3.58 the array is an ordinary
/// reference-counted `GPtrArray` carrying a free function for its elements, so
/// `g_ptr_array_unref` is what frees both it and the uids in it. Calling the
/// wrong one of those would leak the strings rather than fail visibly, which is
/// why this is a wrapper and not a note in a comment.
///
/// # Safety
///
/// `array` must be an array returned by [`summary_dup_uids`] or
/// [`summary_dup_changed`] and not yet freed.
pub unsafe fn summary_free_uids(array: *mut GPtrArray) {
    #[cfg(camel_summary_records)]
    // SAFETY: the caller guarantees the array came from the matching accessor.
    unsafe {
        crate::camel_folder_summary_free_array(array);
    }

    #[cfg(not(camel_summary_records))]
    // SAFETY: as above; from 3.58 the array owns its elements and one unref
    // frees both levels.
    unsafe {
        glib_sys::g_ptr_array_unref(array);
    }
}
