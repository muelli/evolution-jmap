// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `expunge_sync`: the vfunc that carries out the deletions the user has made.
//!
//! [`crate::synchronize`] writes what the user changed *about* a message and
//! [`crate::transfer`] writes where it is filed. This writes the one thing
//! neither of them could: that the user is finished with it.
//!
//! ## Why the mark waited this long
//!
//! `CAMEL_MESSAGE_DELETED` is a bit of Camel's own — [`crate::message_info`]'s
//! `FLAGS_FROM_JMAP` has said since it was written that JMAP has no deleted
//! keyword — so pressing Delete in Evolution marks a summary row and reaches no
//! server at all. [`crate::synchronize`] therefore produced no keyword change
//! for such a row, deliberately and with a note saying so: the mark is not a
//! property of the message, it is a request that the message stop being here,
//! and the request Camel makes of it is this vfunc rather than that one.
//!
//! ## Which write an expunge is
//!
//! Decided in `jmap-mail-sync`'s `expunge_message`, and it is decided per
//! message rather than once: RFC 8621 §4.6 makes `mailboxIds` a set, so a
//! message may be in the folder being expunged *and* in one the user filed it
//! into, and destroying it would take their copy with it. What is decided here
//! is everything around that — which rows are on the work list, what becomes of
//! a row whose message is already gone, and what the folder announces.
//!
//! ## The work list is the flag, and nothing else
//!
//! Not `camel_folder_summary_get_changed`, which is what [`crate::synchronize`]
//! walks. That list is the rows Camel has not written back to its *database*,
//! and a row marked deleted before the last synchronisation is not on it while
//! still being exactly what an expunge is for. So this walks the whole summary
//! and tests the bit, which is what Camel's own providers do.
//!
//! ## What one row's failure does to the rest
//!
//! Every marked row is attempted and the first failure is what the vfunc
//! reports when the walk is done — [`crate::synchronize`]'s judgement, for its
//! reason: a message one server refuses to destroy says nothing about the next,
//! and every row behind the refusal would stay marked behind a write that can
//! never succeed.
//!
//! A message another client destroyed is not a failure at all. It is a message
//! that is already gone, which is what the expunge wanted; the row goes with
//! it.
//!
//! ## The rows go now, not at the next listing
//!
//! [`crate::transfer`]'s judgement about a move, for the same reason: a refresh
//! would reach the same answer, but "the next refresh" is a timer, and until it
//! fires the message list would still be offering a message that is not there.
//! `camel_folder_changed` is what redraws a window that is already open; the
//! rows themselves are only what the next listing is drawn from.
//!
//! ## What is not here
//!
//! **`cancellable`**, the same gap [`crate::refresh`], [`crate::synchronize`]
//! and [`crate::transfer`] document, for the same reason: [`Client`] takes its
//! [`CancelFlag`] when it is built.
//!
//! [`Client`]: jmap_client::Client
//! [`CancelFlag`]: jmap_client::transport::CancelFlag

use std::ffi::{CStr, CString};

use eds_sys::{
    CAMEL_MESSAGE_DELETED, CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name, camel_folder_summary_free_array,
    camel_folder_summary_get, camel_folder_summary_get_array, camel_folder_summary_remove_uid,
    camel_message_info_get_flags,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
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
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.expunge_sync = Some(expunge_sync);
}

/// Gets rid of the messages this folder's user marked deleted.
///
/// `TRUE` for an expunge in which nothing failed, `FALSE` with the error set
/// otherwise — Camel's convention, and what `camel_folder_expunge_sync` reports
/// to the user.
unsafe extern "C" fn expunge_sync(
    folder: *mut CamelFolder,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("expunge_sync", error, || match expunge_folder(folder) {
            Ok(()) => GTRUE,
            Err(problem) => {
                // SAFETY: `to_gerror` hands over an owned GError, and `error`
                // meets `set_raw_gerror`'s contract by this function's.
                set_raw_gerror(error, problem.to_gerror());
                GFALSE
            }
        })
    }
}

/// The walk itself, as a function [`crate::synchronize`] can also reach.
///
/// Camel offers two ways to ask for this and Evolution uses both: the vfunc
/// above, behind "Expunge" and "Empty Trash", and `synchronize_sync`'s
/// `expunge` argument, which is what a folder closing on an account configured
/// to expunge on exit sends. They mean the same thing and share this.
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
    // Cloned rather than borrowed: the borrow would be alive across every
    // request the walk makes, and the store below is borrowed from the same
    // instance graph.
    // SAFETY: as above; the vfunc is dispatched on an instance of our class.
    let mailbox = match unsafe { JmapFolder::borrow(folder).and_then(JmapFolder::mailbox) } {
        Some(mailbox) => mailbox.clone(),
        // SAFETY: as above.
        None => return Err(StoreError::NoFolder(unsafe { name_of(folder) })),
    };

    let mut changes = Changes::new();
    let mut failure = None;
    // SAFETY: as above, and `deleted_rows` copies what it names out of Camel's
    // arrays before any of it is spent.
    for uid in unsafe { deleted_rows(summary) } {
        // A uid Camel stored and we cannot read back as text is not one the
        // server can be asked about either; it is left exactly as it is, which
        // is a row still marked deleted rather than a row this provider took
        // away without having destroyed anything.
        let Ok(text) = uid.to_str() else { continue };
        // SAFETY: as above.
        let gone = match unsafe { parent_store(folder) } {
            Some(store) => store.expunge_message(&Id::new(text), &mailbox),
            None => Err(StoreError::Disconnected),
        };
        match gone {
            // The message left this mailbox, or was not in the account at all.
            // Either way it is not in this folder, and the row goes.
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

    // What tells a message list that is already on screen; the rows above are
    // only what the next one would be drawn from.
    if !changes.is_empty() {
        // SAFETY: a live folder, and a change info this owns for the call.
        unsafe { camel_folder_changed(folder, changes.as_ptr()) };
    }

    match failure {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
}

/// The uids of the rows the user marked deleted.
///
/// The whole summary is walked rather than the changed list, for the reason
/// this module's header gives, and the uids are copied out before anything is
/// sent: the walk makes a request per row and then removes rows from the very
/// summary the array describes.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
unsafe fn deleted_rows(summary: *mut CamelFolderSummary) -> Vec<CString> {
    // SAFETY: the contract above; `get_array` hands back an array the caller
    // owns and frees with `free_array`, holding a reference of its own to every
    // uid in it.
    unsafe {
        let rows = camel_folder_summary_get_array(summary);
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
        camel_folder_summary_free_array(rows);
        uids
    }
}

/// Whether the row for `uid` carries `CAMEL_MESSAGE_DELETED`.
///
/// A row the summary has no longer — one a refresh running alongside removed
/// between the array being taken and this being asked — is not deleted, which
/// is the reading that leaves it alone.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary` and `uid` be
/// NUL-terminated.
unsafe fn is_deleted(summary: *mut CamelFolderSummary, uid: *const gchar) -> bool {
    // SAFETY: the contract above, and `summary_get` hands back a reference this
    // function owns and releases below.
    unsafe {
        let info = camel_folder_summary_get(summary, uid);
        if info.is_null() {
            return false;
        }
        let deleted = camel_message_info_get_flags(info) & CAMEL_MESSAGE_DELETED != 0;
        g_object_unref(info.cast());
        deleted
    }
}

/// The path Camel keys the folder by, for an error message about it.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    // SAFETY: the accessor returns a string the folder owns and outlives the
    // call; `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
