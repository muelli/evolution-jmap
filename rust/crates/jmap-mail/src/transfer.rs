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
//! ## Offline
//!
//! `JmapStore::file_message` fails with `StoreError::Disconnected` with no
//! connection. `file_message` treats that failure as "write it later", the
//! same rule [`crate::expunge`] applies to a deleted mark: it records the
//! filing on the row ([`crate::message_info::pending_filing`]) and marks the
//! row the way an unsaved flag change already is, instead of failing the
//! whole call. The row stays in *this* folder's listing, for a move as much
//! as a copy, until `replay_filing` actually lands the filing from
//! `synchronize_sync`'s reconnect walk; only then is it removed and
//! `camel_folder_changed` fired. Camel and the user still see a move or copy
//! that succeeded right away, the same as a flag change does.
//!
//! The settle above is held over in the same way, because the row stays. The
//! reconnect walk writes the keyword before it sends the filing. See
//! `file_message` for the limit of this.
//!
//! ## Hiding the source row of a queued move
//!
//! A queued move stays in the source folder's summary (the section above),
//! but the user already dragged it away, so it must not still show there.
//! `queue_filing` marks it `CAMEL_MESSAGE_DELETED`, the same local mark a
//! queued delete uses to hide its own row by default. [`crate::expunge`]'s
//! own walk skips a row with a pending filing, so "Expunge" or "Empty
//! Trash" never destroys a message that is only mid-move. A refusal that no
//! retry can fix (`replay_filing`) clears the mark again, the same change
//! that clears the filing itself. A copy sets no mark: the message never
//! left the source mailbox.
//!
//! ## The rows of the destination folder
//!
//! A filing that reached the server adds no row there. The message appears
//! when the next listing of that folder occurs. Only the listing knows the
//! contents of a row, and this module has only a uid.
//!
//! A *queued* filing is different, because no listing can show it: the server
//! does not have the change yet. `place_in_destination` therefore clones the
//! source row into the destination summary at queue time, under the same uid.
//! A clone has the subject, the sender and the date, so the user sees the move
//! or the copy at once.
//!
//! The clone carries the same filing as the row it came from. Thus the walk
//! of the destination folder alone lands the whole move, even when the
//! source folder is not walked. This matters after a restart:
//! `camel_store_synchronize_sync` walks only the folders that are open
//! (`store_synchronize_sync` in `camel-store.c` of evolution-data-server
//! 3.52.3), and a restart leaves that list empty. The source row keeps the
//! same filing, so a walk there lands the move as well. The two requests are
//! equal, thus the order of the two walks does not change the result. A walk
//! removes its row only in the folder the message leaves (`left_here`).
//!
//! This shape also keeps the walk of one folder inside that folder. A walk
//! that opened a second folder to find its queued work would hold the lock of
//! the first folder while it takes the lock of the second one. A concurrent
//! walk of the second folder would do the opposite, and the two walks would
//! then wait for each other.
//!
//! ## A message the user moves twice before the first move is sent
//!
//! The first move already shows the message in its new folder, so the user
//! can move it again from there. The two moves become one filing: from the
//! mailbox the message started in, straight into the mailbox the user chose
//! last (`rechain`). No request then names the mailbox the message only
//! passes through, and the row in that folder goes away.
//!
//! One request per hop would not do. Nothing orders the walk of one folder
//! against the walk of another, so the request of the first move can reach
//! the server after the request of the second one. The message would then
//! stay in the folder it only passed through. For the same reason the row
//! that holds the first move must change with it, which is why `rechain`
//! opens the folder that row is in, and writes the row to the summary
//! database before it lets that folder go.
//!
//! A *copy* out of such a row joins nothing. It only adds a mailbox, and the
//! queued move must still land, so the two requests are independent in any
//! order. The row keeps the move it waits for, and the new destination row
//! carries the copy alone.
//!
//! ## A filing that the server refuses for good
//!
//! The destination row is a promise that this provider made alone, before
//! the server saw the filing. A refusal that no retry changes
//! ([`StoreError::refuses_retry`]) breaks that promise, so `replay_filing`
//! takes the row away again and reports the reason of the server. The source
//! row of a move keeps its place, because the message never left that
//! mailbox; only its filing goes, so the next walk sends no request that the
//! server refuses again.
//!
//! ## A filing the server says has no message
//!
//! A `notFound` refusal of the `Email/set` means the message is not in this
//! mailbox either, so the row goes at once, the same as a message another
//! client destroyed. But a hostile or merely buggy server can answer
//! `notFound` for an id it still lists everywhere else, and this code cannot
//! tell that apart from a genuine deletion. Removing the row is still
//! right for the genuine case, so it also forgets the folder's recorded
//! state ([`crate::summary::clear_summary_state`]): the next refresh lists
//! the mailbox in full rather than asking `Email/changes` since a state the
//! refusal never moved, and a server that still lists the message brings the
//! row back then (SECURITY-AUDIT-2026-11-01 row S3-3).
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
//! comes back. `FolderLock` closes that window, as the wrappers of the other
//! three vfuncs do.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr;

