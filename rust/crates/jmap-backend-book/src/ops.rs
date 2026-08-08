// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The bodies of the `EBookMetaBackend` sync vfuncs.
//!
//! Each function here has the shape of the vfunc it implements — the same
//! out-parameters, the same "FALSE means `error` is set" contract — but takes
//! a `&BookSync` instead of an `EBookMetaBackend *`. The subclass on top is
//! then a panic guard, a look at the connection slot, and a call to one of
//! these; and, more to the point, all of this is testable, because
//! constructing a real `EBookMetaBackend` needs an `ESourceRegistry` and so a
//! running `evolution-source-registry` on the session bus.
//!
//! The out-parameter discipline is the same throughout, because it is what
//! EDS relies on:
//!
//! - on success every out-parameter the caller asked for is written, and
//!   ownership of what is written passes to EDS;
//! - on failure nothing is written and `error` is set instead — a vfunc that
//!   returns FALSE having already filled an out-parameter leaks it, since EDS
//!   only frees the outputs of a call that succeeded;
//! - a NULL out-parameter means "not interested" and is skipped, which is why
//!   the lists are not even built when nobody wants them.

use eds_sys::{
    E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG, EContact,
    e_book_client_error_create, e_client_error_create,
};
use glib_sys::{GError, GFALSE, GSList, GTRUE, gboolean, gchar};
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::marshal::{read_string, set_out_list, set_out_string};
use jmap_book_sync::{BookSync, SyncError};
use jmap_proto::State;

use crate::marshal;

/// What `get_changes_sync` decided, which is one answer more than a `gboolean`
/// can carry.
#[derive(Debug)]
pub enum Outcome {
    /// The out-parameters are filled in; the vfunc returns TRUE.
    Reported,
    /// This delta cannot be computed, and that is not a failure: the caller
    /// chains up to `EBookMetaBackend`'s own `get_changes_sync`, which lists
    /// the book in full and diffs it against the offline cache. Nothing has
    /// been written and no error has been set.
    ListInstead,
    /// `error` is set; the vfunc returns FALSE.
    Failed,
}

/// Every card in the address book — `list_existing_sync`.
///
/// # Safety
///
/// The out-parameters must each be NULL or point at a writable, currently-NULL
/// location of the matching type, and `error` must be NULL or a valid,
/// currently-NULL `GError **`. That is what an EDS vfunc receives.
pub unsafe fn list_existing(
    sync: &BookSync,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> gboolean {
    let (state, contacts) = match sync.list_existing() {
        Ok(listed) => listed,
        // SAFETY: `error` satisfies set_raw_gerror's contract by this
        // function's own.
        Err(failure) => return unsafe { fail(error, &failure) },
    };

    // SAFETY: as above for the out-parameters; both allocations are GLib ones
    // ownership of which passes to the caller.
    unsafe {
        set_out_string(out_new_sync_tag, state.as_str());
        set_out_list(out_existing_objects, || marshal::info_list(&contacts));
    }
    GTRUE
}

/// What changed since `last_sync_tag` — `get_changes_sync`.
///
/// Two of the three answers are "list the whole book instead": no tag at all,
/// which is the first sync, and a tag the server will not diff from (RFC 8620
/// §5.2, `cannotCalculateChanges`). Reporting either as a failure would leave
/// the address book empty until someone deleted the cache by hand.
///
/// Everything that changed is reported as **modified** rather than split
/// across `out_created_objects` and `out_modified_objects`. JMAP does draw
/// that distinction, but `BookSync::get_changes` has already spent it on a
/// question only it can answer — a card that shows up as *updated* and is no
/// longer filed in this address book has been moved out and must be reported
/// gone, whereas a *created* one that is not ours never was our business — and
/// what remains is a set of cards that exist and are ours. EDS runs both lists
/// through the same loader, so the split is presentational; inventing one
/// would be a guess dressed up as information.
///
/// # Safety
///
/// As [`list_existing`], and `last_sync_tag` must be NULL or a valid
/// NUL-terminated string.
#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
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
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(tag) = (unsafe { read_string(last_sync_tag) }) else {
        return Outcome::ListInstead;
    };

    let changes = match sync.get_changes(&State::from(tag)) {
        Ok(changes) => changes,
        Err(failure) if failure.is_cannot_calculate_changes() => return Outcome::ListInstead,
        Err(failure) => {
            // SAFETY: `error` satisfies the contract by this function's own.
            unsafe { set_raw_gerror(error, to_gerror(&failure)) };
            return Outcome::Failed;
        }
    };

    // SAFETY: as above for the out-parameters; the allocations are GLib ones
    // ownership of which passes to the caller.
    unsafe {
        set_out_string(out_new_sync_tag, changes.new_state.as_str());
        // The paging happens inside `BookSync::get_changes`, so there is never
        // anything left for EDS to ask again for.
        if !out_repeat.is_null() {
            *out_repeat = GFALSE;
        }
        set_out_list(out_modified_objects, || {
            marshal::info_list(&changes.changed)
        });
        set_out_list(out_removed_objects, || marshal::uid_list(&changes.removed));
    }
    Outcome::Reported
}

