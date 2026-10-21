// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_message_sync`: the vfunc that opens a message from a message list row.
//!
//! A row from [`crate::refresh`] holds only list data. This vfunc returns the
//! full `CamelMimeMessage`. [`MailSync::message_source`] gets the RFC 5322 bytes
//! (an `Email/get` for the blob id, then a blob download). The Camel parser
//! makes the object, so all parts of Camel agree about the message.
//!
//! `camel_data_wrapper_construct_from_data_sync` parses the downloaded buffer
//! directly. A `CamelStreamMem` adds more FFI and no value. The message is in
//! memory twice during the parse.
//!
//! An `Email` is immutable (RFC 8621 §4.1), so [`crate::cache`] keeps the
//! download, one file for each message. The vfunc reads the cache before it
//! connects, so a cached message opens offline. The cache key is the uid,
//! because a server can change the blob id (see [`MailSync::message_source`]).
//!
//! The `cancellable` is [`observe`]d for the full call. A cancelled open writes
//! nothing to the cache, because the cache write occurs after the full download.
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
use jmap_backend_core::error::{fail, set_raw_gerror};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::{guard, guard_ptr};
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};

/// Installs the message vfuncs on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that starts with a
/// `CamelFolderClass`. All descendants of `CamelFolder` agree with this.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_message_sync = Some(get_message_sync);
    vfuncs.get_message_cached = Some(get_message_cached);
}

/// Fetches one message and parses it.
///
/// Returns a new reference on success, or NULL and sets the error on failure.
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
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it outlives this observation. The observation lets Stop
            // cancel the requests below.
            let _cancel = observe(cancellable);

            // The Camel wrapper rejects a NULL uid, so this branch catches an
            // empty uid. Report a missing message: a vfunc must not abort the
            // process.
            let Some(uid) = read_string(message_uid) else {
                return fail(
                    error,
                    &StoreError::NoMessage(String::new()),
                    StoreError::to_gerror,
                );
            };
            let uid = Id::new(uid);

            // Read the cache before the connection, so a cached message opens
            // offline. This is the same lookup as `get_message_cached`, for
            // callers that call this vfunc directly.
            let message = cached_message(folder, message_uid);
            if !message.is_null() {
                return message;
            }

            let Some(store) = parent_store(folder) else {
                return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            let source = match store.message_source(&uid) {
                Ok(source) => source,
                Err(failure) => return fail(error, &failure, StoreError::to_gerror),
            };
            // Cache the bytes before the parse. A later Camel release can parse
            // a message that this release rejects, and no second download is
            // necessary.
            if let Some(cache) = JmapFolder::borrow(folder).and_then(JmapFolder::cache) {
                cache.store(uid.as_str(), &source, listed_size(folder, message_uid));
            }
            parse(&source, error)
        })
    }
}

/// Returns a cached message without `camel_folder_lock`.
///
/// `camel_folder_get_message_sync` calls this vfunc first, and calls the locked
/// `get_message_sync` only on a miss. This vfunc makes no request, so it does
/// not observe the `cancellable`.
///
/// NULL means a miss, which is not an error. The slot has no `GError`
/// out-parameter.
unsafe extern "C" fn get_message_cached(
    folder: *mut CamelFolder,
    message_uid: *const gchar,
    _cancellable: *mut GCancellable,
) -> *mut CamelMimeMessage {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours and a
    // NUL-terminated uid.
    unsafe {
        guard("get_message_cached", ptr::null_mut(), || {
            cached_message(folder, message_uid)
        })
    }
}

/// The cached copy of `message_uid` in `folder`, or NULL for a miss.
///
/// A folder with no cache and an entry that fails the checks of
/// [`MessageCache::load`] are also misses. [`get_message_cached`] and
/// [`get_message_sync`] both use this function.
///
/// [`MessageCache::load`]: crate::cache::MessageCache::load
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`, and `message_uid` at a live
/// NUL-terminated string.
unsafe fn cached_message(
    folder: *mut CamelFolder,
    message_uid: *const gchar,
) -> *mut CamelMimeMessage {
    // SAFETY: the contract above.
    unsafe {
        let Some(uid) = read_string(message_uid) else {
            return ptr::null_mut();
        };
        let cache = JmapFolder::borrow(folder).and_then(JmapFolder::cache);
        // The size in the row identifies a truncated entry (see [`crate::cache`]).
        let listed = listed_size(folder, message_uid);
        let Some(source) = cache.and_then(|cache| cache.load(uid.as_str(), listed)) else {
            return ptr::null_mut();
        };
        // A parse failure here is silent and is a miss. The download in
        // `get_message_sync` then replaces the entry.
        parse(&source, ptr::null_mut())
    }
}

/// The size in octets in the summary row for `message_uid`, or `None` if there
/// is no row.
///
/// [`crate::message_info`] writes the `Email` `size` into the row. RFC 8621 §4.1
/// defines it as the size of the `blobId` data, which the cache holds.
///
/// A folder that has no refresh yet has no rows. `None` is not an error: the
/// cache then stores and loads the entry without a size check.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`, and `message_uid` must be a live
/// NUL-terminated string.
unsafe fn listed_size(folder: *mut CamelFolder, message_uid: *const gchar) -> Option<u32> {
    // SAFETY: the contract above. The summary is borrowed from the folder.
    // `summary_get` returns a reference that this function owns, and `info`
    // releases it at the end of the scope.
    unsafe {
        let summary = camel_folder_get_folder_summary(folder);
        if summary.is_null() {
            return None;
        }
        let info = Owned::from_raw(camel_folder_summary_get(summary, message_uid))?;
        Some(camel_message_info_get_size(info.as_ptr()))
    }
}

/// Parses the downloaded bytes into a `CamelMimeMessage`.
///
/// The Camel parse error passes through unchanged. A service error would report
/// a malformed message as a broken account.
///
/// # Safety
///
/// As [`set_raw_gerror`] for `error`.
unsafe fn parse(source: &[u8], error: *mut *mut GError) -> *mut CamelMimeMessage {
    // SAFETY: a new message is a valid `CamelDataWrapper`, `source` is a live
    // buffer of the given length, and the error out-parameter is a local that
    // starts NULL. `message` releases its reference at the end of the scope,
    // unless `into_raw` gives it to the caller on success.
    unsafe {
        let Some(message) = Owned::from_raw(camel_mime_message_new()) else {
            // `camel_mime_message_new` does not fail in practice. A NULL gets the
            // same report as a parse failure with no error.
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
            // A Rust slice is at most `isize::MAX` bytes, so this saturation
            // does not occur. A negative length means "NUL-terminated" to GIO,
            // and a read past the buffer end.
            gssize::try_from(source.len()).unwrap_or(gssize::MAX),
            ptr::null_mut(),
            &mut inner,
        );
        if parsed == GFALSE {
            if inner.is_null() {
                // Camel logs a critical for a vfunc that returns NULL with no
                // error, so make one. The code is `INVALID`, not `INVALID_UID`,
                // because the bytes are bad, not the uid.
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