use eds_sys::{
    CAMEL_MESSAGE_DELETED, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    CamelFolderSummary, CamelMessageInfo, camel_folder_changed, camel_folder_get_folder_summary,
    camel_folder_get_full_name, camel_folder_get_parent_store, camel_folder_lock,
    camel_folder_summary_add, camel_folder_summary_check_uid, camel_folder_summary_get,
    camel_folder_summary_lock, camel_folder_summary_remove_uid, camel_folder_summary_save,
    camel_folder_summary_touch, camel_folder_summary_unlock, camel_folder_unlock,
    camel_message_info_clone, camel_message_info_set_dirty, camel_message_info_set_flags,
    camel_message_info_set_folder_flagged, camel_store_get_folder_sync,
};
use gio_sys::GCancellable;
#[cfg(camel_folder_get_uids)]
use glib_sys::g_ptr_array_new;
use glib_sys::{
    GError, GFALSE, GPtrArray, GTRUE, g_clear_error, g_ptr_array_set_size, g_strdup, gboolean,
    gchar,
};
#[cfg(not(camel_folder_get_uids))]
use glib_sys::{g_free, g_ptr_array_new_with_free_func};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::fail_bool;
use jmap_backend_core::marshal::{checked_borrow, read_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::Filing;
use jmap_proto::Id;

use crate::changes::Changes;
use crate::connect::StoreError;
use crate::folder::{JmapFolder, folder_type, parent_store};
use crate::message_info::{
    pending_append, pending_filing, set_pending_filing, swap_pending_filing,
};
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
                false => Filing::copied_from(out_of.clone(), into.clone()),
            };
            // A move into the current mailbox has no patch, so it must not
            // remove the rows. Camel's wrapper catches the same `CamelFolder`
            // twice, but not two folders of one mailbox.
            if filing.is_empty() {
                return GTRUE;
            }

            let mut changes = Changes::new();
            let mut arrivals = Changes::new();
            let mut failure = None;
            for (index, uid) in uids.iter().enumerate() {
                // A NULL slot in Camel's own array never happens in practice
                // (see `uid_list`'s doc). There is no uid to file, so this
                // slot stays unreported, the same as a message this loop
                // never reached.
                let Some(uid) = uid else {
                    continue;
                };
                let filed = file_message(source, summary, uid, &filing, moving);
                // A message that is not on the server is also not in this
                // folder, for a copy and for a move. A queued filing stays in
                // this folder until `synchronize_sync`'s walk confirms it:
                // removing it now would say "moved" before the server has
                // agreed, the offline instance of the module doc's "effect of
                // a move on the source folder" rule. A row this folder only
                // passed through goes at once: it stands for a request that
                // the second move has taken over.
                let leaves = match &filed {
                    Ok(Filed::Landed) => moving,
                    Ok(Filed::Rechained(_)) => true,
                    Ok(Filed::Queued(_)) | Ok(Filed::NoOp) => false,
                    Err(StoreError::NoMessage(_)) => true,
                    Err(_) => false,
                };
                // The destination row is a clone of this folder's row, so it
                // is taken before the removal above can take that row away.
                if let Ok(Filed::Queued(queued) | Filed::Rechained(queued)) = &filed
                    && place_in_destination(summary, destination, uid, queued)
                    && let Ok(text) = uid.to_str()
                {
                    arrivals.add(text);
                }
                if leaves {
                    camel_folder_summary_remove_uid(summary, uid.as_ptr());
                    if let Ok(text) = uid.to_str() {
                        changes.remove(text);
                    }
                    // A notFound refusal removes this row on this folder's own
                    // say-so, not on a listing. Forgetting the recorded state
                    // makes the next refresh list in full instead of asking
                    // `Email/changes` since a state this refusal never moved,
                    // so a server that still lists the message brings the row
                    // back (SECURITY-AUDIT-2026-11-01 row S3-3).
                    if matches!(filed, Err(StoreError::NoMessage(_))) {
                        crate::summary::clear_summary_state(summary);
                    }
                }
                match filed {
                    Ok(Filed::Landed) | Ok(Filed::Queued(_)) | Ok(Filed::Rechained(_)) => {
                        reported.set(index, uid)
                    }
                    Ok(Filed::NoOp) => {}
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
            // The same report for the destination folder, which a queued
            // filing gives a row of its own.
            if !arrivals.is_empty() {
                camel_folder_changed(destination, arrivals.as_ptr());
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

/// What happened when this provider tried to file one message.
enum Filed {
    /// The filing reached the server.
    Landed,
    /// There is no connection. The row now carries the filing, and
    /// `synchronize_sync`'s reconnect walk ([`replay_filing`]) sends it once
    /// the store is back online, the offline-journal shape [`crate::expunge`]
    /// already uses for a deleted mark. The filing is also what the
    /// destination row gets.
    Queued(Filing),
    /// There is no connection, and this row was itself waiting for an
    /// earlier queued filing: the folder is one the message only passes
    /// through. [`rechain`] has joined the two filings into one, which the
    /// destination row gets. The caller removes this row.
    Rechained(Filing),
    /// The uid is not valid text, or the row is already gone (a concurrent
    /// refresh removed it): neither landed nor failed.
    NoOp,
}

/// Files one message, and returns what happened.
///
/// A move settles the row first (see the module header): the move removes the
/// row, which is the only record of an unsaved change. An unchanged row sends
/// no request there. A settle failure is reported as is: an unsaved flag
/// change that could not be written must not be folded silently into a filing
/// that then "succeeds" differently (queued instead of sent).
///
/// A settle that fails with no connection is the one exception, and only when
/// the filing below is queued as well. The reason the failure matters is the
/// removal of the row, and a queued filing removes no row: the row stays in
/// this folder with the keyword and the mark `CAMEL_MESSAGE_FOLDER_FLAGGED`
/// still on it, and `synchronize_sync`'s reconnect walk calls `push_row` on
/// it again, before `replay_filing` sends the filing. The change is thus held
/// over, not lost. Evolution's mark-seen timer dirties the row the cursor
/// rests on, so without this exception an offline move of the selected
/// message almost always fails.
///
/// A filing that lands still takes the row away, so that case reports the
/// held-over failure to the caller too. A rechain would also take the row
/// away, but it reaches a *different* row first: [`rechain`] rewrites the
/// filing of the row the first queued filing started on, a durable write
/// (see [`rewrite_origin_row`]) that a later refusal cannot take back. So a
/// held-over settle refuses the rechain itself, before `queue_filing` can
/// touch that other row: this row then keeps the filing it already had,
/// the other row's filing is untouched, and nothing is rewritten on the
/// strength of a filing this call is about to report as failed.
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
) -> Result<Filed, StoreError> {
    let Ok(text) = uid.to_str() else {
        return Ok(Filed::NoOp);
    };
    // SAFETY: the contract above. `push_row` has the same contract for this pair.
    unsafe {
        let settled = match moving {
            true => push_row(folder, summary, uid),
            false => Ok(()),
        };
        let held_over = match settled {
            Ok(()) => false,
            Err(StoreError::Disconnected) => true,
            Err(other) => return Err(other),
        };
        let store = parent_store(folder).ok_or(StoreError::Disconnected)?;
        let owner = JmapFolder::borrow(folder).and_then(JmapFolder::owner);
        let filed = match store.file_message(&Id::new(text), owner, filing) {
            Ok(()) => Filed::Landed,
            Err(StoreError::Disconnected) => queue_filing(folder, summary, uid, filing, held_over)?,
            Err(other) => return Err(other),
        };
        // The row of a filing that did not land carries the unsaved keyword
        // to the reconnect walk. A row that goes away carries nothing.
        if held_over && !matches!(filed, Filed::Queued(_) | Filed::NoOp) {
            return Err(StoreError::Disconnected);
        }
        Ok(filed)
    }
}

/// Records `filing` on the row so [`replay_filing`] can send it once the
/// store reconnects, and marks the row for that walk to find.
///
/// If the row vanished since the caller listed it (a concurrent refresh),
/// there is nothing to mark, and the caller treats the uid as [`Filed::NoOp`].
///
/// A row that already waits for an earlier queued filing takes the other path
/// ([`rechain`]), which joins the two filings instead of queueing a second
/// request beside the first. A copy beside a waiting filing does not join:
/// the row keeps the filing it waits for, and only the new destination row
/// carries the copy. A move out of a row that waits for a move, or out of a
/// row that waits for a copy with a known source, does join: the row the
/// first filing started on (named by [`Filing::out_of_mailbox`] for a move,
/// [`Filing::copy_source`] for a copy) gets the combined filing instead.
///
/// `held_over` is [`file_message`]'s own settle failure, carried here so a
/// rechain can refuse it before it rewrites the other row: see
/// [`file_message`]'s doc comment for why.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder` and `summary` at its summary.
unsafe fn queue_filing(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
    filing: &Filing,
    held_over: bool,
) -> Result<Filed, StoreError> {
    // SAFETY: the contract above. `Owned` releases the row's reference when it
    // drops at the end of this function.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        return Ok(Filed::NoOp);
    };
    // SAFETY: `info` is a live row for as long as this function holds it, and
    // `folder` is a live folder of this provider.
    if let Some(waiting) = unsafe { pending_filing(info.as_ptr()) }
        && unsafe { arrived_here(folder, &waiting) }
    {
        // The message is not in this mailbox on the server yet. A copy out
        // of it still only adds the new mailbox, so the two requests are
        // independent and their order does not change the result. This row
        // keeps the filing that brings the message here, unchanged: *this*
        // row's own mailbox is not a safe `copy_source` for the new row,
        // because this row does not hold the new copy and will not be
        // rewritten to match it. Drop any source the caller gave the new
        // filing, so a later move out of the new row falls through and
        // overwrites it instead of rechaining onto this one.
        if filing.out_of_mailbox().is_none() {
            tracing::trace!(
                uid = uid.to_string_lossy().as_ref(),
                "queued a copy beside a waiting filing"
            );
            return Ok(Filed::Queued(Filing::copied_into(
                filing.into_mailbox().clone(),
            )));
        }
        // A held-over settle and a rechain do not mix: refuse here, before
        // either row changes, rather than rewrite the other row's filing on
        // the strength of a filing this call is about to report as failed.
        if held_over {
            return Err(StoreError::Disconnected);
        }
        if let Some(origin) = waiting.out_of_mailbox() {
            let joined = Filing::moved(origin.clone(), filing.into_mailbox().clone());
            // SAFETY: the contract above.
            return Ok(unsafe { rechain(folder, uid, origin, joined) });
        }
        // The waiting filing is a copy with a known source: a move out of
        // it now must take the message out of its *source*, not out of
        // this row's own mailbox, which the message never stayed in.
        if let Some(source) = waiting.copy_source() {
            let joined = Filing::copied_from(source.clone(), filing.into_mailbox().clone());
            // SAFETY: the contract above.
            return Ok(unsafe { rechain(folder, uid, source, joined) });
        }
    }
    // SAFETY: `info` is a live row for as long as this function holds it.
    unsafe {
        set_pending_filing(info.as_ptr(), Some(filing.clone()));
        camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE);
        // A move leaves this mailbox. Hide the row now, the module doc's
        // "Hiding the source row of a queued move" section.
        if filing.out_of_mailbox().is_some() {
            camel_message_info_set_flags(
                info.as_ptr(),
                CAMEL_MESSAGE_DELETED,
                CAMEL_MESSAGE_DELETED,
            );
        }
    }
    tracing::trace!(uid = uid.to_string_lossy().as_ref(), "queued filing");
    Ok(Filed::Queued(filing.clone()))
}

