// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `synchronize_sync`: the vfunc that writes the keyword changes of the user
//! to the server.
//!
//! [`crate::refresh`] and [`crate::message`] read from the server. This module
//! writes which keywords a message has: read, important, junk and the labels of
//! the user. Other modules write other changes (for example [`crate::transfer`]
//! and [`crate::expunge`]).
//!
//! ## The work list is wider than the work
//!
//! There is no queue of pending writes. The work list is
//! `camel_folder_summary_get_changed`, which IMAPX also uses for its
//! synchronisation. The list holds the rows that Camel has not written to the
//! *summary database*. That is a superset of the rows that wait for the
//! *server*: a row that the folder only rebuilt is also on it.
//! `CAMEL_MESSAGE_FOLDER_FLAGGED` is the narrower mark: it shows that a change
//! of the user is outstanding.
//!
//! Thus the walk starts from the list but does not trust it. It diffs every row.
//! A row with no difference costs nothing, not even a connection, so closing a
//! folder is free. The walk clears the mark on every row that it settles. A
//! mark left set makes the row retry on every synchronisation while the folder
//! is open. A mark cleared without a write loses a change of the user silently.
//!
//! For the same reason, the two places that build rows from a listing
//! ([`crate::message_info::update_message_info`] and [`crate::summary`]) do not
//! set the mark. Camel's column setters and `camel_folder_summary_add` both mark
//! a row for the server. That is correct for a change of the user and wrong for
//! a description from the server.
//!
//! These two places also read and keep the mark, so a refresh cannot race this
//! walk. A listing that arrives between the click and this vfunc is applied
//! *around* the outstanding change, not over it. Thus the walk still finds the
//! difference that the user made. Without this, the listing would make the row
//! equal to the remembered server set: a row on the work list with nothing to
//! send.
//!
//! ## A difference, and the source of its two ends
//!
//! The request sends a `KeywordChange` of `jmap-mail-sync`: the difference
//! between the keywords of the last listing and the keywords the row has now. A
//! write of the full set would speak for every keyword, also the keywords that
//! no client here knows. The *before* set is a column of `CamelJmapMessageInfo`
//! that has only this purpose. The *after* set comes from Camel's flags word and
//! user flags, where the click of the user lands. A row with no real change thus
//! gives an empty change, and an empty change sends no request. This is
//! important: Camel marks rows dirty for reasons that are not keywords, and
//! Evolution synchronises each folder when it closes it.
//!
//! ## The effect of one row's failure on the other rows
//!
//! The walk tries every dirty row and, at the end, reports the first failure.
//! A stop at the first failure is cheaper on a dead network but wrong on a live
//! one. A keyword that the server refuses for one message says nothing about the
//! next message. Also, every row after the refusal would wait for a write that
//! can never succeed. The cost: the walk finds a lost connection once for each
//! dirty row.
//!
//! A message that another client destroyed is not a failure (see `push_row`).
//!
//! ## Deleted rows
//!
//! JMAP has no deleted keyword, so a deleted row gives no keyword change here.
//! [`crate::expunge`] does all of the removal, and this vfunc calls it after the
//! walk below. Without `expunge`, a deleted row outside the trash moves into the
//! trash, as IMAPX does with a real trash. With `expunge`, Camel also asks the
//! folder to remove the deleted rows.
//!
//! ## Stopping a synchronisation
//!
//! The vfunc [`observe`]s the `cancellable` for the full call, so a stop occurs
//! between two writes, not inside one. Rows already written stay written and
//! lose their mark. This is the correct record: the server has those changes,
//! and the rows after the stop stay on the work list for the next
//! synchronisation.

use std::ffi::{CStr, CString};

use eds_sys::compat::{summary_dup_changed, summary_free_uids};
use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_get_folder_summary,
    camel_folder_get_full_name, camel_folder_summary_get, camel_message_info_set_folder_flagged,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::fail_bool;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::KeywordChange;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::expunge::Walk;
use crate::folder::parent_store;
use crate::message_info::{row_keywords, server_keywords, set_server_keywords};

/// Installs the write vfunc of the folder on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that starts with a
/// `CamelFolderClass` (every descendant of `CamelFolder` does).
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.synchronize_sync = Some(synchronize_sync);
}

/// Writes every queued change of this folder and removes the written changes
/// from the work list.
///
/// Returns `TRUE` if nothing failed, otherwise `FALSE` with the error set. This
/// is Camel's convention: callers of `camel_folder_synchronize_sync` test it to
/// know that the folder is saved.
unsafe extern "C" fn synchronize_sync(
    folder: *mut CamelFolder,
    expunge: gboolean,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("synchronize_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the full call, so
            // it outlives this observation. Thus every request below stops when
            // the user presses Stop.
            let _cancel = observe(cancellable);

            let summary = camel_folder_get_folder_summary(folder);
            if summary.is_null() {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(folder)),
                    StoreError::to_gerror,
                );
            }

            let mut failure = None;
            for uid in queued_rows(summary) {
                if let Err(problem) = push_row(folder, summary, &uid) {
                    failure.get_or_insert(problem);
                }
            }

            // Remove after the keyword walk, for two reasons. A row to be
            // removed can have an unsaved change (read, then deleted, is
            // usual). Also, the walk clears the marks that would retry.
            let walk = if expunge != GFALSE {
                Walk::Synchronize
            } else {
                Walk::FileInTrash
            };
            if let Err(problem) = crate::expunge::expunge_folder(folder, walk) {
                failure.get_or_insert(problem);
            }

            match failure {
                Some(problem) => fail_bool(error, &problem, StoreError::to_gerror),
                None => GTRUE,
            }
        })
    }
}

