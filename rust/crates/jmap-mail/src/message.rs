// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_message_sync`: the vfunc that answers a click on a message list row.
//!
//! [`crate::refresh`] fills a folder with rows, and a row is what Evolution
//! *lists*: a subject, a sender, a size. It is not the message. Opening one asks
//! this vfunc, and what it has to hand back is a `CamelMimeMessage` — the object
//! the preview pane renders, the reply composer quotes and the save-as dialogue
//! writes out.
//!
//! Two steps and each belongs to a different layer. The bytes come from
//! [`MailSync::message_source`], built last increment: an `Email/get` for the
//! blob id, then a download of that blob, which is where the RFC 5322 message a
//! JMAP server holds actually lives. Turning those bytes into an object is
//! Camel's own parser, reached through the message's `CamelDataWrapper` face —
//! and it has to be Camel's, because the object has to agree with every other
//! part of Camel about what the message says. A provider that parsed headers
//! itself would be a second, disagreeing MIME implementation inside the same
//! process.
//!
//! ## One buffer, not a stream
//!
//! `camel_data_wrapper_construct_from_data_sync` rather than a
//! `CamelStreamMem`: the download already produced the whole message in memory,
//! so a stream would be a wrapper around a buffer this code is holding anyway,
//! and it would put three more Camel classes across the FFI boundary for
//! nothing. The cost is that a large message is in memory twice for the length
//! of the parse — the download's copy and the object's — which is the same trade
//! `camel_folder_get_message_sync` callers make anyway, since what they get back
//! *is* the whole message.
//!
//! ## Once, not every time
//!
//! The two steps above are what the *first* open of a message costs. RFC 8621
//! §4.1 makes an `Email` immutable, so every later open would pay the same two
//! round trips for bytes that cannot have changed — and would fail outright with
//! the account offline, in a provider whose store is a `CamelOfflineStore`. So
//! the download is kept: [`crate::cache`], a file per message under the
//! account's own cache directory, consulted here before the connection is even
//! looked for. What is *not* kept is the blob id, for the reason
//! [`MailSync::message_source`] documents — a server may reissue one — which is
//! why the cache is keyed by the uid Camel asked for rather than by anything the
//! fetch produced.
//!
//! ## Stopping one
//!
//! A blob download of a large message is the longest single request this
//! provider makes, and the `cancellable` Camel passes is [`observe`]d for the
//! length of the call. A stopped open caches nothing: the bytes are only written
//! once they are all here.
//!
//! [`MailSync::message_source`]: jmap_mail_sync::MailSync::message_source

use std::ptr;

use eds_sys::{
    CAMEL_FOLDER_ERROR_INVALID, CamelDataWrapper, CamelFolder, CamelFolderClass, CamelMimeMessage,
    camel_data_wrapper_construct_from_data_sync, camel_folder_error_quark,
    camel_folder_get_folder_summary, camel_folder_summary_get, camel_message_info_get_size,
    camel_mime_message_new,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, g_error_new_literal, gchar, gssize};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::guard_ptr;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};

/// Installs the message vfuncs on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_message_sync = Some(get_message_sync);
}

/// Fetches one message and parses it.
///
/// A new reference on success, NULL with the error set on failure — Camel's
/// convention for the object-returning vfuncs, and what
/// `camel_folder_get_message_sync`'s callers test before they render anything.
unsafe extern "C" fn get_message_sync(
    folder: *mut CamelFolder,
    message_uid: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelMimeMessage {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NUL-terminated uid, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_ptr("get_message_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            // The wrapper rejects a NULL uid before it dispatches, so this is
            // the empty string a caller reaching the vfunc directly could pass.
            // Reported as the missing message it describes rather than asserted:
            // a vfunc is not the place to take the process down.
            let Some(uid) = read_string(message_uid) else {
                return fail(error, &StoreError::NoMessage(String::new()));
            };
            let uid = Id::new(uid);

            // Before the connection is even looked for: a message already
            // downloaded is one this provider can hand over with the account
            // offline, which is the whole point of a `CamelOfflineStore`.
            let cache = JmapFolder::borrow(folder).and_then(JmapFolder::cache);
            // And read once, for both ends of the cache: what the row says the
            // message weighs is what tells a complete entry from what a crash
            // left behind — see [`crate::cache`].
            let listed = listed_size(folder, message_uid);
            if let Some(source) = cache.and_then(|cache| cache.load(uid.as_str(), listed)) {
                // Parsed without an error out-parameter, so a failure is silent
                // and falls through to the fetch below: an entry Camel's parser
                // will not read is not a message to report, it is one to
                // replace.
                let message = parse(&source, ptr::null_mut());
                if !message.is_null() {
                    return message;
                }
            }

            let Some(store) = parent_store(folder) else {
                return fail(error, &StoreError::Disconnected);
            };

            let source = match store.message_source(&uid) {
                Ok(source) => source,
                Err(failure) => return fail(error, &failure),
            };
            // Kept before it is parsed, not after: what is worth keeping is the
            // bytes the server sent, and a message this parser rejects is one
            // the next release of Camel may not — while a failed parse that
            // discarded the download would make every open of that message
            // another two round trips.
            if let Some(cache) = cache {
                cache.store(uid.as_str(), &source, listed);
            }
            parse(&source, error)
        })
    }
}

