// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `append_message_sync`: the vfunc that adds a message from outside the
//! account to one of its folders.
//!
//! [`crate::transfer`] moves a message that the account already holds. This
//! module uploads a `CamelMimeMessage` that the server does not have, for
//! example a draft or a dropped `.eml` file. [`MailSync::import_message`] sends
//! the upload and the RFC 8621 §4.8 `Email/import`.
//!
//! [`write_message`] serialises the message with the Camel RFC 5322 writer. A
//! second MIME writer could disagree with Camel, and the server stores the
//! result. The writer is in [`jmap_backend_core::mime`] because sending needs
//! the same bytes. [`Unwritable::into_gerror`] takes the error domain.
//!
//! A connected append adds no summary row: a row made from only a uid has no
//! subject, sender or date. The message appears at the next listing.
//!
//! The append does not write to [`crate::cache`]. RFC 8621 §4.8 lets a server
//! repair an imported message, so the local bytes can differ from the stored
//! message.
//!
//! [`Client::upload_blob`] refuses a message larger than `maxSizeUpload`
//! (RFC 8620 §6.1) before the upload. The [`Error::TooLarge`] arrives as a
//! [`StoreError::Client`] in `CAMEL_FOLDER_ERROR`, because the account is not
//! broken.
//!
//! ## Offline
//!
//! With no connection, `queue_append` builds a row right away instead of
//! waiting for a listing that offline has none of: a saved draft or a dropped
//! `.eml` file must show up in the folder, not vanish until the account comes
//! back. `message_info::new_pending_append_row` builds the row from the
//! message's own headers, [`crate::pending_append_cache::PendingAppendCache`]
//! keeps the bytes `Email/import` has not seen yet, and
//! `message_info::set_pending_append` marks the row with the destination
//! mailbox. The reconnect walk that uploads a pending append is a
//! separate, later claim; this one only queues it.
//!
//! [`Error::TooLarge`]: jmap_client::Error::TooLarge
//! [`Client::upload_blob`]: jmap_client::Client::upload_blob
//!
//! [`MailSync::import_message`]: jmap_mail_sync::MailSync::import_message

use std::ffi::{CStr, CString, c_int};

use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID, CamelFolder, CamelFolderClass, CamelFolderSummary,
    CamelMessageInfo, CamelMimeMessage, camel_folder_changed, camel_folder_error_quark,
    camel_folder_get_folder_summary, camel_folder_get_full_name, camel_folder_summary_add,
    camel_folder_summary_get, camel_message_info_clone, camel_message_info_get_date_received,
    camel_message_info_set_folder_flagged, camel_message_info_set_uid,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, g_strdup, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail_bool, set_raw_gerror};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::Keywords;
use jmap_proto::Id;

use crate::changes::Changes;
use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};
use crate::message_info::{
    clear_pending_write, new_pending_append_row, pending_append, row_keywords, set_pending_append,
    set_server_keywords, synthetic_uid,
};
use jmap_backend_core::mime::{Unwritable, write_message};

/// Installs the folder's append vfunc on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that starts with a
/// `CamelFolderClass`. All descendants of `CamelFolder` agree with this.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.append_message_sync = Some(append_message_sync);
}

