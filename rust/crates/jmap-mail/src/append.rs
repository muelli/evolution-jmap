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
//! The append adds no summary row: a row made from only a uid has no subject,
//! sender or date. The message appears at the next listing.
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
//! [`Error::TooLarge`]: jmap_client::Error::TooLarge
//! [`Client::upload_blob`]: jmap_client::Client::upload_blob
//!
//! [`MailSync::import_message`]: jmap_mail_sync::MailSync::import_message

use std::ffi::{CString, c_int};

use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID, CamelFolder, CamelFolderClass, CamelMessageInfo, CamelMimeMessage,
    camel_folder_error_quark, camel_folder_get_full_name, camel_message_info_get_date_received,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, g_strdup, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail_bool, set_raw_gerror};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::Keywords;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};
use crate::message_info::row_keywords;
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

            match store.import_message(mailbox, source, &keywords, received_at) {
                Ok(uid) => {
                    report(appended_uid, &uid);
                    GTRUE
                }
                Err(problem) => fail_bool(error, &problem, StoreError::to_gerror),
            }
        })
    }
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
