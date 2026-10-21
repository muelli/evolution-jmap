// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `transfer_messages_to_sync`: the vfunc that moves or copies a message into
//! a different folder (for example, when the user drags it).
//!
//! [`crate::synchronize`] writes changes *about* a message. This module writes
//! the location of the message. Both send one `Email/set` update.
//! `jmap-mail-sync`'s [`Filing`] makes the patch: a copy adds a `mailboxIds`
//! member, a move adds one and removes one, in one change. This module decides
//! the two mailboxes, the result for the rows of the source folder, and which of
//! Camel's answers the caller gets.
//!
//! ## One request per message, not one per transfer
//!
//! One `Email/set` can update all selected messages, but the server applies it
//! as one state change. Thus a transfer that half succeeds would give one
//! failure with no list of the moved messages. Camel asks about a list and needs
//! one answer per message: a row that landed must leave the folder, and a row
//! that did not land must stay. Thus the walk sends one request per uid, as
//! [`crate::synchronize`] does. When it has tried every message, the vfunc
//! reports the first failure. A stop at the first failure would leave the
//! remaining selection unchanged, for a reason that possibly does not apply to it.
//!
//! ## The effect of a move on the source folder
//!
//! The rows go immediately, not at the next listing. A refresh gives the same
//! result: JMAP moves mail through `mailboxIds`, so the next `Email/query` does
//! not name a message that left the mailbox. But the next refresh comes from a
//! timer, and until then the message list still shows the moved message.
//!
//! A removed row also removes the only record of an unsaved change of the user.
//! Camel keeps that change on the row, with the mark `CAMEL_MESSAGE_FOLDER_FLAGGED`,
//! until `synchronize_sync` writes it. Thus a move settles the row before it
//! removes it, with the same function as the synchronisation walk. For an
//! unchanged row this costs nothing: the diff is empty and there is no request.
//! Without this step, a move would lose a flag change silently.
//!
//! ## What this module does not decide
//!
//! **The rows of the destination folder.** This module adds no row there. The
//! message appears when the next listing of that folder occurs. Only the listing
//! knows the contents of a row: this module has only a uid. A row made from a
//! uid only would show no subject, sender or date until the refresh replaced it.
//!
//! ## Stopping a transfer
//!
//! The vfunc [`observe`]s the `cancellable` for the full call. A stop between two
//! messages keeps the messages already filed in their new mailbox: there is one
//! `Email/set` per message and no transaction over the set. For a move, this
//! folder also removes the rows of those messages, so the display agrees with
//! the server.
//!
//! ## The one folder-changing vfunc that Camel does not lock
//!
//! `camel_folder_refresh_info_sync`, `_synchronize_sync` and `_expunge_sync`
//! each hold `camel_folder_lock` on the folder for the full vfunc call. This is
//! verified in `camel-folder.c` of evolution-data-server 3.52.3, not taken from
//! the header. Thus, on one folder, a refresh, a synchronisation and an expunge
//! cannot run at the same time.
//! `camel_folder_transfer_messages_to_sync`'s wrapper is different. It locks
//! only the *destination*, and only on the cross-store path, which never comes
//! to this vfunc (it uses `append_message_sync`). It locks nothing on the source
//! before it calls this vfunc.
//!
//! Without its own lock, the `camel_folder_summary_remove_uid` call below can
//! occur during an unrelated refresh of the *source* folder's summary. That
//! refresh can have a listing from before this `Email/set`, which correctly names
//! the message. If the refresh applies that listing after the removal, the row
//! comes back. [`FolderLock`] closes that window, as the wrappers of the other
//! three vfuncs do.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr;

use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name, camel_folder_lock,
    camel_folder_summary_remove_uid, camel_folder_unlock,
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

/// Installs the filing vfunc of the folder on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that starts with a
/// `CamelFolderClass` (every descendant of `CamelFolder` does).
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.transfer_messages_to_sync = Some(transfer_messages_to_sync);
}

