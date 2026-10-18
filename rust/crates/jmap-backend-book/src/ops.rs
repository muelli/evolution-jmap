// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implementation bodies for `EBookMetaBackend` vfuncs operating over [`BookSync`].
//! Out-parameters are written and transferred to EDS on success; errors are set on failure.

use eds_sys::{E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, EContact, e_book_client_error_create};
use glib_sys::{GError, GFALSE, GSList, GTRUE, gboolean, gchar};
use jmap_backend_core::error::{
    cstring_lossy, fail_bool, fail_invalid, invalid_arg_gerror, set_raw_gerror,
};
use jmap_backend_core::i18n::translate_with;
use jmap_backend_core::marshal::{dup_string, read_string, set_out_string};
use jmap_book_sync::{BookSync, SyncError};
use jmap_proto::State;

use crate::marshal;

/// Outcome of a `get_changes` operation.
#[derive(Debug)]
pub enum Outcome {
    /// Changes were calculated and out-parameters populated.
    Reported,
    /// Changes cannot be calculated; caller chains up to full cache diffing.
    ListInstead,
    /// Operation failed; GError set.
    Failed,
}

/// Lists all existing contacts in the address book for `list_existing_sync`.
///
/// # Safety
///
/// Out-parameters and `error` must be valid pointers per EDS vfunc contracts.
pub unsafe fn list_existing(
    sync: &BookSync,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let address_book_id = sync.address_book_id().as_str();
    tracing::debug!(
        account_id,
        address_book_id,
        "listing existing address book contacts"
    );
    let (state, contacts) = match sync.list_existing() {
        Ok(listed) => listed,
        // SAFETY: `error` satisfies set_raw_gerror contract.
        Err(failure) => {
            tracing::debug!(
                account_id,
                address_book_id,
                ?failure,
                "listing existing address book contacts failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    tracing::debug!(
        account_id,
        address_book_id,
        state = state.as_str(),
        count = contacts.len(),
        "listed existing address book contacts"
    );

    // SAFETY: Out-parameters satisfy caller contracts. Both allocations are GLib ones transferring ownership to caller.
    // Build outputs into locals first and publish in a single step.
    unsafe {
        let existing_objects =
            (!out_existing_objects.is_null()).then(|| marshal::info_list(&contacts));
        let new_sync_tag = (!out_new_sync_tag.is_null()).then(|| dup_string(state.as_str()));

        if let Some(list) = existing_objects {
            *out_existing_objects = list;
        }
        if let Some(tag) = new_sync_tag {
            *out_new_sync_tag = tag;
        }
    }
    GTRUE
}

/// Computes contact changes since `last_sync_tag` for `get_changes_sync`.
///
/// Returns [`Outcome::ListInstead`] if `last_sync_tag` is absent or the server
/// returns `cannotCalculateChanges` (RFC 8620 §5.2).
///
/// # Safety
///
/// As [`list_existing`], and `last_sync_tag` must be NULL or a valid NUL-terminated string.
#[allow(clippy::too_many_arguments)] // Matches the EDS vfunc signature.
pub unsafe fn get_changes(
    sync: &BookSync,
    last_sync_tag: *const gchar,
    out_new_sync_tag: *mut *mut gchar,
    out_repeat: *mut gboolean,
    _out_created_objects: *mut *mut GSList,
    out_modified_objects: *mut *mut GSList,
    out_removed_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> Outcome {
    let account_id = sync.account_id().as_str();
    let address_book_id = sync.address_book_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(tag) = (unsafe { read_string(last_sync_tag) }) else {
        tracing::debug!(
            account_id,
            address_book_id,
            "get_changes called with no last_sync_tag; listing address book instead"
        );
        return Outcome::ListInstead;
    };

    tracing::debug!(
        account_id,
        address_book_id,
        last_sync_tag = tag.as_str(),
        "getting address book changes"
    );

    let changes = match sync.get_changes(&State::from(tag)) {
        Ok(changes) => changes,
        Err(failure) if failure.is_cannot_calculate_changes() => {
            tracing::debug!(
                account_id,
                address_book_id,
                "server cannot calculate changes; listing address book instead"
            );
            return Outcome::ListInstead;
        }
        Err(failure) => {
            tracing::debug!(
                account_id,
                address_book_id,
                ?failure,
                "getting address book changes failed"
            );
            // SAFETY: `error` satisfies caller contract.
            unsafe { set_raw_gerror(error, to_gerror(&failure)) };
            return Outcome::Failed;
        }
    };

    tracing::debug!(
        account_id,
        address_book_id,
        new_sync_tag = changes.new_state.as_str(),
        changed_count = changes.changed.len(),
        removed_count = changes.removed.len(),
        "reported address book changes"
    );

    // SAFETY: Out-parameters satisfy caller contracts. Allocations transfer ownership to caller.
    // Build outputs into locals first and publish in a single step.
    unsafe {
        let modified =
            (!out_modified_objects.is_null()).then(|| marshal::info_list(&changes.changed));
        let removed =
            (!out_removed_objects.is_null()).then(|| marshal::removed_info_list(&changes.removed));
        let new_sync_tag =
            (!out_new_sync_tag.is_null()).then(|| dup_string(changes.new_state.as_str()));

        if let Some(list) = modified {
            *out_modified_objects = list;
        }
        if let Some(list) = removed {
            *out_removed_objects = list;
        }
        if let Some(tag) = new_sync_tag {
            *out_new_sync_tag = tag;
        }
        // Paging occurs inside BookSync::get_changes; signal completion to EDS.
        if !out_repeat.is_null() {
            *out_repeat = GFALSE;
        }
    }
    Outcome::Reported
}

/// Loads a contact by UID for `load_contact_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated string.
pub unsafe fn load_contact(
    sync: &BookSync,
    uid: *const gchar,
    out_contact: *mut *mut EContact,
    _out_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let address_book_id = sync.address_book_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        tracing::debug!(
            account_id,
            address_book_id,
            "contact was asked for without an identifier"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a contact was asked for without an identifier") };
    };

    tracing::debug!(
        account_id,
        address_book_id,
        uid = uid.as_str(),
        "loading contact"
    );

    let info = match sync.load_contact(&uid) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                address_book_id,
                uid = uid.as_str(),
                ?failure,
                "loading contact failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    let contact = marshal::contact_from_vcard(&info.vcard);
    if contact.is_null() {
        tracing::debug!(
            account_id,
            address_book_id,
            uid = uid.as_str(),
            "contact could not be rendered as a vCard"
        );
        // SAFETY: as above.
        return unsafe {
            fail_invalid(
                error,
                &format!("the contact {uid} could not be rendered as a vCard"),
            )
        };
    }

    tracing::debug!(
        account_id,
        address_book_id,
        uid = uid.as_str(),
        revision = info.revision.as_str(),
        "loaded contact"
    );

    // SAFETY: `out_contact` satisfies caller contract. Reference from contact_from_vcard transfers to caller or is dropped.
    unsafe {
        if out_contact.is_null() {
            marshal::contact_unref(contact);
        } else {
            *out_contact = contact;
        }
    }
    GTRUE
}

/// Creates or updates a contact for `save_contact_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `contact` must be NULL or a valid `EContact`.
pub unsafe fn save_contact(
    sync: &BookSync,
    overwrite_existing: gboolean,
    contact: *mut EContact,
    out_new_uid: *mut *mut gchar,
    _out_new_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let address_book_id = sync.address_book_id().as_str();
    let is_overwrite = overwrite_existing != GFALSE;

    // SAFETY: the caller guarantees a valid EContact or NULL.
    let Some(vcard) = (unsafe { marshal::vcard_from_contact(contact) }) else {
        tracing::debug!(
            account_id,
            address_book_id,
            "contact to save is not a vCard"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "the contact to save is not a vCard") };
    };

    let existing_uid = if !is_overwrite {
        // A create. Local UID is ignored; the server assigns the identifier.
        None
    } else {
        // SAFETY: as above.
        match unsafe { marshal::contact_uid(contact) } {
            Some(uid) => Some(uid),
            // Editing without a UID would duplicate rather than update; report invalid argument.
            // SAFETY: as above.
            None => {
                tracing::debug!(
                    account_id,
                    address_book_id,
                    "contact was edited without an identifier"
                );
                return unsafe {
                    fail_invalid(error, "a contact was edited without an identifier")
                };
            }
        }
    };

    tracing::debug!(
        account_id,
        address_book_id,
        overwrite_existing = is_overwrite,
        existing_uid = existing_uid.as_deref(),
        "saving contact"
    );

    let info = match sync.save_contact(&vcard, existing_uid.as_deref()) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                address_book_id,
                overwrite_existing = is_overwrite,
                existing_uid = existing_uid.as_deref(),
                ?failure,
                "saving contact failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    tracing::debug!(
        account_id,
        address_book_id,
        uid = info.uid.as_str(),
        revision = info.revision.as_str(),
        "saved contact"
    );

    // SAFETY: `out_new_uid` satisfies the contract by this function's own, and
    // ownership of the duplicate passes through it.
    unsafe { set_out_string(out_new_uid, &info.uid) };
    GTRUE
}

/// Destroys a contact for `remove_contact_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated string.
pub unsafe fn remove_contact(
    sync: &BookSync,
    uid: *const gchar,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let address_book_id = sync.address_book_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        tracing::debug!(
            account_id,
            address_book_id,
            "contact was removed without an identifier"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a contact was removed without an identifier") };
    };

    tracing::debug!(
        account_id,
        address_book_id,
        uid = uid.as_str(),
        "removing contact"
    );

    match sync.remove_contact(&uid) {
        Ok(()) => {
            tracing::debug!(
                account_id,
                address_book_id,
                uid = uid.as_str(),
                "removed contact"
            );
            GTRUE
        }
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                address_book_id,
                uid = uid.as_str(),
                ?failure,
                "removing contact failed"
            );
            unsafe { fail_bool(error, &failure, to_gerror) }
        }
    }
}

