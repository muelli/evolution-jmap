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
//! ## One line of subclass
//!
//! `CamelJmapSummary` overrides no vfunc. What a `CamelFolderSummary` subclass
//! usually exists for is building rows out of *messages*: the three
//! `message_info_new_from_*` vfuncs, which turn a parser, a MIME message or a
//! header list into a row, and `next_uid_string`, which invents a uid for a
//! message that arrived without one. A JMAP folder is listed rather than
//! parsed — the rows come from `Email/get`, already structured, and every one
//! of them arrives with the server's own immutable id — so all four would be
//! overrides of paths this provider does not take.
//!
//! It exists for the one thing a summary decides that is not a vfunc at all:
//! `message_info_type`, the class it instantiates a row as. Camel reads that
//! field when it loads the summary back out of the database, so a folder whose
//! summary declared nothing would come back from a restart holding plain
//! `CamelMessageInfoBase` rows — and with them no [`server keywords`], which is
//! the column that makes a flag change a difference rather than a guess. The
//! rows [`crate::message_info`] builds are of that type either way; this is what
//! makes the ones Camel builds match them.
//!
//! [`server keywords`]: crate::message_info::server_keywords
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
use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CamelFolder, CamelFolderSummary, CamelFolderSummaryClass, camel_folder_summary_add,
    camel_folder_summary_check_uid, camel_folder_summary_free_array, camel_folder_summary_get,
    camel_folder_summary_get_array, camel_folder_summary_get_type, camel_folder_summary_lock,
    camel_folder_summary_remove_uid, camel_folder_summary_unlock, camel_folder_take_folder_summary,
};
use glib_sys::{GFALSE, GTRUE, GType, gchar};
use gobject_sys::{g_object_new, g_object_unref};
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::MessageSummary;

use crate::changes::Changes;
use crate::folder_info::c_string;
use crate::message_info::{message_info_type, new_message_info, update_message_info};

/// The instance struct. Nothing of its own: what this type carries is a class
/// field, not per-summary state.
#[repr(C)]
pub struct JmapSummary {
    parent: CamelFolderSummary,
}

/// The class struct, and the field this type exists for.
#[repr(C)]
pub struct JmapSummaryClass {
    parent_class: CamelFolderSummaryClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelFolderSummary
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelFolderSummary derives from GObject.
unsafe impl ObjectSubclass for JmapSummary {
    const NAME: &'static CStr = c"CamelJmapSummary";
    type Instance = JmapSummary;
    type Class = JmapSummaryClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_folder_summary_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // The whole subclass. Camel instantiates a row of this type whenever it
        // builds one itself — which is every row of every folder after a
        // restart, read back out of the summary database.
        //
        // SAFETY: the class leads with CamelFolderSummaryClass — the contract
        // above.
        unsafe { (*class).parent_class.message_info_type = message_info_type() };
    }
}

/// Registers the summary type, or returns it if it is already registered.
pub fn summary_type() -> GType {
    register_static::<JmapSummary>()
}

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
/// Constructed the way `camel_folder_summary_new` constructs a base one — the
/// folder is a construct-only property — rather than through it, because what
/// this folder needs is the subclass above.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder` that has not been given a
/// summary yet.
pub unsafe fn attach_summary(folder: *mut CamelFolder) {
    // SAFETY: a variadic construct call on a registered type; `folder` is a
    // live `CamelFolder` by this function's contract, which is the type the
    // property carries, and the list is NULL-terminated. The summary is handed
    // straight over along with its ownership.
    unsafe {
        let summary = g_object_new(
            summary_type(),
            c"folder".as_ptr(),
            folder,
            ptr::null::<gchar>(),
        )
        .cast::<CamelFolderSummary>();
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