/// One card by identifier — `load_contact_sync`.
///
/// `out_extra` is deliberately left alone. It is per-object opaque state a
/// backend may park in the EDS cache, and this one has none: the JMAP id *is*
/// the uid and the revision already carries the change token, which is why
/// [`marshal::info_list`] reports a NULL extra for the same reason.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated
/// string.
pub unsafe fn load_contact(
    sync: &BookSync,
    uid: *const gchar,
    out_contact: *mut *mut EContact,
    _out_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a contact was asked for without an identifier") };
    };

    let info = match sync.load_contact(&uid) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => return unsafe { fail(error, &failure) },
    };

    let contact = marshal::contact_from_vcard(&info.vcard);
    if contact.is_null() {
        // Our own rendering of the card is not a vCard, which is a bug here
        // rather than anything the server did — but it still has to reach EDS
        // as a failure rather than as an empty contact.
        // SAFETY: as above.
        return unsafe {
            fail_invalid(
                error,
                &format!("the contact {uid} could not be rendered as a vCard"),
            )
        };
    }

    // SAFETY: `out_contact` satisfies the contract by this function's own; the
    // reference taken by `contact_from_vcard` passes to the caller, or is
    // dropped again if the caller did not want it.
    unsafe {
        if out_contact.is_null() {
            marshal::contact_unref(contact);
        } else {
            *out_contact = contact;
        }
    }
    GTRUE
}

/// Store a card — `save_contact_sync`.
///
/// `out_new_extra` is left alone, for the reason given on [`load_contact`].
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
    // SAFETY: the caller guarantees a valid EContact or NULL.
    let Some(vcard) = (unsafe { marshal::vcard_from_contact(contact) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "the contact to save is not a vCard") };
    };

    let existing_uid = if overwrite_existing == GFALSE {
        // A create. The vCard's UID is a name Evolution invented locally
        // (`pas-id-…`) and never a JMAP id, so the server assigns the real one.
        None
    } else {
        // SAFETY: as above.
        match unsafe { marshal::contact_uid(contact) } {
            Some(uid) => Some(uid),
            // Sending this as a create would silently duplicate the user's
            // contact on the server, which is worse than a visible failure.
            // SAFETY: as above.
            None => {
                return unsafe {
                    fail_invalid(error, "a contact was edited without an identifier")
                };
            }
        }
    };

    let info = match sync.save_contact(&vcard, existing_uid.as_deref()) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => return unsafe { fail(error, &failure) },
    };

    // SAFETY: `out_new_uid` satisfies the contract by this function's own, and
    // ownership of the duplicate passes through it.
    unsafe { set_out_string(out_new_uid, &info.uid) };
    GTRUE
}

/// Destroy a card — `remove_contact_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated
/// string.
pub unsafe fn remove_contact(
    sync: &BookSync,
    uid: *const gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a contact was removed without an identifier") };
    };

    match sync.remove_contact(&uid) {
        Ok(()) => GTRUE,
        // SAFETY: as above.
        Err(failure) => unsafe { fail(error, &failure) },
    }
}

/// Allocates a `GError` describing `failure`. Ownership passes to the caller,
/// as with [`jmap_backend_core::error::to_gerror`], which handles the
/// [`SyncError::Client`] half.
///
/// The other half is where the address book differs from every other EDS
/// client: a card that is not there has to be reported in the
/// `E_BOOK_CLIENT_ERROR` domain, because `EBookMetaBackend` matches on exactly
/// that domain and code to decide that a card is gone rather than that the
/// sync failed. Any other code and the cache entry never goes away.
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
        // A vCard Evolution handed us that the mapping cannot read: the
        // argument was bad, not the server.
        SyncError::VCard(_) => invalid_arg(&failure.to_string()),
    }
}

fn invalid_arg(message: &str) -> *mut GError {
    let message = cstring_lossy(message);
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call.
    unsafe { e_client_error_create(E_CLIENT_ERROR_INVALID_ARG, message.as_ptr()) }
}

/// Reports `failure` through `error` and returns the vfunc's FALSE.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail(error: *mut *mut GError, failure: &SyncError) -> gboolean {
    unsafe { set_raw_gerror(error, to_gerror(failure)) };
    GFALSE
}

/// The same, for the arguments EDS itself got wrong.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_invalid(error: *mut *mut GError, message: &str) -> gboolean {
    unsafe { set_raw_gerror(error, invalid_arg(message)) };
    GFALSE
}