/// How many octets this folder's row for `message_uid` says the message has, or
/// `None` if there is no row to ask.
///
/// The number is the `Email`'s `size` as [`crate::message_info`] wrote it into
/// the row, which RFC 8621 §4.1 defines as the octets of the data the `blobId`
/// references — the same bytes the cache holds. Asked of the summary rather than
/// carried into the vfunc because the vfunc's argument is a uid: what Evolution
/// clicked is a line of the message list, and the row behind that line is where
/// everything already known about the message lives.
///
/// `None` for a uid the summary has no row for, which is every uid a caller
/// invented and the ordinary state of a folder that has not been refreshed. That
/// is not an error here — it is a message the cache will hold unchecked, exactly
/// as it did before there was a check — because the fetch below is what decides
/// whether the uid means anything.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`, and `message_uid` must be a live
/// NUL-terminated string.
unsafe fn listed_size(folder: *mut CamelFolder, message_uid: *const gchar) -> Option<u32> {
    // SAFETY: the contract above; the summary is borrowed from the folder, and
    // `summary_get` hands back a reference this function owns and releases
    // when `info` drops at the end of the scope.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        if summary.is_null() {
            return None;
        }
        let info = Owned::from_raw(camel_folder_summary_get(summary, message_uid))?;
        Some(camel_message_info_get_size(info.as_ptr()))
    }
}

/// The downloaded bytes, as the object Camel renders.
///
/// The failure is Camel's own and is passed through rather than reclassified:
/// a message the parser rejects is a message this provider has no better
/// account of than the parser does, and wrapping it in a service error would
/// report a malformed message as a broken account.
///
/// # Safety
///
/// As [`set_raw_gerror`] for `error`.
unsafe fn parse(source: &[u8], error: *mut *mut GError) -> *mut CamelMimeMessage {
    // SAFETY: a fresh message is a valid `CamelDataWrapper`, `source` is a live
    // buffer of the length given, and the error out-parameter is a local that
    // starts NULL. `message` releases its reference wherever this scope ends,
    // unless `into_raw` hands it to the caller on the success path below.
    unsafe {
        let Some(message) = Owned::from_raw(camel_mime_message_new()) else {
            // Unreachable in practice — `camel_mime_message_new` has no failure
            // mode to report — but a NULL construction is not one this parse
            // succeeded at either, so it gets the same "unexplained" report as
            // a parse the wrapper itself refused with no error set, below.
            set_raw_gerror(
                error,
                g_error_new_literal(
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INVALID as i32,
                    c"the downloaded message could not be parsed".as_ptr(),
                ),
            );
            return ptr::null_mut();
        };
        let mut inner: *mut GError = ptr::null_mut();
        let parsed = camel_data_wrapper_construct_from_data_sync(
            message.as_ptr().cast::<CamelDataWrapper>(),
            source.as_ptr().cast(),
            // A message larger than `gssize` cannot be described to Camel at
            // all; saturating leaves a truncated parse, which fails loudly,
            // rather than a negative length, which Camel reads as "to the end
            // of the buffer" and would walk off it.
            gssize::try_from(source.len()).unwrap_or(gssize::MAX),
            ptr::null_mut(),
            &mut inner,
        );
        if parsed == GFALSE {
            if inner.is_null() {
                // A parser that failed without saying why still has to be
                // reported as a failure: Camel logs a critical of its own for a
                // vfunc that answers NULL with no error set. `INVALID` rather
                // than `INVALID_UID` — the uid was fine, the bytes behind it
                // were not — and not a service error, because nothing is wrong
                // with the account.
                inner = g_error_new_literal(
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INVALID as i32,
                    c"the downloaded message could not be parsed".as_ptr(),
                );
            }
            set_raw_gerror(error, inner);
            return ptr::null_mut();
        }
        message.into_raw()
    }
}

/// Reports a failure and answers with it.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail(error: *mut *mut GError, failure: &StoreError) -> *mut CamelMimeMessage {
    // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe { set_raw_gerror(error, failure.to_gerror()) };
    ptr::null_mut()
}