/// Files every named message into `destination`, and removes it from `source`
/// if the originals are to go.
///
/// Returns `TRUE` if nothing failed, otherwise `FALSE` with the error set. This
/// is Camel's convention, and `camel_folder_transfer_messages_to` reports it to
/// the user.
///
/// Camel calls this vfunc only for a transfer between two folders of one store.
/// `camel_folder_transfer_messages_to_sync` itself answers a transfer into the
/// same folder and a transfer of no messages. It sends a transfer between stores
/// through `get_message` and `append_message`. That path ends at
/// [`crate::append`], which uploads the message and files it with `Email/import`
/// (RFC 8621 §4.8). That is a different request, because this account has never
/// seen a message from a different account.
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
            // SAFETY: `source` is a live `CamelFolder` by the contract of this
            // function. The lock stays for the rest of the call (see the module
            // docs for the reason).
            let _lock = FolderLock::acquire(source);

            // SAFETY: Camel keeps its cancellable alive for the full call, so
            // it outlives this observation. Thus every request below stops when
            // the user presses Stop.
            let _cancel = observe(cancellable);

            let uids = uid_list(message_uids);
            let reported = Reported::new(transferred_uids, uids.len());

            // Check the type of `destination`, but not of `source`: GObject
            // dispatched on the class of the source, and the destination is any
            // folder of the caller. Camel uses the class of the *destination*
            // when it is a vtrash folder, so the wrapper permits a destination
            // of a different provider here.
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
            // A move into the current mailbox has no patch, so it must not
            // remove the rows. Camel's wrapper catches the same `CamelFolder`
            // twice, but not two folders of one mailbox.
            if filing.is_empty() {
                return GTRUE;
            }

            let mut changes = Changes::new();
            let mut failure = None;
            for (index, uid) in uids.iter().enumerate() {
                let filed = file_message(source, summary, uid, &filing, moving);
                // A message that is not on the server is also not in this
                // folder, for a copy and for a move.
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

            // This tells a message list on screen about the removed rows. The
            // summary rows only supply the next list that is drawn.
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

/// `camel_folder_lock`, released on drop. The module docs tell why
/// `transfer_messages_to_sync` must take it itself, unlike the other
/// folder-changing vfuncs.
///
/// An RAII guard, not a manual lock and unlock: the guarded function has
/// several early returns (a bad destination or source, no summary, a move into
/// the current mailbox). A later bug can leave a manual lock held on one of them.
struct FolderLock(*mut CamelFolder);

impl FolderLock {
    /// # Safety
    ///
    /// `folder` must be NULL or point at a live `CamelFolder`, kept alive for
    /// as long as the returned value is.
    unsafe fn acquire(folder: *mut CamelFolder) -> Self {
        // SAFETY: the contract above.
        unsafe { camel_folder_lock(folder) };
        Self(folder)
    }
}

impl Drop for FolderLock {
    fn drop(&mut self) {
        // SAFETY: by the contract of this type, the folder locked in `acquire`
        // is still live: nothing below the call frame of this vfunc released it.
        unsafe { camel_folder_unlock(self.0) };
    }
}

/// Files one message, and returns whether it landed.
///
/// `Ok(false)` means neither landed nor failed: the uid is not valid text, so
/// the server cannot receive it, and this provider did not make that row. The
/// row stays as it is: not reported as transferred, not removed, not a failure.
///
/// A move settles the row first (see the module header): the move removes the
/// row, which is the only record of an unsaved change. An unchanged row sends
/// no request there.
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
    // SAFETY: the contract above. `push_row` has the same contract for this pair.
    unsafe {
        if moving {
            push_row(folder, summary, uid)?;
        }
        let store = parent_store(folder).ok_or(StoreError::Disconnected)?;
        store.file_message(&Id::new(text), filing)?;
    }
    Ok(true)
}

/// The mailbox that `folder` shows, if `folder` is a folder of this provider.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn mailbox_of<'a>(folder: *mut CamelFolder) -> Option<&'a Id> {
    // SAFETY: the contract above. The type check makes the borrow below sound,
    // as it does for the store of a folder.
    unsafe { checked_borrow::<_, JmapFolder>(folder, folder_type())?.mailbox() }
}

/// A copy of the uids in Camel's array.
///
/// A copy, not a borrow: the walk removes rows from the summary and makes one
/// request per uid. The array belongs to the caller, and Camel's vee-folder path
/// rebuilds it between calls.
///
/// # Safety
///
/// `array` must be NULL or a live `GPtrArray` of NUL-terminated strings.
unsafe fn uid_list(array: *mut GPtrArray) -> Vec<CString> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above. Each string lives as long as the array, the
    // array outlives this call, and this code copies each string.
    unsafe {
        (0..(*array).len)
            .filter_map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                (!uid.is_null()).then(|| CStr::from_ptr(uid).to_owned())
            })
            .collect()
    }
}

/// The destination uids of the transferred messages, for a caller that asks.
///
/// In JMAP, the destination uid is the source uid. RFC 8621 §4.1 gives an
/// `Email` one immutable id per account. Filing into a different mailbox makes
/// no second object. Thus this code copies the uid; it does not search for one.
///
/// The array has the shape of Camel's generic transfer, because its callers free
/// that shape. The array has one slot per uid from the start. A message that did
/// not land has `NULL`. One `g_free` releases each string.
struct Reported(*mut GPtrArray);

impl Reported {
    /// Gives the caller an empty array of the correct size, or nothing if the
    /// caller did not ask.
    ///
    /// # Safety
    ///
    /// `out` must be NULL or writable.
    unsafe fn new(out: *mut *mut GPtrArray, len: usize) -> Self {
        if out.is_null() {
            return Self(ptr::null_mut());
        }
        // SAFETY: a new array, sized so that every slot exists before a write.
        // `set_size` sets the new slots to NULL, the value for a message that
        // did not land.
        unsafe {
            let array = g_ptr_array_new();
            g_ptr_array_set_size(array, len as c_int);
            *out = array;
            Self(array)
        }
    }

    /// Records the destination uid of the message at `index` of the caller's list.
    ///
    /// # Safety
    ///
    /// `index` must be below the length this was built with, and `uid`
    /// NUL-terminated.
    unsafe fn set(&self, index: usize, uid: &CStr) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: the contract above. The caller frees the copy.
        unsafe { *(*self.0).pdata.add(index) = g_strdup(uid.as_ptr()).cast() };
    }
}

/// The path that Camel uses as the key of the folder, for an error message.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    if folder.is_null() {
        return String::new();
    }
    // SAFETY: the accessor returns a string that the folder owns and that
    // outlives the call. `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