/// Uploads one message and puts it in the mailbox of this folder.
///
/// Returns `TRUE` and sets `appended_uid` on success, or `FALSE` and sets the
/// error. The uid is the server's new id, which the folder lists the message by.
unsafe extern "C" fn append_message_sync(
    folder: *mut CamelFolder,
    message: *mut CamelMimeMessage,
    info: *mut CamelMessageInfo,
    appended_uid: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a live
    // message, an info that is NULL or a live row, and out-parameters that are
    // NULL or writable.
    unsafe {
        guard_bool("append_message_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it outlives this observation. The observation lets Stop
            // cancel the requests below.
            let _cancel = observe(cancellable);

            let Some(mailbox) = JmapFolder::borrow(folder).and_then(JmapFolder::mailbox) else {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(folder)),
                    StoreError::to_gerror,
                );
            };

            // Write the message before the store is found: this failure is not
            // the fault of the account.
            //
            // SAFETY: the vfunc contract (this block's contract) keeps the
            // message live.
            let source = match write_message(message) {
                Ok(source) => source,
                Err(failure) => return unwritable(error, failure),
            };

            // The source row gives the flags and the arrival date, which the
            // message does not contain. Camel declares `info` nullable.
            let keywords = match info.is_null() {
                true => Keywords::default(),
                false => row_keywords(info),
            };
            let received_at = received_at(info);

            let Some(store) = parent_store(folder) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            // Refuse from the last listing. Otherwise a read-only shared mailbox
            // accepts the upload, and RFC 8621 §4.8 `Email/import` then rejects it.
            if store.may_add_items(mailbox) == Some(false) {
                return fail_bool(
                    error,
                    &StoreError::NoPermission(name_of(folder)),
                    StoreError::to_gerror,
                );
            }

            // The upload takes `source` by value, so a failed attempt gives it
            // back no other way than a clone kept for the offline path below.
            match store.import_message(mailbox, source.clone(), &keywords, received_at) {
                Ok(uid) => {
                    report(appended_uid, &uid);
                    GTRUE
                }
                Err(StoreError::Disconnected) => queue_append(
                    folder,
                    message,
                    &source,
                    info,
                    mailbox.clone(),
                    appended_uid,
                    error,
                ),
                Err(problem) => fail_bool(error, &problem, StoreError::to_gerror),
            }
        })
    }
}

