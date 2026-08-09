// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `append_message_sync`: the vfunc that puts a message from outside the
//! account into one of its folders.
//!
//! [`crate::transfer`] moves a message the account already holds, which is one
//! `Email/set` and no mail on the wire at all. This is the other arrival, and
//! the expensive one: a `CamelMimeMessage` Camel is holding — the message
//! another account's folder was asked for, the draft the composer just built,
//! the `.eml` the user dropped on the folder — which the server has never seen.
//! RFC 8621 §4.8's `Email/import` over an uploaded blob is how a JMAP account
//! takes one, and [`MailSync::import_message`] is that pair of requests; what
//! this module does is turn the object into the bytes they need and decide what
//! the folder makes of the answer.
//!
//! ## Camel's writer, for [`crate::message`]'s reason turned around
//!
//! The message is serialised through
//! `camel_data_wrapper_write_to_output_stream_sync`, which is Camel's own RFC
//! 5322 emitter reached through the message's `CamelDataWrapper` face. That is
//! the mirror of the decision [`crate::message`] makes about the parse: the
//! object has to agree with every other part of Camel about what the message
//! says, and a provider that wrote headers itself would be a second, disagreeing
//! MIME implementation inside the same process — one whose disagreement is
//! *stored*, because what goes up is what the account holds from then on.
//!
//! A `GMemoryOutputStream` rather than a `CamelStreamMem`: the destination is a
//! buffer either way, and the GIO one is the object Camel's own stream class is
//! a wrapper around. The whole message is in memory twice for the length of the
//! upload — the stream's copy and the request's — which is the same cost
//! [`crate::message`] pays in the other direction.
//!
//! ## The row is the listing's to write
//!
//! Nothing is added to the folder's summary, which is the decision
//! [`crate::transfer`] takes about the folder a message is dragged *into*, made
//! again here and for the same reason: what this side holds is a uid, and a row
//! built from a uid alone would be a message list line with no subject, sender
//! or date until a refresh replaced it. The message appears when the folder is
//! next listed.
//!
//! Nor is the message put in [`crate::cache`], although the bytes are right
//! here and the uid is known. What the cache holds has to be what the *server*
//! holds under that uid, and RFC 8621 §4.8 lets a server repair a message it is
//! given rather than store it verbatim — so an entry written from this side
//! would be one that could disagree with the account forever, and be served in
//! preference to it. One download the first time the message is opened is the
//! cheaper mistake.
//!
//! ## What is not here
//!
//! **`cancellable`**, the same gap [`crate::refresh`] and [`crate::message`]
//! document, for the same reason: [`Client`] takes its [`CancelFlag`] when it is
//! built. An append uploads the whole message, so it is the longest request this
//! provider makes going the other way.
//!
//! ## The one refusal that costs nothing
//!
//! A message larger than RFC 8620 §6.1's `maxSizeUpload` never reaches the
//! wire: [`Client::upload_blob`] compares the length against the session
//! document and answers [`Error::TooLarge`], which arrives here as a
//! [`StoreError::Client`] and is reported in `CAMEL_FOLDER_ERROR` — the
//! message is what could not be used, and the account is not broken for having
//! a limit. Every other way an append fails needs the server to say so; this
//! one is knowable before the upload starts, and an upload is the one request
//! whose body is the whole message.
//!
//! [`Error::TooLarge`]: jmap_client::Error::TooLarge
//! [`Client::upload_blob`]: jmap_client::Client::upload_blob
//!
//! [`MailSync::import_message`]: jmap_mail_sync::MailSync::import_message
//! [`Client`]: jmap_client::Client
//! [`CancelFlag`]: jmap_client::transport::CancelFlag

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID, CamelDataWrapper, CamelFolder, CamelFolderClass, CamelMessageInfo,
    CamelMimeMessage, camel_data_wrapper_write_to_output_stream_sync, camel_folder_error_quark,
    camel_folder_get_full_name, camel_message_info_get_date_received,
};
use gio_sys::{
    GCancellable, GMemoryOutputStream, GOutputStream, g_memory_output_stream_get_data,
    g_memory_output_stream_get_data_size, g_memory_output_stream_new_resizable,
    g_output_stream_flush,
};
use glib_sys::{GError, GFALSE, GTRUE, g_error_new_literal, g_strdup, gboolean, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::Keywords;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};
use crate::message_info::row_keywords;

/// Installs the folder's append vfunc on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.append_message_sync = Some(append_message_sync);
}

