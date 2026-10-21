// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `expunge_sync`: the vfunc that removes the messages that the user deleted.
//!
//! ## Why the deleted mark goes to this vfunc
//!
//! `CAMEL_MESSAGE_DELETED` is a Camel flag. JMAP has no deleted keyword (refer
//! to `FLAGS_FROM_JMAP` in [`crate::message_info`]). Thus [`crate::synchronize`]
//! does not write the mark. The mark is a request to remove the message, and
//! Camel sends that request through this vfunc.
//!
//! ## The write for each message
//!
//! `expunge_message` in `jmap-mail-sync` decides the write for each message. RFC
//! 8621 §4.6 makes `mailboxIds` a set, so a message can also be in a different
//! mailbox. In that case, a destroy would also remove that copy.
//!
//! ## The work list
//!
//! This module walks the full summary and tests the flag, as the Camel providers
//! do. It does not use `camel_folder_summary_get_changed`: that list holds only
//! rows not yet written to the Camel database. A row marked before the last
//! synchronization is not on it.
//!
//! ## Failures
//!
//! The walk tries every marked row and reports the first failure at the end. A
//! failure for one message tells nothing about the next message.
//!
//! A message that another client destroyed is not a failure. Its row is removed.
//!
//! ## Row removal
//!
//! The rows are removed immediately, not at the next refresh. Until a refresh,
//! the message list would show messages that are not on the server.
//! `camel_folder_changed` updates an open message list.
//!
//! ## Cancellation
//!
//! The call [`observe`]s the `cancellable`. A stop is a failure of the walk. The
//! rows before the stop are removed, and the rows after it stay marked.

use std::ffi::{CStr, CString};

use eds_sys::compat::{summary_dup_uids, summary_free_uids};
use eds_sys::{
    CAMEL_MESSAGE_DELETED, CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name, camel_folder_summary_get,
    camel_folder_summary_remove_uid, camel_message_info_get_flags,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::guard_bool;
use jmap_proto::Id;

use crate::changes::Changes;
use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};

/// Installs the folder's expunge vfunc on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point to an initialized class struct whose first member is a
/// `CamelFolderClass`. All descendants of `CamelFolder` agree with this.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.expunge_sync = Some(expunge_sync);
}

/// Removes the messages in this folder that the user marked deleted.
///
/// Returns `TRUE` if nothing failed. Otherwise it sets the error and returns
/// `FALSE` (the Camel convention).
unsafe extern "C" fn expunge_sync(
    folder: *mut CamelFolder,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the Camel contract for the vfunc: a valid instance of this type,
    // and an out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("expunge_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the full call, so it
            // outlives this observation. The observation stops every request
            // below when the user clicks Stop.
            let _cancel = observe(cancellable);
            match expunge_folder(folder) {
                Ok(()) => GTRUE,
                Err(problem) => {
                    // SAFETY: `to_gerror` gives an owned GError. The contract
                    // of this function makes `error` agree with the contract of
                    // `set_raw_gerror`.
                    set_raw_gerror(error, problem.to_gerror());
                    GFALSE
                }
            }
        })
    }
}

/// Walks the summary and removes the marked messages.
///
/// Two Camel paths call this: the vfunc above ("Expunge" and "Empty Trash"),
/// and the `expunge` argument of `synchronize_sync` in [`crate::synchronize`].
/// Evolution sets that argument when it closes a folder on an account that
/// expunges on exit.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder`.
pub(crate) unsafe fn expunge_folder(folder: *mut CamelFolder) -> Result<(), StoreError> {
    // SAFETY: the contract above.
    let summary = unsafe { camel_folder_get_folder_summary(folder) };
    if summary.is_null() {
        // SAFETY: as above.
        return Err(StoreError::NoFolder(unsafe { name_of(folder) }));
    }
    // Clone the mailbox. A borrow would stay alive across all requests of the
    // walk, and the store below is borrowed from the same instance graph.
    // SAFETY: as above. Camel calls the vfunc on an instance of this class.
    let mailbox = match unsafe { JmapFolder::borrow(folder).and_then(JmapFolder::mailbox) } {
        Some(mailbox) => mailbox.clone(),
        // SAFETY: as above.
        None => return Err(StoreError::NoFolder(unsafe { name_of(folder) })),
    };

    let mut changes = Changes::new();
    let mut failure = None;
    // SAFETY: as above. `deleted_rows` copies the uids out of the Camel array
    // before the walk uses them.
    for uid in unsafe { deleted_rows(summary) } {
        // A uid that is not valid UTF-8 cannot go to the server. Its row stays
        // marked deleted, because this function removes a row only after the
        // server write.
        let Ok(text) = uid.to_str() else { continue };
        // SAFETY: as above.
        let gone = match unsafe { parent_store(folder) } {
            Some(store) => store.expunge_message(&Id::new(text), &mailbox),
            None => Err(StoreError::Disconnected),
        };
        match gone {
            // The message left this mailbox, or the account has no such message.
            // In both cases the folder does not contain it, so remove the row.
            Ok(()) | Err(StoreError::NoMessage(_)) => {
                // SAFETY: a live summary and a uid it holds a row for.
                unsafe { camel_folder_summary_remove_uid(summary, uid.as_ptr()) };
                changes.remove(text);
            }
            Err(problem) => {
                failure.get_or_insert(problem);
            }
        }
    }

    // This updates an open message list. The summary rows supply only the next
    // message list.
    if !changes.is_empty() {
        // SAFETY: a live folder, and a change info this owns for the call.
        unsafe { camel_folder_changed(folder, changes.as_ptr()) };
    }

    match failure {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
}

/// Returns the uids of the rows that the user marked deleted.
///
/// This walks the full summary, not the changed list (the module docs tell
/// why). It copies the uids before the walk, because the walk removes rows from
/// the summary that the array describes.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
unsafe fn deleted_rows(summary: *mut CamelFolderSummary) -> Vec<CString> {
    // SAFETY: the contract above. `summary_dup_uids` returns an array that the
    // caller owns and frees with `summary_free_uids`. The array holds its own
    // reference to each uid in it.
    unsafe {
        let rows = summary_dup_uids(summary);
        if rows.is_null() {
            return Vec::new();
        }
        let uids = (0..(*rows).len)
            .filter_map(|index| {
                let uid: *const gchar = (*rows).pdata.add(index as usize).read().cast();
                if uid.is_null() {
                    return None;
                }
                is_deleted(summary, uid).then(|| CStr::from_ptr(uid).to_owned())
            })
            .collect();
        summary_free_uids(rows);
        uids
    }
}

/// Tells if the row for `uid` has `CAMEL_MESSAGE_DELETED`.
///
/// A concurrent refresh can remove a row after the uid array is copied. Such a
/// row is not deleted, so the walk does not touch it.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary` and `uid` be
/// NUL-terminated.
unsafe fn is_deleted(summary: *mut CamelFolderSummary, uid: *const gchar) -> bool {
    // SAFETY: the contract above. `summary_get` returns a reference that this
    // function owns. The drop of `info` at the end of the scope releases it.
    unsafe {
        let Some(info) = Owned::from_raw(camel_folder_summary_get(summary, uid)) else {
            return false;
        };
        camel_message_info_get_flags(info.as_ptr()) & CAMEL_MESSAGE_DELETED != 0
    }
}

/// Returns the full name of the folder, for an error message.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    // SAFETY: the accessor returns a string that the folder owns and that
    // outlives the call. `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