/// Keeps an append that could not reach the server because the account is
/// offline: builds a local row for `message` and stores `source` until a
/// later claim uploads it, instead of failing the call.
///
/// Returns `TRUE` and sets `appended_uid` to the row's synthetic uid, the
/// same contract as a landed append, so Camel and the user see the append as
/// done. Returns `FALSE` only if the queue itself cannot be built (the folder
/// has no summary or no byte store, or the write to disk fails): an append
/// this provider cannot even remember locally must not claim to have
/// succeeded.
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder`, `message` at the live
/// `CamelMimeMessage` whose serialised bytes are `source`, and `info` at NULL
/// or a live row (Camel declares the vfunc's own row argument nullable).
unsafe fn queue_append(
    folder: *mut CamelFolder,
    message: *mut CamelMimeMessage,
    source: &[u8],
    info: *mut CamelMessageInfo,
    mailbox: Id,
    appended_uid: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the contract above.
    let summary = unsafe { camel_folder_get_folder_summary(folder) };
    if summary.is_null() {
        // SAFETY: `error` meets `fail_bool`'s contract by this function's own.
        return unsafe {
            fail_bool(
                error,
                &StoreError::NoFolder(name_of(folder)),
                StoreError::to_gerror,
            )
        };
    }
    // SAFETY: the contract above.
    let cache = unsafe { JmapFolder::borrow(folder) }.and_then(JmapFolder::pending_append_cache);
    let Some(cache) = cache else {
        // SAFETY: as above.
        return unsafe { fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror) };
    };

    let uid = synthetic_uid();
    if !cache.store(&uid, source) {
        // SAFETY: as above.
        return unsafe { fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror) };
    }

    // SAFETY: the contract above.
    let keywords = match info.is_null() {
        true => Keywords::default(),
        false => unsafe { row_keywords(info) },
    };
    // SAFETY: as above.
    let received_at = unsafe { received_at(info) };

    // SAFETY: `summary` and `message` are live by the contract above.
    let row = unsafe {
        Owned::from_raw(new_pending_append_row(
            summary,
            message,
            &uid,
            &keywords,
            received_at,
            mailbox,
        ))
    };
    let Some(row) = row else {
        cache.remove(&uid);
        // SAFETY: as above.
        return unsafe { fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror) };
    };

    // SAFETY: `summary` is live, and `row` is the row this call just built
    // for it. `force_keep_uid` is TRUE: the uid is this row's own synthetic
    // one, not one the summary's own counter would hand out.
    unsafe {
        camel_folder_summary_add(summary, row.as_ptr(), GTRUE);
        // `add` marks every new row as data the server has not seen
        // (`CAMEL_MESSAGE_FOLDER_FLAGGED`). That mark feeds
        // `crate::synchronize`'s keyword-diff walk, which would read this
        // row's synthetic uid as a server id and send it a bogus `Email/set`.
        // Clear it: the reconnect walk for a pending append is a separate,
        // later mechanism, not that one.
        clear_pending_write(row.as_ptr());
    }

    let mut changes = Changes::new();
    changes.add(&uid);
    // SAFETY: a live folder, and a change info this call owns for the call.
    // `report` needs only a writable or NULL out-parameter, which this
    // function's own contract (Camel's vfunc contract, via its one caller)
    // gives it.
    unsafe {
        camel_folder_changed(folder, changes.as_ptr());
        report(appended_uid, &Id::new(uid));
    }
    GTRUE
}

/// Completes an append that [`queue_append`] could not send while offline,
/// for one row of the work list [`crate::synchronize::synchronize_sync`]
/// walks.
///
/// Returns the new uid if the append landed: the caller must replace the row
/// in the folder's summary, under that uid in place of the synthetic one
/// this row used while offline, and report both in the next `changed`
/// signal. The replacement row is already built and added to `summary` by
/// this function; only the old row's removal is left to the caller, the same
/// division [`crate::transfer::replay_filing`] uses for a landed move.
/// Returns `None` if there is nothing to replay: no pending append, or still
/// offline (the same "write it later" rule `replay_filing` uses).
///
/// # Safety
///
/// `folder` must point at a live `JmapFolder` and `summary` at its summary.
pub(crate) unsafe fn replay_append(
    folder: *mut CamelFolder,
    summary: *mut CamelFolderSummary,
    uid: &CStr,
) -> Result<Option<CString>, StoreError> {
    // SAFETY: the contract above. `Owned` releases the row's reference when it
    // drops at the end of this function.
    let Some(info) = (unsafe { Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr())) })
    else {
        return Ok(None);
    };
    // SAFETY: `info` is a live row while this function holds its reference.
    let Some(append) = (unsafe { pending_append(info.as_ptr()) }) else {
        return Ok(None);
    };
    let Ok(text) = uid.to_str() else {
        return Ok(None);
    };
    // SAFETY: the contract above.
    let cache = unsafe { JmapFolder::borrow(folder) }.and_then(JmapFolder::pending_append_cache);
    let Some(cache) = cache else {
        return Err(StoreError::Disconnected);
    };
    let Some(source) = cache.load(text) else {
        // The bytes are gone, which only a damaged cache directory can cause:
        // retrying forever cannot fix a missing file. Give up quietly, the
        // same restraint `push_row` already applies to a message another
        // client destroyed.
        // SAFETY: `info` is still the live row.
        unsafe {
            set_pending_append(info.as_ptr(), None);
            clear_pending_write(info.as_ptr());
        }
        return Ok(None);
    };

    // SAFETY: `info` is still the live row.
    let keywords = unsafe { row_keywords(info.as_ptr()) };
    // SAFETY: as above.
    let recv = unsafe { received_at(info.as_ptr()) };

    let result = match unsafe { parent_store(folder) } {
        Some(store) => store.import_message(append.mailbox(), source, &keywords, recv),
        None => Err(StoreError::Disconnected),
    };
    let new_uid = match result {
        Ok(new_uid) => new_uid,
        Err(StoreError::Disconnected) => {
            // Still offline. Unlike `replay_filing`'s own row, `push_row`
            // never touches a pending-append row's mark (see its own guard),
            // so there is nothing to restore here.
            // SAFETY: `info` is still the live row.
            unsafe { camel_message_info_set_folder_flagged(info.as_ptr(), GTRUE) };
            return Ok(None);
        }
        Err(other) => return Err(other),
    };
    let Ok(new_cstring) = CString::new(new_uid.as_str()) else {
        // Cannot happen for a server-chosen id; defensive only.
        return Ok(None);
    };

    // The server already has the message now. Build its replacement row:
    // `clone` copies every column this provider's own override and Camel's
    // base class keep, so the message keeps the subject, sender and date
    // this provider already extracted when it built the synthetic row.
    //
    // SAFETY: `info` and `summary` are both live.
    let copy = unsafe { Owned::from_raw(camel_message_info_clone(info.as_ptr(), summary)) };
    let Some(copy) = copy else {
        // The upload cannot be retried (the server already has it), so
        // forget the pending append; the next ordinary refresh lists the
        // message at its new uid instead.
        cache.remove(text);
        // SAFETY: `info` is still the live row.
        unsafe {
            set_pending_append(info.as_ptr(), None);
            clear_pending_write(info.as_ptr());
        }
        return Ok(None);
    };
    // SAFETY: `copy` is a row this function owns until `camel_folder_
    // summary_add` below gives the summary its own reference.
    unsafe {
        camel_message_info_set_uid(copy.as_ptr(), new_cstring.as_ptr());
        set_pending_append(copy.as_ptr(), None);
        set_server_keywords(copy.as_ptr(), keywords);
        clear_pending_write(copy.as_ptr());
        camel_folder_summary_add(summary, copy.as_ptr(), GTRUE);
    }
    cache.remove(text);
    Ok(Some(new_cstring))
}