/// Joins a second queued filing to the first one, and gives back the one
/// filing that both folders then carry.
///
/// `origin` names the row the first filing started on: the mailbox a queued
/// move takes the message out of, or the mailbox a queued copy came from.
/// `joined` is the combined filing, built by the caller: a move chain joins
/// into another move, straight from `origin`; a copy that a later move
/// takes out of its destination before it lands joins into a copy, straight
/// from `origin`. Either way no request names the mailbox of `folder`,
/// which the message only passes through. The caller removes the row of
/// `folder` and clones the row into the new destination.
///
/// The row that holds the first filing (the row in the folder of `origin`)
/// must change with it. A request that still files the message into the
/// mailbox the user has left would put it back there: nothing orders the
/// walk of one folder against the walk of another, so that request can reach
/// the server last. Thus this function opens the folder of `origin` and
/// writes the joined filing on its row.
///
/// `camel_store_get_folder_sync` takes no folder lock, so the lock this
/// vfunc holds on `folder` cannot meet one. It waits only while another
/// thread builds that same folder. Writing the row of the other folder needs
/// no folder lock either: the row has its own, as the clone that
/// [`place_in_destination`] adds to another summary already relies on.
///
/// A move back into `origin` is empty. There is then nothing left to ask the
/// server for, so the first move goes away: the row of `origin` loses its
/// filing and shows again.
///
/// If the folder of `origin` cannot be opened (the account has no listing,
/// or the folder is gone), the joined filing is still correct for the two
/// folders this call has. The row of `origin` then keeps the first filing.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder`.
unsafe fn rechain(folder: *mut CamelFolder, uid: &CStr, origin: &Id, joined: Filing) -> Filed {
    // SAFETY: the contract above.
    unsafe { rewrite_origin_row(folder, uid, origin, &joined) };
    tracing::trace!(
        uid = uid.to_string_lossy().as_ref(),
        "rechained queued filing"
    );
    Filed::Rechained(joined)
}