/// The uids of the rows that Camel has not written back (the work list).
///
/// The list holds every row that waits for the server, and also rows that wait
/// only for the summary database (see the module header). The diff separates them.
///
/// This copies the uids and frees Camel's array before it sends a request. The
/// walk makes one request per row, and the folder can add rows meanwhile.
/// A row that arrives during the walk cannot be an unsent change of the user, so
/// the walk ignores it. `push_row` handles a row that disappears during the walk.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
unsafe fn queued_rows(summary: *mut CamelFolderSummary) -> Vec<CString> {
    // SAFETY: the contract above. `summary_dup_changed` returns an array that the
    // caller owns and frees with `summary_free_uids`. The array holds its own
    // reference to each uid in it.
    unsafe {
        let queued = summary_dup_changed(summary);
        if queued.is_null() {
            return Vec::new();
        }
        let uids = (0..(*queued).len)
            .filter_map(|index| {
                let uid: *const gchar = (*queued).pdata.add(index as usize).read().cast();
                (!uid.is_null()).then(|| CStr::from_ptr(uid).to_owned())
            })
            .collect();
        summary_free_uids(queued);
        uids
    }
}

/// Sends the change of one row, and removes the row from the work list if the
/// row is settled.
///
/// A row is settled in three cases. The third case is the important one:
///
/// - The write succeeded. The remembered set of the row becomes the set just
///   written. If not, the *next* change would diff against an old server set.
/// - There was nothing to write. Most rows on the work list are this case: rows
///   that Camel has not saved to its database, or rows dirty for a change that is
///   not a keyword (JMAP has no keyword for the local mark `CAMEL_MESSAGE_DELETED`).
///   Such a row sends no request and opens no connection: the function gets the
///   store only when there is something to send. Thus closing a folder of
///   unchanged mail is free.
/// - The message is gone. Another client that destroys a message is usual, not a
///   fault. A uid in a summary is a claim about the last listing, so the flag
///   change is moot, not failed. As a failure, it would show the user an alert
///   about a message that does not exist. Left on the work list, it would retry a
///   write that can never succeed. The next refresh removes the row, because a
///   refresh finds messages that left the mailbox.
///
/// [`crate::transfer`] also calls this function before it removes a row. A move
/// removes the row, and the row is the only record of a change the user has not saved.
///
/// A row with no remembered set (loaded from a summary older than that column)
/// diffs against the empty set. `CamelJmapMessageInfo` documents this safe
/// degradation: a diff from the empty set adds keywords and removes none.
///
/// This function does not hold the lock of the summary during the request. A
/// synchronisation is one round trip per changed row. A lock for that time
/// prevents the message list from drawing while the last click saves.
/// `camel_folder_summary_get` takes the lock itself, only for as long as it needs
/// it. A parallel refresh can renew the remembered set of the row from a new
/// listing. This function then overwrites it with the result of the write, the
/// more recent of the two answers.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub(crate) unsafe fn push_row(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
) -> Result<(), StoreError> {
    // SAFETY: the contract above. `summary_get` returns a reference that this
    // function owns and releases when `info` drops at the end of the scope.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        // A parallel refresh removed the row after the work list named it.
        // There is nothing to send and nothing to clear.
        return Ok(());
    };

    // SAFETY: `info` is a live row while this function holds its reference.
    let result = unsafe {
        let before = server_keywords(info.as_ptr()).unwrap_or_default();
        let after = row_keywords(info.as_ptr());
        let change = KeywordChange::between(&before, &after);

        let result = if change.is_empty() {
            Ok(())
        } else {
            let Ok(text) = uid.to_str() else {
                // A uid that is not valid text cannot go to the server. Keep
                // the row on the work list: do not settle a row with no write.
                return Ok(());
            };
            match parent_store(folder) {
                Some(store) => store.set_keywords(&Id::new(text), &change),
                None => Err(StoreError::Disconnected),
            }
        };
        match &result {
            Ok(()) => {
                set_server_keywords(info.as_ptr(), after);
                camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE);
            }
            Err(StoreError::NoMessage(_)) => {
                camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE);
            }
            Err(_) => {}
        }
        result
    };

    match result {
        Err(StoreError::NoMessage(_)) => Ok(()),
        other => other,
    }
}

/// The path that Camel uses as the key of the folder, for an error message.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    // SAFETY: the accessor returns a string that the folder owns and that
    // outlives the call. `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
