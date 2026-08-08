// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderSummary`: where a folder keeps the rows [`crate::message_info`]
//! builds.
//!
//! A `CamelFolder` on its own is a name and a store. The summary is its
//! contents: Camel asks it for the message count shown beside the folder, for
//! the uid list the message list is drawn from, and for the row behind each
//! line of it. That is why a folder without one is a folder Camel never asks
//! anything of — `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` is the flag it tests
//! first, and [`crate::folder`] could not honestly set it until now.
//!
//! ## No subclass
//!
//! Camel's own providers subclass `CamelFolderSummary`, and this one does not.
//! What the subclass exists for is building rows out of *messages*: the three
//! `message_info_new_from_*` vfuncs, which turn a parser, a MIME message or a
//! header list into a row, and `next_uid_string`, which invents a uid for a
//! message that arrived without one. A JMAP folder is listed rather than
//! parsed — the rows come from `Email/get`, already structured, and every one
//! of them arrives with the server's own immutable id — so all four would be
//! overrides of paths this provider does not take. The one thing a base
//! summary decides on our behalf is which message-info class to instantiate,
//! and its answer is `CamelMessageInfoBase`, which is the class
//! [`crate::message_info`] already builds and pins.
//!
//! A subclass becomes real when something local has to be numbered — appending
//! a message to a folder, which is `EmailSubmission` and a later increment.
//!
//! ## A listing is not a copy
//!
//! [`apply_listing`] is called again every time the folder is refreshed, so
//! most of what it does is meet rows that are already there. Three rules, and
//! they are what the tests are about:
//!
//! - A message the listing no longer names has left the mailbox — JMAP moves
//!   mail by changing `mailboxIds`, so leaving and being deleted look the same
//!   from inside one folder — and its row goes.
//! - A message that is already there keeps its row and has only its mutable
//!   columns rewritten. RFC 8621 §4.1 makes everything but `keywords` and
//!   `mailboxIds` immutable, so there is nothing else a refresh could have
//!   learnt, and replacing the row would throw away the local marks Camel
//!   keeps in the same flags word.
//! - A message that is new gets a row under the server's id, not under one the
//!   summary invented.

use std::collections::BTreeSet;

use eds_sys::{
    CamelFolder, CamelFolderSummary, camel_folder_summary_add, camel_folder_summary_check_uid,
    camel_folder_summary_free_array, camel_folder_summary_get, camel_folder_summary_get_array,
    camel_folder_summary_lock, camel_folder_summary_new, camel_folder_summary_remove_uid,
    camel_folder_summary_unlock, camel_folder_take_folder_summary,
};
use glib_sys::{GFALSE, GTRUE, gchar};
use gobject_sys::g_object_unref;
use jmap_mail_sync::MessageSummary;

use crate::changes::Changes;
use crate::folder_info::c_string;
use crate::message_info::{new_message_info, update_message_info};

/// Gives `folder` the summary it keeps its rows in.
///
/// Called once, from [`crate::folder::new_folder`], because a summary is
/// constructed with the folder it belongs to and a folder is never without
/// one: the alternative is a window in which Camel could ask a folder that
/// claims `HAS_SUMMARY_CAPABILITY` for a count it has nowhere to get.
/// `take_folder_summary` takes the reference rather than adding to it, which
/// is what makes the folder the summary's owner and its finalizer the only
/// place the summary is released.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder` that has not been given a
/// summary yet.
pub unsafe fn attach_summary(folder: *mut CamelFolder) {
    // SAFETY: the folder is live by this function's contract, and the summary
    // built from it is handed straight over along with its ownership.
    unsafe {
        let summary = camel_folder_summary_new(folder);
        camel_folder_take_folder_summary(folder, summary);
    }
}