/// Writes `joined` on the row that holds the first filing, in the folder
/// that shows `origin`. An empty `joined` clears that row's filing instead.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder`.
unsafe fn rewrite_origin_row(folder: *mut CamelFolder, uid: &CStr, origin: &Id, joined: &Filing) {
    // SAFETY: the contract above. The store outlives this vfunc call.
    let Some(path) = (unsafe { parent_store(folder) }).and_then(|store| store.folder_path(origin))
    else {
        return;
    };
    let Ok(path) = CString::new(path) else {
        return;
    };
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the contract above gives a live folder, thus a live store. The
    // path is NUL-terminated and alive across the call, and the error
    // out-parameter is writable and NULL. `Owned` releases the reference that
    // the call returns.
    let opened = unsafe {
        let store = camel_folder_get_parent_store(folder);
        let opened = camel_store_get_folder_sync(
            store,
            path.as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        );
        g_clear_error(&mut error);
        Owned::from_raw(opened)
    };
    let Some(opened) = opened else {
        return;
    };
    // SAFETY: a live folder, thus a live summary or NULL.
    let summary = unsafe { camel_folder_get_folder_summary(opened.as_ptr()) };
    if summary.is_null() {
        return;
    }
    // SAFETY: a live summary, and a uid that outlives the call. `Owned`
    // releases the row's reference when it drops at the end of this function.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        return;
    };
    // SAFETY: `info` is a live row for as long as this function holds it.
    unsafe {
        if joined.is_empty() {
            set_pending_filing(info.as_ptr(), None);
            // Undo the hide first: `camel_message_info_set_flags` sets
            // `CAMEL_MESSAGE_FOLDER_FLAGGED` again on any change it makes,
            // so the clear below must come after it.
            camel_message_info_set_flags(info.as_ptr(), CAMEL_MESSAGE_DELETED, 0);
            camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE);
        } else {
            set_pending_filing(info.as_ptr(), Some(joined.clone()));
            camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE);
        }
    }
    // Write the row to the database now. The filing itself is a column that
    // no Camel accessor touches, and the one bit above is already set for a
    // row that waits, so nothing else marks this row for a save. The folder
    // is also open for this call alone, and Camel keeps only a weak
    // reference to it, so it can go as soon as the reference below does.
    // SAFETY: a live row, and the live summary that row belongs to.
    unsafe {
        camel_message_info_set_dirty(info.as_ptr(), GTRUE);
        camel_folder_summary_touch(summary);
        let mut save_error: *mut GError = ptr::null_mut();
        camel_folder_summary_save(summary, &mut save_error);
        g_clear_error(&mut save_error);
    }
}