/// Allocates a `GError` for `failure`, mapping `NotFound` to `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND`.
pub fn to_gerror(failure: &SyncError) -> *mut GError {
    match failure {
        SyncError::Client(error) => jmap_backend_core::error::to_gerror(error),
        SyncError::NotFound(_) => {
            let message = cstring_lossy(&failure.to_string());
            // SAFETY: the code is one of the enum's own values and the message
            // is copied by the call.
            unsafe {
                e_book_client_error_create(E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, message.as_ptr())
            }
        }
        // Unreadable vCard represents an invalid argument.
        SyncError::VCard(_) => invalid_arg_gerror(&failure.to_string()),
        SyncError::PhotoTooLarge { photo_octets } => invalid_arg_gerror(&translate_with(
            // TRANSLATORS: shown when a contact could not be saved because of its picture. %1$s is the picture's size, such as "1.2 MB".
            c"The server cannot store a contact with a picture this large (%1$s), so the contact was not saved. Choose a smaller picture and save it again.",
            &[format_size(*photo_octets).as_str()],
        )),
    }
}

/// `octets` as the user's desktop states a file size ("1.2 MB"), localised.
fn format_size(octets: u64) -> String {
    // SAFETY: g_format_size takes any value and returns a new, NUL-terminated
    // string that the caller frees with g_free; it is copied out first.
    unsafe {
        let text = glib_sys::g_format_size(octets);
        let size = std::ffi::CStr::from_ptr(text)
            .to_string_lossy()
            .into_owned();
        glib_sys::g_free(text.cast());
        size
    }
}