/// Brings the summary in line with what one listing of the mailbox found, and
/// reports what moved.
///
/// The whole reconciliation happens under the summary's lock, so a reader
/// never sees the folder mid-refresh — half a mailbox is a worse answer than
/// the previous one.
///
/// The [`Changes`] that come back are the other half of the answer: rewriting
/// the rows brings a folder that is *opened* up to date, and the diff is the
/// only thing that brings one that is already open up to date. It is returned
/// rather than emitted here because emitting is a fact about a `CamelFolder`
/// and this function is given a summary — the same separation the rest of the
/// module keeps, and what lets every rule below be tested without a signal to
/// listen for.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn apply_listing(
    summary: *mut CamelFolderSummary,
    messages: &[MessageSummary],
) -> Changes {
    let listed: BTreeSet<&str> = messages
        .iter()
        .map(|message| message.uid.as_str())
        .collect();
    let mut changes = Changes::new();

    // SAFETY: the summary is live by this function's contract, and Camel's
    // summary lock is recursive — the calls below take it again themselves.
    unsafe {
        camel_folder_summary_lock(summary);
        remove_absent(summary, &listed, &mut changes);
        for message in messages {
            apply_message(summary, message, &mut changes);
        }
        camel_folder_summary_unlock(summary);
    }

    changes
}

/// Drops the rows for messages the listing did not name.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn remove_absent(
    summary: *mut CamelFolderSummary,
    listed: &BTreeSet<&str>,
    changes: &mut Changes,
) {
    // SAFETY: `get_array` hands back a snapshot the caller owns, holding a
    // reference of its own to every uid in it — so removing a row while
    // walking it neither frees the string being read nor disturbs the walk.
    unsafe {
        let existing = camel_folder_summary_get_array(summary);
        if existing.is_null() {
            return;
        }
        for index in 0..(*existing).len {
            let uid: *const gchar = (*existing).pdata.add(index as usize).read().cast();
            if uid.is_null() {
                continue;
            }
            // A uid Camel stored and we cannot read back as text is not one
            // this listing can have named either, so it goes with the rest.
            let text = std::ffi::CStr::from_ptr(uid).to_str();
            if text.is_ok_and(|uid| listed.contains(uid)) {
                continue;
            }
            camel_folder_summary_remove_uid(summary, uid);
            // Reported under the name Camel keeps it by, when there is one: a
            // removal announced under a name the message list never held is a
            // line that stays on screen.
            if let Ok(text) = text {
                changes.remove(text);
            }
        }
        camel_folder_summary_free_array(existing);
    }
}

/// Adds the row for one message, or updates the one that is already there.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn apply_message(
    summary: *mut CamelFolderSummary,
    message: &MessageSummary,
    changes: &mut Changes,
) {
    let uid = c_string(message.uid.as_str());

    // SAFETY: the uid outlives every call it is passed to; `summary_get`
    // returns a reference this function owns and drops, and `summary_add`
    // takes a reference of its own to the row built here.
    unsafe {
        if camel_folder_summary_check_uid(summary, uid.as_ptr()) != GFALSE {
            let info = camel_folder_summary_get(summary, uid.as_ptr());
            if !info.is_null() {
                // Reported only when the row really moved. A refresh is a poll,
                // so nearly every row it meets is the row it left there, and a
                // folder that announced all of them would redraw the message
                // list the user is reading every time the timer went off.
                if update_message_info(info, message) {
                    changes.change(message.uid.as_str());
                }
                g_object_unref(info.cast());
                return;
            }
            // A uid the summary lists and has no row for is a summary whose
            // database went missing under it. Rebuilding the row from the
            // listing is the recovery; falling through does exactly that.
        }

        let info = new_message_info(message);
        if info.is_null() {
            return;
        }
        // `force_keep_uid`, because the uid is the JMAP `Email` id: unique in
        // the mailbox, immutable, and what every later request names the
        // message by, so a row numbered from the summary's own counter is one
        // nothing can be fetched for. It says that rather than prevents it:
        // Camel renumbers only a uid that is empty or already loaded, and the
        // second cannot get this far past the check above. The value is the
        // one that stays right if it ever does.
        camel_folder_summary_add(summary, info, GTRUE);
        g_object_unref(info.cast());
        changes.add(message.uid.as_str());
    }
}