/// Gives the destination folder a row for a message whose filing is still
/// queued, and reports whether it added one.
///
/// The row is a clone of the source row, under the same uid. JMAP gives a
/// message one id per account (RFC 8621 §4.1), and filing it into another
/// mailbox makes no second object, so the uid of the destination row is the
/// uid the message already has.
///
/// The clone carries the same filing as the row it came from. Thus the walk
/// of the destination folder alone completes the whole move: it files the
/// message in and takes it out of the mailbox it came from, in one patch.
/// The walk removes a row only in the folder the message leaves
/// ([`left_here`]), so this row stays where the user put it.
///
/// The clone also loses the hide mark that the source row of a move carries
/// ([`queue_filing`]). `camel_message_info_clone` copies the flags word, and
/// a destination row that keeps that mark hides the message in the folder
/// the user chose, as well as in the folder it came from.
///
/// Four rows get no row in the destination:
///
/// - A row that waits for its own upload ([`crate::append`]'s queue). Its uid
///   names nothing on the server, so a second row would ask for a second
///   upload of the same message.
/// - A row the destination already has. The destination is then a mailbox the
///   message is already in, or a second queued filing of the same message.
///   The test for it and the add are one step, under the lock of the
///   destination summary: see the comment there.
/// - A row that vanished since the caller listed it.
/// - A row whose filing is empty: a second queued move that comes back to the
///   mailbox the first one started from asks the server for nothing.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary` and `destination` at a
/// live `CamelFolder`.
unsafe fn place_in_destination(
    summary: *mut CamelFolderSummary,
    destination: *mut CamelFolder,
    uid: &CStr,
    filing: &Filing,
) -> bool {
    if filing.is_empty() {
        return false;
    }
    // SAFETY: the contract above.
    let target = unsafe { camel_folder_get_folder_summary(destination) };
    if target.is_null() || target == summary {
        return false;
    }
    // SAFETY: the contract above. `Owned` releases the row's reference when it
    // drops at the end of this function.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        return false;
    };
    // SAFETY: `info` is a live row for as long as this function holds it.
    if unsafe { pending_append(info.as_ptr()) }.is_some() {
        return false;
    }
    // SAFETY: `info` is a live row and `target` a live summary of this
    // provider, so the clone comes back as a row of this provider's own type
    // with `target` as its summary.
    let copy = unsafe { Owned::from_raw(camel_message_info_clone(info.as_ptr(), target)) };
    let Some(copy) = copy else {
        return false;
    };
    // SAFETY: `copy` is a row this function owns until `camel_folder_summary_
    // add` below gives the summary its own reference. `add` also marks the row
    // for the work list that `crate::synchronize` walks.
    unsafe {
        set_pending_filing(copy.as_ptr(), Some(filing.clone()));
        camel_message_info_set_flags(copy.as_ptr(), CAMEL_MESSAGE_DELETED, 0);
    }
    // The question and the add are one step. `force_keep_uid` keeps the uid of
    // the clone, which also tells Camel to skip its own test for a uid the
    // summary has already (`camel_folder_summary_add` in
    // `camel-folder-summary.c` of evolution-data-server 3.52.3). A refresh of
    // this same folder adds its rows under this same lock
    // ([`crate::summary::apply_delta`]), so a uid that arrives between the two
    // would get a second add, and with it a second count of one message.
    //
    // The lock holds across these two calls alone. Both take this summary and
    // no other lock, so no other thread can wait the other way round. The work
    // above needs the summary of the *source* and the row of the clone, and
    // stays outside.
    // SAFETY: a live summary, a uid that outlives the call, and a row this
    // function owns. Camel's summary lock is recursive: both calls take it
    // again themselves.
    let added = unsafe {
        camel_folder_summary_lock(target);
        let arrived = camel_folder_summary_check_uid(target, uid.as_ptr()) != GFALSE;
        if !arrived {
            camel_folder_summary_add(target, copy.as_ptr(), GTRUE);
        }
        camel_folder_summary_unlock(target);
        !arrived
    };
    if !added {
        return false;
    }
    tracing::trace!(uid = uid.to_string_lossy().as_ref(), "queued arrival");
    true
}

/// What one [`replay_filing`] call leaves for its caller to do.
pub(crate) struct Replay {
    /// Whether the caller removes this row from the summary and tells Camel.
    pub(crate) remove: bool,
    /// The failure to report, if the call had one.
    pub(crate) failure: Option<StoreError>,
}

