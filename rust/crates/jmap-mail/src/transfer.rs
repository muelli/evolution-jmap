// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_messages_to_sync`: the vfunc behind dragging a message into
//! another folder.
//!
//! [`crate::synchronize`] writes what the user changed *about* a message; this
//! writes where it is. Both are one `Email/set` update, and the patch itself
//! was built an increment ago as `jmap-mail-sync`'s [`Filing`] — a copy adds a
//! `mailboxIds` member, a move adds one and takes another away, in one change.
//! What is decided here is everything around that: which two mailboxes, what
//! becomes of the rows the source folder is left holding, and which of Camel's
//! several answers the caller gets.
//!
//! ## One request per message, not one per transfer
//!
//! One `Email/set` can carry an update for every selected message at once, and
//! it would be applied as a single state change — so a transfer that half
//! succeeded would come back as one failure with no way to say which messages
//! moved. Camel asks about a list and needs an answer per message: a row that
//! landed must leave the folder and a row that did not must stay in it. So the
//! walk is a request per uid, exactly like [`crate::synchronize`]'s, and the
//! first failure is what the vfunc reports once every message has been tried.
//! Stopping at the first would leave the rest of the user's selection untouched
//! for a reason that may say nothing about them.
//!
//! ## What a move does to the folder it left
//!
//! The rows go, now, rather than at the next listing. A refresh would reach the
//! same answer — JMAP moves mail by changing `mailboxIds`, so a message that
//! left the mailbox is one the next `Email/query` does not name — but "the next
//! refresh" is a timer, and until it fires the message list would still be
//! showing the message the user just moved out of it.
//!
//! Removing a row also removes the only record of a change the user has made
//! and not yet saved: Camel keeps that on the row, marked with
//! `CAMEL_MESSAGE_FOLDER_FLAGGED`, until `synchronize_sync` writes it. So a
//! move settles the row before it takes it away, through the same function the
//! synchronisation walk uses. It costs nothing for a row nobody changed — the
//! diff is empty and no request is made — and it is the difference between a
//! flag surviving a move and this provider dropping it in silence.
//!
//! ## What is not decided here
//!
//! **The destination folder's rows.** Nothing is added to them, and the message
//! appears there when that folder is next listed. The listing is the only thing
//! that knows what a row in it should say: what this side holds is a uid, and a
//! row built from a uid alone would be a message list line with no subject,
//! sender or date until the refresh replaced it.
//!
//! ## Stopping one
//!
//! The `cancellable` is [`observe`]d for the length of the call. A stop between
//! two messages leaves the ones already filed filed — a move is one `Email/set`
//! per message and there is no transaction over the set — and the rows of those
//! messages are removed from this folder, so what the user sees matches what the
//! server holds.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr;

use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name, camel_folder_summary_remove_uid,
};
use gio_sys::GCancellable;
use glib_sys::{
    GError, GFALSE, GPtrArray, GTRUE, g_ptr_array_new, g_ptr_array_set_size, g_strdup, gboolean,
    gchar,
};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::fail_bool;
use jmap_backend_core::marshal::{checked_borrow, read_string};
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::Filing;
use jmap_proto::Id;

use crate::changes::Changes;
use crate::connect::StoreError;
use crate::folder::{JmapFolder, folder_type, parent_store};
use crate::synchronize::push_row;

/// Installs the folder's filing vfunc on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.transfer_messages_to_sync = Some(transfer_messages_to_sync);
}

/// Files every named message into `destination`, and out of `source` if the
/// originals are to go.
///
/// `TRUE` for a transfer in which nothing failed, `FALSE` with the error set
/// otherwise — Camel's convention, and what `camel_folder_transfer_messages_to`
/// reports to the user.
///
/// The vfunc is reached only for a transfer between two folders of one store:
/// `camel_folder_transfer_messages_to_sync` answers a transfer into the folder
/// the messages are already in and one of no messages itself, and sends
/// anything crossing stores down its own path of `get_message` and
/// `append_message`. That path ends at [`crate::append`], which uploads the
/// message and files it with `Email/import` (RFC 8621 §4.8) — a different
/// request entirely, because a message from another account is one this one has
/// never seen.
unsafe extern "C" fn transfer_messages_to_sync(
    source: *mut CamelFolder,
    message_uids: *mut GPtrArray,
    destination: *mut CamelFolder,
    delete_originals: gboolean,
    transferred_uids: *mut *mut GPtrArray,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: two valid folders, an array of
    // uids of the first, and out-parameters that are NULL or writable.
    unsafe {
        guard_bool("transfer_messages_to_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            let uids = uid_list(message_uids);
            let reported = Reported::new(transferred_uids, uids.len());

            // Type-checked rather than assumed, unlike `source`: GObject
            // dispatched this call on the source's class, while the
            // destination is whatever the caller passed. Camel picks the
            // *destination's* class when it is a vtrash folder, so a folder of
            // someone else's arriving here is a case the wrapper allows for.
            let Some(into) = mailbox_of(destination) else {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(destination)),
                    StoreError::to_gerror,
                );
            };
            let Some(out_of) = JmapFolder::borrow(source).and_then(JmapFolder::mailbox) else {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(source)),
                    StoreError::to_gerror,
                );
            };
            let summary = camel_folder_get_folder_summary(source);
            if summary.is_null() {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(source)),
                    StoreError::to_gerror,
                );
            }

            let moving = delete_originals != GFALSE;
            let filing = match moving {
                true => Filing::moved(out_of.clone(), into.clone()),
                false => Filing::copied_into(into.clone()),
            };
            // A move into the mailbox the messages are already in: the one
            // filing that cannot be written down, and therefore one that must
            // not take the rows away either. Camel's own wrapper settles the
            // usual way of asking for it — the same `CamelFolder` twice — but
            // two folders of one mailbox are a pair it cannot recognise.
            if filing.is_empty() {
                return GTRUE;
            }

            let mut changes = Changes::new();
            let mut failure = None;
            for (index, uid) in uids.iter().enumerate() {
                let filed = file_message(source, summary, uid, &filing, moving);
                // A message that is not on the server at all is not in this
                // folder either, whichever way the transfer was going.
                let leaves = match &filed {
                    Ok(true) => moving,
                    Ok(false) => false,
                    Err(StoreError::NoMessage(_)) => true,
                    Err(_) => false,
                };
                if leaves {
                    camel_folder_summary_remove_uid(summary, uid.as_ptr());
                    if let Ok(text) = uid.to_str() {
                        changes.remove(text);
                    }
                }
                match filed {
                    Ok(true) => reported.set(index, uid),
                    Ok(false) => {}
                    Err(problem) => {
                        failure.get_or_insert(problem);
                    }
                }
            }

            // What tells a message list that is already on screen; the rows
            // above are only what the next one would be drawn from.
            if !changes.is_empty() {
                camel_folder_changed(source, changes.as_ptr());
            }

            match failure {
                Some(problem) => fail_bool(error, &problem, StoreError::to_gerror),
                None => GTRUE,
            }
        })
    }
}

