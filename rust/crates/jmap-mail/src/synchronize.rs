// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `synchronize_sync`: the vfunc that carries the user's changes the other way.
//!
//! Everything the provider has built so far reads. [`crate::refresh`] lists a
//! mailbox into the folder's summary, [`crate::message`] downloads one message's
//! bytes — both are the server telling Evolution something. This is the one
//! place Evolution tells the server something, and what it can tell it, today,
//! is which keywords a message carries: read, important, junk, and whatever the
//! user has labelled it.
//!
//! ## The work list is wider than the work
//!
//! There is no queue of pending writes. What there is is
//! `camel_folder_summary_get_changed`, the list IMAPX drives its own
//! synchronisation from — and it is worth being precise about what that list
//! holds, because its name suggests something narrower. It is the rows Camel has
//! not yet written back to the *summary database*, which is a superset of the
//! rows waiting for the *server*: a row the folder has merely rebuilt is on it
//! too. `CAMEL_MESSAGE_FOLDER_FLAGGED` is the narrower mark, and the one that
//! means a change of the user's is outstanding.
//!
//! So the list is where the walk starts and not what it trusts. Every row on it
//! is diffed, and a row with nothing to say costs nothing at all — not even a
//! connection, which is what makes closing a folder free. The bit is still
//! cleared on every row the walk settles, because a bit left set is a row
//! retried on every synchronisation for as long as the folder is open, and a bit
//! cleared without the write having happened is a change the user made and this
//! provider dropped in silence.
//!
//! It is also why the two places that build rows from a listing —
//! [`crate::message_info::update_message_info`] and [`crate::summary`] — take
//! care not to set it. Camel's column setters and `camel_folder_summary_add`
//! both mark a row as having to reach the server, which is right for the user
//! changing a message and backwards for the server describing one.
//!
//! The bit is read as well as preserved there, and that is what keeps this walk
//! from being raced by a refresh: a listing arriving between the user's click
//! and this vfunc is replayed *around* the outstanding change rather than
//! written over it, so what the walk finds when it gets here is still the
//! difference the user made. Without that, the listing would leave the row
//! claiming exactly what it remembers the server holding — a row still on the
//! work list with nothing left on it to send.
//!
//! ## A difference, and where its two ends come from
//!
//! What goes on the wire is `jmap-mail-sync`'s `KeywordChange` — the difference
//! between the keywords the last listing found and the keywords the row claims
//! now — because a whole-set write would speak for every keyword on the message,
//! including the ones no client here has heard of. The *before* is the column
//! `CamelJmapMessageInfo` keeps for exactly this and nothing else; the *after* is
//! read back out of Camel's own flags word and user flags, which is where the
//! user's click landed. A row nobody really changed therefore produces an empty
//! change, and an empty change costs no request at all — which matters, because
//! Camel marks a row dirty for reasons that are not keywords and Evolution
//! synchronises a folder every time it closes one.
//!
//! ## What one row's failure does to the rest
//!
//! Every dirty row is attempted, and the first failure is what the vfunc reports
//! when the walk is done. Stopping at the first would be cheaper on a dead
//! network and wrong on a live one: a keyword one server refuses says nothing
//! about the next message, and every row behind the refusal would stay queued
//! behind a write that can never succeed. The cost is that a connection that has
//! just gone away is discovered once per dirty row.
//!
//! A message another client destroyed is not a failure at all — see
//! [`push_row`].
//!
//! ## What is not here yet
//!
//! **`expunge`.** Camel's argument asks the folder to also get rid of the
//! messages the user marked deleted. JMAP has no deleted keyword — deleting mail
//! is `Email/set` taking the message out of its mailboxes, or `Email/set`
//! destroying it, depending on what the account calls its trash — so it is a
//! mailbox change rather than a flag change and belongs with the increment that
//! implements `expunge_sync`. Until then the argument is ignored: a row marked
//! deleted produces no keyword change, leaves the server as it was, and keeps
//! its `DELETED` bit, which is what that later increment will read.
//!
//! **`cancellable`**, the same gap [`crate::refresh`] documents and for the same
//! reason: [`Client`] takes its [`CancelFlag`] when it is built.
//!
//! [`Client`]: jmap_client::Client
//! [`CancelFlag`]: jmap_client::transport::CancelFlag

use std::ffi::{CStr, CString};

use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_get_folder_summary,
    camel_folder_get_full_name, camel_folder_summary_free_array, camel_folder_summary_get,
    camel_folder_summary_get_changed, camel_message_info_set_folder_flagged,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::KeywordChange;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::parent_store;
use crate::message_info::{row_keywords, server_keywords, set_server_keywords};

/// Installs the folder's write vfunc on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.synchronize_sync = Some(synchronize_sync);
}

/// Writes every queued change of this folder and empties the queue of the ones
/// that landed.
///
/// `TRUE` for a synchronisation in which nothing failed, `FALSE` with the error
/// set otherwise — Camel's convention, and what the callers of
/// `camel_folder_synchronize_sync` test before they consider the folder saved.
unsafe extern "C" fn synchronize_sync(
    folder: *mut CamelFolder,
    _expunge: gboolean,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("synchronize_sync", error, || {
            let summary = camel_folder_get_folder_summary(folder);
            if summary.is_null() {
                return fail(error, &StoreError::NoFolder(name_of(folder)));
            }

            let mut failure = None;
            for uid in queued_rows(summary) {
                if let Err(problem) = push_row(folder, summary, &uid) {
                    failure.get_or_insert(problem);
                }
            }

            match failure {
                Some(problem) => fail(error, &problem),
                None => GTRUE,
            }
        })
    }
}