impl Replay {
    /// The row stays as it is and there is nothing to report.
    fn nothing() -> Self {
        Self {
            remove: false,
            failure: None,
        }
    }
}

/// Completes a filing that [`file_message`] could not send while offline, for
/// one row of the work list [`crate::synchronize::synchronize_sync`] walks.
///
/// A landed move asks the caller to remove the row, the same as an immediate
/// move in [`transfer_messages_to_sync`] above. A copy never removes a row.
///
/// Still offline is not a failure, the same "write it later" rule
/// [`crate::expunge`] uses for a deleted mark: the row and its filing stay,
/// untouched, for the next walk. A transport failure gets the same treatment.
///
/// A refusal that no retry changes ends the filing instead (the module
/// header, "A filing that the server refuses for good"). Both the removal of
/// the destination row and the reason go back to the caller, which has the
/// one `camel_folder_changed` of the walk and its error report.
///
/// Each of those outcomes writes the row after the request, and the row is
/// not locked across it. A filing that another thread queued on the row in
/// that window is not this call's to clear, and the row stays where it is,
/// because the row carries that filing: see [`settle_sent_filing`].
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder` and `summary` at its summary.
pub(crate) unsafe fn replay_filing(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
) -> Replay {
    // SAFETY: the contract above. `Owned` releases the row's reference when it
    // drops at the end of this function.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        return Replay::nothing();
    };
    // SAFETY: `info` is a live row for as long as this function holds it.
    let Some(filing) = (unsafe { pending_filing(info.as_ptr()) }) else {
        return Replay::nothing();
    };
    let Ok(text) = uid.to_str() else {
        return Replay::nothing();
    };
    let moving = filing.out_of_mailbox().is_some();
    // SAFETY: the contract above: a live folder of this provider.
    let leaves_here = unsafe { left_here(folder, &filing) };
    // SAFETY: the contract above.
    // SAFETY: the contract above.
    let owner = unsafe { JmapFolder::borrow(folder) }.and_then(JmapFolder::owner);
    let result = match unsafe { parent_store(folder) } {
        Some(store) => store.file_message(&Id::new(text), owner, &filing),
        None => Err(StoreError::Disconnected),
    };
    match result {
        Ok(()) => {
            // SAFETY: `info` is still the live row.
            if !unsafe { settle_sent_filing(info.as_ptr(), &filing) } {
                // The row still has a filing to send. Keep it on the work
                // list: an empty join cleared the mark with the filing it
                // took away, and `push_row` cleared it before the request.
                // SAFETY: `info` is still the live row.
                unsafe { camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE) };
                tracing::trace!(uid = text, "amended a filing queued during the replay");
                return Replay::nothing();
            }
            // SAFETY: `info` is still the live row.
            unsafe { camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE) };
            tracing::trace!(uid = text, "replayed filing");
            Replay {
                remove: leaves_here,
                failure: None,
            }
        }
        Err(StoreError::Disconnected) => {
            // Still offline. `push_row`, called just before this in the same
            // walk, clears the row's mark whenever it finds no keyword change
            // of its own to write; restore it here, or the row would fall off
            // the work list with its filing still unsent.
            // SAFETY: `info` is still the live row.
            unsafe { camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE) };
            Replay::nothing()
        }
        Err(StoreError::NoMessage(_)) => {
            // Another client destroyed the message before this filing could
            // land. There is nothing left to file. The row itself is not
            // removed here, the same restraint `push_row` already applies to
            // a vanished message: the next refresh finds it gone and removes it.
            // SAFETY: `info` is still the live row.
            if unsafe { swap_pending_filing(info.as_ptr(), Some(&filing), None) }.is_err() {
                // A filing queued during the request, for a message the
                // server no longer has. It is not this call's to clear: the
                // next walk sends it, and ends it the same way.
                tracing::trace!(uid = text, "kept a filing queued during the replay");
                return Replay::nothing();
            }
            // SAFETY: `info` is still the live row.
            unsafe {
                camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE);
            }
            tracing::trace!(uid = text, "dropped filing");
            Replay::nothing()
        }
        // A refusal that no retry changes. Undo the filing, and the
        // destination row that showed it, instead of asking again for ever.
        Err(refusal) if refusal.refuses_retry() => {
            // SAFETY: `info` is still the live row.
            if unsafe { swap_pending_filing(info.as_ptr(), Some(&filing), None) }.is_err() {
                // A filing queued during the request. The refused one never
                // reached the server, so that filing is still true for a
                // message that never moved: keep it, and the hide and the
                // mark its own writer set with it. The refusal still goes
                // to the caller.
                tracing::trace!(uid = text, "kept a filing queued during the replay");
                return Replay {
                    remove: false,
                    failure: Some(refusal),
                };
            }
            // SAFETY: `info` is still the live row.
            unsafe {
                // Undoes queue_filing's hide, before the folder-flagged
                // clear below: Camel's own `camel_message_info_set_flags`
                // sets `CAMEL_MESSAGE_FOLDER_FLAGGED` again on any change
                // it makes, so clearing it first would not last. A no-op
                // for the destination row of a copy or a move, which this
                // mark never reached.
                if moving {
                    camel_message_info_set_flags(info.as_ptr(), CAMEL_MESSAGE_DELETED, 0);
                }
                camel_message_info_set_folder_flagged(info.as_ptr(), GFALSE);
            }
            tracing::trace!(uid = text, "rolled back filing");
            Replay {
                // SAFETY: the contract above: a live folder of this provider.
                remove: unsafe { arrived_here(folder, &filing) },
                failure: Some(refusal),
            }
        }
        Err(other) => {
            // Still pending: a transport failure (a DNS failure, a refused
            // connection, a TLS failure) is as unsent as `Disconnected`
            // above, and needs the same restore of the mark `push_row`
            // may have cleared, or the filing falls off the work list with
            // no way back onto it.
            // SAFETY: `info` is still the live row.
            unsafe { camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE) };
            tracing::trace!(uid = text, "kept filing pending after a transport failure");
            Replay {
                remove: false,
                failure: Some(other),
            }
        }
    }
}

/// Takes the filing that [`replay_filing`] sent off the row, now that the
/// server has it, and gives back whether the row has nothing left to send.
///
/// The row is not locked across the request. Another thread can queue a
/// filing on it in that window: [`rewrite_origin_row`] writes the joined
/// filing of a [`rechain`] here, or clears it for a join that is empty. Such
/// a filing was built for a message that is still where it was, because the
/// first filing of the chain had not been sent yet. This call sent it, so
/// the message is in `sent.into_mailbox()` now, a mailbox the joined filing
/// names nowhere: the message only passes through it. The filing thus gets
/// the one amendment that makes it true again, a move out of that mailbox
/// into the destination the user chose. The row keeps it for the next walk.
///
/// A filing from any other writer stands as it is. It was built for the
/// message where the user saw it, and it adds or removes its own mailboxes,
/// with no assumption about the one this call just filed into.
///
/// Each write goes through [`swap_pending_filing`], so this call only ever
/// replaces a filing it has read. A write that loses to yet another thread
/// reads that thread's filing and answers for it instead.
///
/// # Safety
///
/// `info` must point at a live `CamelMessageInfo`.
unsafe fn settle_sent_filing(info: *mut CamelMessageInfo, sent: &Filing) -> bool {
    let mut held = Some(sent.clone());
    loop {
        let next = match &held {
            // The filing this call sent, and the server has it now.
            Some(filing) if filing == sent => None,
            Some(filing) if rechained_from(sent, filing) => Some(Filing::moved(
                sent.into_mailbox().clone(),
                filing.into_mailbox().clone(),
            )),
            Some(filing) => Some(filing.clone()),
            // An empty join: the chain goes back to the mailbox it started
            // in, which a move out of this call's destination reaches now.
            None => sent
                .out_of_mailbox()
                .map(|origin| Filing::moved(sent.into_mailbox().clone(), origin.clone())),
        };
        let next = next.filter(|filing| !filing.is_empty());
        let settled = next.is_none();
        // SAFETY: the contract above.
        match unsafe { swap_pending_filing(info, held.as_ref(), next) } {
            Ok(()) => return settled,
            Err(other) => held = other,
        }
    }
}

/// Whether `queued` is `sent` after a [`rechain`]: the same chain, out of
/// the same row, with the destination the user chose last.
///
/// A rechain keeps the mailbox the chain starts in ([`Filing::out_of_mailbox`]
/// for a move, [`Filing::copy_source`] for a copy) and changes only the
/// destination. A filing with no such mailbox starts no chain, and a filing
/// that names another one belongs to another chain. Both are the work of a
/// writer that only overwrote this row.
fn rechained_from(sent: &Filing, queued: &Filing) -> bool {
    let starts_a_chain = sent.out_of_mailbox().is_some() || sent.copy_source().is_some();
    starts_a_chain
        && sent.out_of_mailbox() == queued.out_of_mailbox()
        && sent.copy_source() == queued.copy_source()
}

/// Whether the message leaves the mailbox of this folder when the filing
/// lands, so that the row goes with it.
///
/// Only the folder the move takes the message out of loses its row. The
/// destination row carries the same move filing ([`place_in_destination`]),
/// and a walk there must keep its own row: the message arrives in that
/// mailbox. A copy removes no row anywhere.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn left_here(folder: *mut CamelFolder, filing: &Filing) -> bool {
    let Some(out_of) = filing.out_of_mailbox() else {
        return false;
    };
    // SAFETY: the contract above.
    (unsafe { mailbox_of(folder) }) == Some(out_of)
}

/// Whether this row is the arrival record that the folder keeps for itself:
/// the clone of [`place_in_destination`], not the row the transfer came from.
///
/// The mailbox of the folder gives the answer. Only the clone sits in the
/// mailbox that its own filing names. A source row names a different mailbox,
/// for a move and for a copy: a filing into the mailbox that the message is
/// already in is empty, and `transfer_messages_to_sync` stops before it
/// queues one.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `CamelFolder`.
unsafe fn arrived_here(folder: *mut CamelFolder, filing: &Filing) -> bool {
    // SAFETY: the contract above.
    (unsafe { mailbox_of(folder) }) == Some(filing.into_mailbox())
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

/// A copy of the uids in Camel's array, one slot per input element, in order.
///
/// A copy, not a borrow: the walk removes rows from the summary and makes one
/// request per uid. The array belongs to the caller, and Camel's vee-folder path
/// rebuilds it between calls.
///
/// A slot is `None` only for a NULL element of `array`. Camel never puts one
/// there in practice, but [`Reported::new`] sizes its own output array from
/// this list's length, which must match the *input* array's length and keep
/// its positions: dropping a hole instead of keeping its slot would shift
/// every later uid's reported result one position out of place.
///
/// # Safety
///
/// `array` must be NULL or a live `GPtrArray` of NUL-terminated strings.
unsafe fn uid_list(array: *mut GPtrArray) -> Vec<Option<CString>> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above. Each string lives as long as the array, the
    // array outlives this call, and this code copies each string.
    unsafe {
        (0..(*array).len)
            .map(|index| {
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
/// not land has `NULL`.
///
/// The way a caller releases a filled slot's string depends on the installed
/// EDS, the same split [`crate::message::new_uncached_uids_array`] already
/// names. Before EDS 3.58, Camel's own `folder_transfer_messages_to_sync`
/// builds this array with a plain `g_ptr_array_new()`, and every Camel-side
/// consumer frees it as elements then container
/// (`g_ptr_array_foreach(g_free)`, then `g_ptr_array_free`). From 3.58, the
/// same function gives the array its own `g_free` and a caller frees it with
/// one call. Giving the wrong one here either leaks (no free function, freed
/// with one call) or double frees (free function present, a caller also
/// frees each element the old way).
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
        // did not land. The ownership contract of a filled slot's string is
        // the struct doc's EDS-version split.
        unsafe {
            #[cfg(camel_folder_get_uids)]
            let array = g_ptr_array_new();
            #[cfg(not(camel_folder_get_uids))]
            let array = g_ptr_array_new_with_free_func(Some(g_free));
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

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::ptr;

    use glib_sys::{g_free, g_ptr_array_add, g_ptr_array_free, g_ptr_array_new_with_free_func};
    use jmap_mail_sync::Filing;
    use jmap_proto::Id;

    use super::{rechained_from, uid_list};

    /// A NULL element of Camel's uid array never happens in practice, but
    /// `uid_list` must keep its position rather than compact it away:
    /// `Reported::new` sizes its own output array from this list's length,
    /// which Camel's contract keys to the *input* array's length and
    /// positions.
    #[test]
    fn a_null_hole_in_camels_array_keeps_its_own_position() {
        // SAFETY: a `GPtrArray` built and freed entirely within this test,
        // never shared with another thread or call.
        unsafe {
            let array = g_ptr_array_new_with_free_func(Some(g_free));
            let first = CString::new("E1").unwrap();
            let third = CString::new("E3").unwrap();
            g_ptr_array_add(array, glib_sys::g_strdup(first.as_ptr()).cast());
            g_ptr_array_add(array, ptr::null_mut());
            g_ptr_array_add(array, glib_sys::g_strdup(third.as_ptr()).cast());

            let uids = uid_list(array);

            g_ptr_array_free(array, glib_sys::GTRUE);

            assert_eq!(
                uids.len(),
                3,
                "a hole must not shorten the list Reported::new sizes from"
            );
            assert_eq!(uids[0].as_deref(), Some(first.as_c_str()));
            assert_eq!(uids[1], None, "the hole stays a hole, at its own index");
            assert_eq!(uids[2].as_deref(), Some(third.as_c_str()));
        }
    }

    /// `settle_sent_filing` amends a rechained filing, and only that one. A
    /// rechain keeps the mailbox the chain starts in and changes only the
    /// destination, so the two filings agree on that mailbox.
    #[test]
    fn only_a_filing_of_the_same_chain_is_a_rechain_of_the_sent_one() {
        let (inbox, archive, projects) = (Id::new("M1"), Id::new("M2"), Id::new("M3"));
        let moved = Filing::moved(inbox.clone(), archive.clone());
        assert!(
            rechained_from(&moved, &Filing::moved(inbox.clone(), projects.clone())),
            "a move chain out of the same mailbox is a rechain"
        );
        assert!(
            rechained_from(
                &Filing::copied_from(inbox.clone(), archive.clone()),
                &Filing::copied_from(inbox.clone(), projects.clone())
            ),
            "a copy chain out of the same mailbox is a rechain"
        );
        assert!(
            !rechained_from(&moved, &Filing::copied_into(projects.clone())),
            "a copy out of the row is its own request, not a rechain"
        );
        assert!(
            !rechained_from(
                &Filing::copied_from(inbox.clone(), archive.clone()),
                &Filing::moved(inbox.clone(), projects.clone())
            ),
            "a move out of the row is its own request, not a rechain of a copy"
        );
        assert!(
            !rechained_from(
                &Filing::copied_into(archive),
                &Filing::copied_into(projects)
            ),
            "a copy with no source starts no chain to rechain"
        );
    }
}