/// Uploads one message and files it into this folder's mailbox.
///
/// `TRUE` with `appended_uid` filled in on success, `FALSE` with the error set
/// otherwise — Camel's convention, and what the callers of
/// `camel_folder_append_message_sync` test before they consider the message
/// delivered. The uid is the id the server minted, which is what the folder will
/// list the message under.
unsafe extern "C" fn append_message_sync(
    folder: *mut CamelFolder,
    message: *mut CamelMimeMessage,
    info: *mut CamelMessageInfo,
    appended_uid: *mut *mut gchar,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a live
    // message, an info that is NULL or a live row, and out-parameters that are
    // NULL or writable.
    unsafe {
        guard_bool("append_message_sync", error, || {
            let Some(mailbox) = JmapFolder::borrow(folder).and_then(JmapFolder::mailbox) else {
                return fail(error, &StoreError::NoFolder(name_of(folder)));
            };

            // Before the connection is looked for, because it is the one step
            // that can fail without the account being involved at all: a
            // message Camel cannot write out is not one to blame the server
            // for.
            let Some(source) = serialize(message, error) else {
                return GFALSE;
            };

            // What the folder the message came from knew about it, and the
            // message itself does not say: which flags the user had set on it,
            // and when it arrived. A message nothing is known about is Camel's
            // own case — the argument is nullable — rather than a defence.
            let keywords = match info.is_null() {
                true => Keywords::default(),
                false => row_keywords(info),
            };
            let received_at = received_at(info);

            let Some(store) = parent_store(folder) else {
                return fail(error, &StoreError::Disconnected);
            };
            match store.import_message(mailbox, source, &keywords, received_at) {
                Ok(uid) => {
                    report(appended_uid, &uid);
                    GTRUE
                }
                Err(problem) => fail(error, &problem),
            }
        })
    }
}

/// The message as the octets `Email/import` uploads, or `None` with the error
/// set.
///
/// The failure is Camel's own and is passed through rather than reclassified,
/// exactly as [`crate::message`] passes a parse failure through: a message its
/// own writer will not write out is one this provider has no better account of
/// than the writer does, and reporting it as a service error would blame the
/// account for it.
///
/// The stream is flushed before its buffer is read. `GMemoryOutputStream` does
/// not buffer, but the writer above is free to wrap it — and a message that
/// arrived truncated by however much a filter was still holding would be a
/// silently corrupted message on the server.
///
/// # Safety
///
/// `message` must be NULL or point at a live `CamelMimeMessage`, and `error`
/// must meet [`set_raw_gerror`]'s contract.
unsafe fn serialize(message: *mut CamelMimeMessage, error: *mut *mut GError) -> Option<Vec<u8>> {
    // SAFETY: a fresh resizable memory stream, the message by this function's
    // contract, and two error out-parameters that are locals starting NULL.
    unsafe {
        let stream: *mut GOutputStream = g_memory_output_stream_new_resizable();
        let mut inner: *mut GError = ptr::null_mut();
        let written = camel_data_wrapper_write_to_output_stream_sync(
            message.cast::<CamelDataWrapper>(),
            stream,
            ptr::null_mut(),
            &mut inner,
        );
        let flushed =
            written >= 0 && g_output_stream_flush(stream, ptr::null_mut(), &mut inner) != GFALSE;

        if !flushed {
            g_object_unref(stream.cast());
            if inner.is_null() {
                // A writer that failed without saying why still has to be
                // reported as a failure: Camel logs a critical of its own for a
                // vfunc that answers FALSE with no error set. `INVALID` because
                // the message is what could not be used, and not a service
                // error, because nothing is wrong with the account.
                inner = g_error_new_literal(
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INVALID as i32,
                    c"the message could not be written out".as_ptr(),
                );
            }
            set_raw_gerror(error, inner);
            return None;
        }

        let stream = stream.cast::<GMemoryOutputStream>();
        let data = g_memory_output_stream_get_data(stream).cast::<u8>();
        let len = g_memory_output_stream_get_data_size(stream);
        // Copied out rather than stolen, so that the stream's own free function
        // stays the one that releases the buffer; the upload needs an owned
        // `Vec` either way.
        let source = match data.is_null() {
            true => Vec::new(),
            false => std::slice::from_raw_parts(data, len as usize).to_vec(),
        };
        g_object_unref(stream.cast());
        Some(source)
    }
}

/// When the row says the message arrived, as `Email/import`'s `receivedAt`.
///
/// Zero is not 1970 here: it is the value a `CamelMessageInfo` carries when
/// nothing has dated it, and sending it as a date would file every message
/// Camel knows nothing about at the epoch — sorted to the far end of the folder
/// for good, since RFC 8621 §4.1 makes an `Email` immutable. `None` leaves the
/// date to the server, which is what RFC 8621 §4.8 defines a default for.
///
/// Anything else is passed through, including a negative: a message from before
/// 1970 is a date `UTCDate` can spell, and it is not this layer's place to
/// decide the folder was wrong about it.
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

/// Tells the caller what the message is called in this folder now.
///
/// Camel declares the out-parameter optional and its callers treat a NULL as
/// "the provider could not say", so a uid that cannot be spelled as a C string
/// leaves it unset rather than failing the append: the message is on the server
/// either way, and reporting a failure would have Evolution offer to send it
/// again.
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
    // SAFETY: the contract above; the copy is the caller's to free, which is
    // what Camel documents for this argument.
    unsafe { *out = g_strdup(text.as_ptr()) };
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
