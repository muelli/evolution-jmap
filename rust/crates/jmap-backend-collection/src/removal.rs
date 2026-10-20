// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deletion part of a populate: it removes the children that this collection
//! has and no longer needs.
//!
//! If a collection backend only created children, an address book that a user
//! deletes in the web UI of the server stays in the Evolution sidebar. Thus a
//! populate lists the children that it has and removes the children that the
//! discovery did not find. This is the only place in this crate that destroys
//! user data on purpose. `e_source_remove_sync()` removes the uid of the child,
//! its `.source` file and its offline cache of the collection. No undo is
//! possible: if Evolution creates the source again, it is a different source
//! with a different uid.
//!
//! [`Fanout::is_obsolete`] in `jmap-collection-sync` makes the decision. That
//! crate tests it with resource id strings and documents the reasons. The most
//! important reason: a child of a *disabled* part is dormant, not obsolete. Here,
//! this backend is intentionally different from the EDS WebDAV backend. This
//! module contains the two operations that need the headers: it reads the
//! resource id of each child from the `ESource`, and it removes the child.
//!
//! ## A child that this module cannot read is not changed
//!
//! [`obsolete`] calls [`resource_id_of`] first and stops on `None`. This is the
//! safe answer to a question with only two answers. The sources that a populate
//! gets include children of other collection backends, children that a later
//! version of this backend wrote, and files that a user edited. "I cannot read
//! this" must not become "this is obsolete". From the side of the removal, the
//! two are the same, and only one of them is recoverable.
//!
//! ## A failed removal is reported, not raised
//!
//! `ECollectionBackendClass::populate` returns `void`: it has no `GError` to set
//! and no caller to give one to. Thus [`remove_obsolete`] removes what it can and
//! continues after a refusal. It returns one [`NotRemoved`] for each child that
//! it could not remove. A populate can log these. The next populate finds the
//! same children and tries again. This is the only recovery available.
//!
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete

use std::ptr;

use eds_sys::{ESource, e_source_remove_sync};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, g_error_free};
use jmap_backend_core::marshal::read_string;
use jmap_collection_sync::Fanout;

use crate::resource_id::resource_id_of;

/// A child that this populate had to remove and could not remove.
///
/// The only possible action is to log it. The module comment tells why a
/// populate has no error path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRemoved {
    /// The resource id of the child, which this backend and EDS both use.
    pub resource_id: String,
    /// The reason from EDS, for the log line. It is never empty: a refusal
    /// without a `GError` has a message that tells this.
    pub message: String,
}

/// Returns the children of the collection of `fanout` that this populate must
/// remove, in the order of `children`.
///
/// `children` is the result of `e_collection_backend_list_contacts_sources()`
/// and `e_collection_backend_list_calendar_sources()`. It contains each child of
/// this collection, also the children that this backend did not write. This
/// function never selects those.
///
/// # Safety
///
/// Each pointer must be NULL or a valid `ESource` that outlives the call. The
/// function only reads the sources. It returns the same pointers that it gets.
pub unsafe fn obsolete(fanout: &Fanout, children: &[*mut ESource]) -> Vec<*mut ESource> {
    // SAFETY: the contract of the caller is the contract of this function.
    unsafe { named_obsolete(fanout, children) }
        .into_iter()
        .map(|(source, _)| source)
        .collect()
}

/// Removes each child that [`obsolete`] selects, and reports the children that
/// EDS refused to remove.
///
/// A refusal does not stop the next removals, and it is not an error of the
/// populate. The child stays, this populate reports it, and the next populate
/// tries again.
///
/// # Safety
///
/// As [`obsolete`]. Also, this function *removes* each selected source, with the
/// data loss that the module comment describes. Thus `children` must contain only
/// children of the collection of this backend.
pub unsafe fn remove_obsolete(fanout: &Fanout, children: &[*mut ESource]) -> Vec<NotRemoved> {
    // SAFETY: the contract of the caller is the contract of this function.
    let obsolete = unsafe { named_obsolete(fanout, children) };
    tracing::debug!(
        obsolete_count = obsolete.len(),
        total_children_count = children.len(),
        "removing obsolete collection child sources"
    );

    obsolete
        .into_iter()
        .filter_map(|(source, resource_id)| {
            // No cancellable: `populate` gets none, and no caller can cancel it.
            // SAFETY: a valid child source because of the contract above.
            let removed = unsafe { remove_source(source, ptr::null_mut()) };
            removed.err().map(|message| NotRemoved {
                resource_id,
                message,
            })
        })
        .collect()
}

/// Calls `e_source_remove_sync` once and changes a failure into a readable
/// reason.
///
/// This is the only call in this crate that destroys a child. The populate code
/// above removes children that the server no longer has. [`crate::delete_resource`]
/// removes the child that the *user* deletes. EDS documents this removal as an
/// obligation of `delete_resource_sync`, and `ews_backend_delete_resource_sync`
/// also ends with `e_source_remove_sync`. The removal deletes the uid, the
/// `.source` file and the offline cache of the child. If Evolution creates the
/// source again, it is a different source with a different uid. No undo is
/// possible from either caller.
///
/// The `Err` is a message, not a `GError`, because the two callers report it
/// differently. A populate logs it, and the vfunc puts it in its own `GError`.
/// Neither caller can use the domain and code of a second `GError`.
///
/// # Safety
///
/// `source` must be a valid `ESource` that is a child of the collection of this
/// backend. `cancellable` must be NULL or a valid `GCancellable`.
pub unsafe fn remove_source(
    source: *mut ESource,
    cancellable: *mut GCancellable,
) -> Result<(), String> {
    // SAFETY: a valid source because of the contract of this function.
    let resource_id = unsafe { resource_id_of(source) };
    tracing::debug!(
        resource_id = resource_id.as_deref(),
        "removing child source from registry server"
    );

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: the documented arguments are a valid child source, a cancellable
    // that agrees with the contract of this function, and an out-parameter set
    // to NULL.
    let removed = unsafe { e_source_remove_sync(source, cancellable, &mut error) };

    if removed != GFALSE {
        // Only a defective callee sets an error on success. Free the error,
        // because a leak causes a report.
        if !error.is_null() {
            // SAFETY: a GError that this call owns and that nothing else holds.
            unsafe { g_error_free(error) };
        }
        return Ok(());
    }

    // SAFETY: the call failed, so the out-parameter is NULL or a GError that
    // this function now owns.
    Err(unsafe { take_message(error) })
}

/// As [`obsolete`], but it also returns the resource id of each source. The id is
/// read only once. If it is read twice, a source that changes between the reads
/// can be removed under an id that it no longer has.
///
/// # Safety
///
/// As [`obsolete`].
unsafe fn named_obsolete(
    fanout: &Fanout,
    children: &[*mut ESource],
) -> Vec<(*mut ESource, String)> {
    children
        .iter()
        .copied()
        .filter_map(|source| {
            // Do not select a source without a resource id of this backend (see
            // the module comment).
            // SAFETY: a valid or NULL source because of the contract above.
            let resource_id = unsafe { resource_id_of(source) }?;
            fanout
                .is_obsolete(&resource_id)
                .then_some((source, resource_id))
        })
        .collect()
}

/// Returns the message of a `GError` that this call owns, then frees the error.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        // A `FALSE` without a `GError` means that the callee gave no reason. The
        // child is still not removed, and a report without a reason is better
        // than no report.
        return "the removal failed and EDS set no error".to_owned();
    }

    // SAFETY: a live GError. Its message is a NUL-terminated string that it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: this function got ownership through the out-parameter.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "the removal failed and EDS gave no message".to_owned())
}