/// The uids of the rows Camel is holding as not yet written back.
///
/// Every row waiting for the server is on this list, and so are rows waiting
/// only for the summary database — see this module's header. The walk sorts them
/// out by diffing, which is what it would have to do anyway.
///
/// Copied out of Camel's array and the array freed before anything is sent, for
/// two reasons that point the same way: the walk below makes a request per row,
/// and the summary it is walking is one the folder may be adding rows to
/// meanwhile. A row that arrives during the walk is not this synchronisation's
/// business — it cannot be a change the user has made and left unsent — and a
/// row that disappears during it is handled where it is met.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
unsafe fn queued_rows(summary: *mut CamelFolderSummary) -> Vec<CString> {
    // SAFETY: the contract above; `get_changed` hands back an array the caller
    // owns and frees with `free_array`, holding a reference of its own to every
    // uid in it.
    unsafe {
        let queued = camel_folder_summary_get_changed(summary);
        if queued.is_null() {
            return Vec::new();
        }
        let uids = (0..(*queued).len)
            .filter_map(|index| {
                let uid: *const gchar = (*queued).pdata.add(index as usize).read().cast();
                (!uid.is_null()).then(|| CStr::from_ptr(uid).to_owned())
            })
            .collect();
        camel_folder_summary_free_array(queued);
        uids
    }
}

/// Sends one row's change, and takes the row off the work list if it is settled.
///
/// Settled means three things, and the third is the one worth naming:
///
/// - The write succeeded. The row's remembered set becomes what was just sent,
///   or the *next* change would be diffed against a set the server stopped
///   holding a synchronisation ago.
/// - There was nothing to write. Most rows on the work list are this: one Camel
///   has not saved to its database yet, or one dirty for a change that is not a
///   keyword — `CAMEL_MESSAGE_DELETED` is a local mark JMAP has no keyword for.
///   Such a row costs no request and, deliberately, not even a connection: the
///   store is looked for only once there is something to say to it, which is
///   what makes closing a folder full of unchanged mail free.
/// - The message is gone. Another client destroying it is ordinary rather than
///   a fault: a uid in a summary is a claim about the last listing, and the flag
///   change is moot rather than failed. Reported as a failure it would put an
///   alert in front of the user about a message that is not there; left queued,
///   it would retry a write that can never succeed. The row itself goes at the
///   next refresh, which is where a message leaving the mailbox is noticed.
///
/// A row whose remembered set is absent — one loaded from a summary written
/// before that column existed — is diffed from nothing, which is the same
/// conservative degradation `CamelJmapMessageInfo` documents: a difference from
/// the empty set adds keywords and removes none.
///
/// The summary's own lock is deliberately not held across the request. A
/// synchronisation is one round trip per changed row, and a folder locked for
/// the length of that is a message list that cannot be drawn while the user's
/// last click is being saved; `camel_folder_summary_get` takes the lock itself
/// for as long as it needs it. What a refresh running alongside can do is renew
/// the row's remembered set from a fresh listing, which this then overwrites
/// with what the write just established — the more recent of the two answers.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
unsafe fn push_row(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
) -> Result<(), StoreError> {
    // SAFETY: the contract above, and `summary_get` hands back a reference this
    // function owns and releases below.
    let info = unsafe { camel_folder_summary_get(summary, uid.as_ptr()) };
    if info.is_null() {
        // A uid the summary named a moment ago and has no row for now: the row
        // was removed by a refresh running alongside. There is nothing to send
        // and nothing left to clear.
        return Ok(());
    }

    // SAFETY: `info` is a live row for as long as this function's reference is.
    let result = unsafe {
        let before = server_keywords(info).unwrap_or_default();
        let after = row_keywords(info);
        let change = KeywordChange::between(&before, &after);

        // A uid Camel stored and we cannot read back as text is not one the
        // server can be asked about either; it is left queued rather than
        // reported, because the row is not one this provider put there.
        let result = match (change.is_empty(), uid.to_str()) {
            (true, _) => Ok(()),
            (false, Ok(text)) => match parent_store(folder) {
                Some(store) => store.set_keywords(&Id::new(text), &change),
                None => Err(StoreError::Disconnected),
            },
            (false, Err(_)) => Ok(()),
        };
        match &result {
            Ok(()) => {
                set_server_keywords(info, after);
                camel_message_info_set_folder_flagged(info, GFALSE);
            }
            Err(StoreError::NoMessage(_)) => {
                camel_message_info_set_folder_flagged(info, GFALSE);
            }
            Err(_) => {}
        }
        result
    };

    // SAFETY: the reference taken above.
    unsafe { g_object_unref(info.cast()) };
    match result {
        Err(StoreError::NoMessage(_)) => Ok(()),
        other => other,
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

/// Reports a failure and answers with it.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail(error: *mut *mut GError, failure: &StoreError) -> gboolean {
    // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe { set_raw_gerror(error, failure.to_gerror()) };
    GFALSE
}