/// Sets the error for a message that the Camel writer cannot write, and returns
/// `FALSE`.
///
/// The Camel error passes through unchanged, as in [`crate::message`]. The
/// domain and code apply only to a failure that has no error. The code is
/// `CAMEL_FOLDER_ERROR_INVALID`: a service error makes Evolution think the
/// account is unusable.
///
/// # Safety
///
/// `error` must meet [`set_raw_gerror`]'s contract.
unsafe fn unwritable(error: *mut *mut GError, failure: Unwritable) -> gboolean {
    // SAFETY: no arguments, and the quark registers itself.
    let quark = unsafe { camel_folder_error_quark() };
    // SAFETY: `into_gerror` gives an owned GError. The contract of this
    // function makes `error` meet the contract of `set_raw_gerror`.
    unsafe {
        set_raw_gerror(
            error,
            failure.into_gerror(quark, CAMEL_FOLDER_ERROR_INVALID as c_int),
        )
    };
    GFALSE
}

/// The arrival date from the row, for the `receivedAt` of `Email/import`.
///
/// Zero means "no date" in a `CamelMessageInfo`, so it gives `None`. Then the
/// server uses its RFC 8621 §4.8 default. An `Email` is immutable (RFC 8621
/// §4.1), so a wrong date at the epoch stays permanently.
///
/// Other values pass through. A negative value is a valid pre-1970 `UTCDate`.
///
/// # Safety
///
/// `info` must be NULL or point at a live `CamelMessageInfo`.
unsafe fn received_at(info: *mut CamelMessageInfo) -> Option<i64> {
    if info.is_null() {
        return None;
    }
    // SAFETY: the contract above.
    match unsafe { camel_message_info_get_date_received(info) } {
        0 => None,
        seconds => Some(seconds),
    }
}

/// Gives the caller the uid of the message in this folder.
///
/// The out-parameter is optional, and a NULL means "unknown". A uid with a NUL
/// byte leaves it unset. The message is on the server, so a failure would make
/// Evolution offer to send it again.
///
/// # Safety
///
/// `out` must be NULL or writable.
unsafe fn report(out: *mut *mut gchar, uid: &Id) {
    if out.is_null() {
        return;
    }
    let Ok(text) = CString::new(uid.as_str()) else {
        return;
    };
    // SAFETY: the contract above. The caller frees the copy, as Camel
    // documents for this argument.
    unsafe { *out = g_strdup(text.as_ptr()) };
}

/// The full name of the folder in Camel, for an error message.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    // SAFETY: the accessor returns a string that the folder owns and that
    // outlives the call. `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}