/// Files one message, reporting whether it landed.
///
/// `Ok(false)` is the one outcome that is neither: a uid Camel stored and this
/// cannot read back as text is not one the server can be asked about either,
/// and it is not a row this provider put there. It is left exactly as it is —
/// not reported as transferred, not taken out of the folder, not counted as a
/// failure of the account.
///
/// A move settles the row first, for the reason this module's header gives: the
/// row is about to go, and it is the only place a change the user has not saved
/// yet is written down. A row nobody changed makes no request there.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder` and `summary` at its summary.
unsafe fn file_message(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
    filing: &Filing,
    moving: bool,
) -> Result<bool, StoreError> {
    let Ok(text) = uid.to_str() else {
        return Ok(false);
    };
    // SAFETY: the contract above, and `push_row` takes the same pair.
    unsafe {
        if moving {
            push_row(folder, summary, uid)?;
        }
        let store = parent_store(folder).ok_or(StoreError::Disconnected)?;
        store.file_message(&Id::new(text), filing)?;
    }
    Ok(true)
}

/// The mailbox `folder` is a view of, if it is a folder of this provider at
/// all.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn mailbox_of<'a>(folder: *mut CamelFolder) -> Option<&'a Id> {
    // SAFETY: the contract above; the type check is what makes the borrow
    // below sound, exactly as it is for the store behind a folder.
    unsafe { checked_borrow::<_, JmapFolder>(folder, folder_type())?.mailbox() }
}

/// The uids Camel named, copied out of its array.
///
/// Copied rather than borrowed because the walk that follows removes rows from
/// the summary and makes a request per uid, while the array belongs to the
/// caller — and, in Camel's own vee-folder path, is rebuilt between calls.
///
/// # Safety
///
/// `array` must be NULL or a live `GPtrArray` of NUL-terminated strings.
unsafe fn uid_list(array: *mut GPtrArray) -> Vec<CString> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above; every string lives as long as the array,
    // which outlives this call, and each is copied here.
    unsafe {
        (0..(*array).len)
            .filter_map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                (!uid.is_null()).then(|| CStr::from_ptr(uid).to_owned())
            })
            .collect()
    }
}

/// Where the transferred messages ended up, for the caller that asked.
///
/// The answer JMAP gives is the question: RFC 8621 §4.1 gives an `Email` one
/// immutable id per account, and filing it into another mailbox does not make a
/// second object — so the message in the destination is the message that was in
/// the source, under the uid the caller passed in. A protocol whose server mints
/// a new uid in the destination has something to look up here; this one has
/// something to copy.
///
/// Allocated and filled the way Camel's own generic transfer does it, because
/// that is the shape its callers free: an array sized to the uid list up front,
/// `NULL` in the slot of any message that did not land, and every string one
/// `g_free` will release.
struct Reported(*mut GPtrArray);

impl Reported {
    /// Hands the caller an empty array of the right size, or nothing if it did
    /// not ask.
    ///
    /// # Safety
    ///
    /// `out` must be NULL or writable.
    unsafe fn new(out: *mut *mut GPtrArray, len: usize) -> Self {
        if out.is_null() {
            return Self(ptr::null_mut());
        }
        // SAFETY: a fresh array, sized so that every slot exists before any is
        // written; `set_size` NULLs the new ones, which is what a message that
        // did not land leaves behind.
        unsafe {
            let array = g_ptr_array_new();
            g_ptr_array_set_size(array, len as c_int);
            *out = array;
            Self(array)
        }
    }

    /// Records where the message at `index` of the caller's list ended up.
    ///
    /// # Safety
    ///
    /// `index` must be below the length this was built with, and `uid`
    /// NUL-terminated.
    unsafe fn set(&self, index: usize, uid: &CStr) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: the contract above; the copy is the caller's to free.
        unsafe { *(*self.0).pdata.add(index) = g_strdup(uid.as_ptr()).cast() };
    }
}

/// The path Camel keys the folder by, for an error message about it.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    if folder.is_null() {
        return String::new();
    }
    // SAFETY: the accessor returns a string the folder owns and outlives the
    // call; `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
