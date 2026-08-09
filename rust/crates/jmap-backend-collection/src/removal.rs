// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The half of a populate that deletes: the children this collection already
//! has and no longer warrants.
//!
//! A collection backend that only ever created children would leave an address
//! book deleted in the server's web UI in Evolution's sidebar forever, so a
//! populate has to close the loop — list the children it already has, and
//! remove the ones this discovery did not find. That makes it the one place in
//! this crate that destroys user data on purpose: `e_source_remove_sync()`
//! takes the child's uid, its `.source` file and its offline cache of the
//! collection with it, and there is no undo, because the source Evolution would
//! recreate afterwards is a different source with a different uid.
//!
//! The decision itself is [`Fanout::is_obsolete`]'s, in `jmap-collection-sync`,
//! where it is tested against resource id strings and where the reasoning is
//! written down — most of all why a child of a *switched-off* part is dormant
//! rather than obsolete, which is where this backend deliberately parts company
//! with EDS's WebDAV one. What is here is the two things that need the headers:
//! reading each child's resource id back off the `ESource`, and the removal.
//!
//! ## A child we cannot read is a child we do not touch
//!
//! [`obsolete`] asks [`resource_id_of`] first and gives up on a `None`. That is
//! not defensive coding, it is the safe direction of a question that has no
//! third answer: the sources a populate is handed include children of other
//! collection backends, children written by a future version of this one, and
//! anything a user hand-edited. "I cannot read this" must not become "this is
//! obsolete", because the two are indistinguishable from the removal's side and
//! only one of them is recoverable.
//!
//! ## A failed removal is reported, not raised
//!
//! `ECollectionBackendClass::populate` returns `void`: there is no `GError` to
//! fill and nobody to hand one to. So [`remove_obsolete`] removes what it can,
//! keeps going past a refusal, and hands back one [`NotRemoved`] per child it
//! could not remove — a populate can log those, and the next populate will find
//! the same children and try again, which is the whole of the recovery
//! available.
//!
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete

use std::ptr;

use eds_sys::{ESource, e_source_remove_sync};
use glib_sys::{GError, GFALSE, g_error_free};
use jmap_backend_core::marshal::read_string;
use jmap_collection_sync::Fanout;

use crate::resource_id::resource_id_of;

/// A child this populate should have removed and could not.
///
/// There is nothing to be done with one but log it: see the module comment on
/// why a populate has no error path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRemoved {
    /// The child, named the way this backend and EDS both know it.
    pub resource_id: String,
    /// What EDS said, for the log line. Never empty: a refusal that carried no
    /// `GError` is reported as such rather than as a blank reason.
    pub message: String,
}

/// The children of `fanout`'s collection that this populate must remove, in the
/// order they were given in.
///
/// `children` is what `e_collection_backend_list_contacts_sources()` and
/// `e_collection_backend_list_calendar_sources()` answered — every child this
/// collection has, including the ones this backend did not write, which are
/// never picked.
///
/// # Safety
///
/// Each pointer must be NULL or a valid `ESource` that outlives the call. The
/// sources are only read from; the pointers handed back are the ones handed in.
pub unsafe fn obsolete(fanout: &Fanout, children: &[*mut ESource]) -> Vec<*mut ESource> {
    // SAFETY: the caller's contract is this function's.
    unsafe { named_obsolete(fanout, children) }
        .into_iter()
        .map(|(source, _)| source)
        .collect()
}

/// Removes every child [`obsolete`] picks, and reports the ones EDS refused.
///
/// A refusal does not stop the removals after it, and it is not an error of the
/// populate: the child stays, this populate says so, and the next one asks
/// again.
///
/// # Safety
///
/// As [`obsolete`] — and every source picked is *removed*, which is the
/// destruction this module's comment describes. A pointer to a source that is
/// not a child of this backend's collection has no business in `children`.
pub unsafe fn remove_obsolete(fanout: &Fanout, children: &[*mut ESource]) -> Vec<NotRemoved> {
    // SAFETY: the caller's contract is this function's.
    let obsolete = unsafe { named_obsolete(fanout, children) };

    obsolete
        .into_iter()
        .filter_map(|(source, resource_id)| {
            let mut error: *mut GError = ptr::null_mut();
            // No cancellable: `populate` is handed none, and there is nothing
            // above this to cancel it.
            // SAFETY: a valid child source, a NULL cancellable and an
            // out-parameter initialised to NULL are the documented arguments.
            let removed = unsafe { e_source_remove_sync(source, ptr::null_mut(), &mut error) };

            if removed != GFALSE {
                // A success that also set an error would be a broken callee,
                // but freeing it costs nothing and leaking it costs a report.
                if !error.is_null() {
                    // SAFETY: a GError this call owns and nothing else holds.
                    unsafe { g_error_free(error) };
                }
                return None;
            }

            // SAFETY: the call failed, so the out-parameter is NULL or a GError
            // ownership of which passed to us.
            let message = unsafe { take_message(error) };
            Some(NotRemoved {
                resource_id,
                message,
            })
        })
        .collect()
}

/// [`obsolete`], with each source's resource id kept — read once, since reading
/// it twice is two chances for a source that changed underneath to be removed
/// under a name it no longer has.
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
            // A source with no resource id of ours is one we have no opinion
            // about — see the module comment.
            // SAFETY: a valid or NULL source by the contract above.
            let resource_id = unsafe { resource_id_of(source) }?;
            fanout
                .is_obsolete(&resource_id)
                .then_some((source, resource_id))
        })
        .collect()
}

/// The message of a `GError` this call owns, and then frees.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        // A `FALSE` with no `GError` is a callee that did not say why. There is
        // still a child that was not removed, and a report with no reason is
        // worth more than no report.
        return "the removal failed and EDS set no error".to_owned();
    }

    // SAFETY: a live GError; its message is a NUL-terminated string it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: ownership passed to us with the out-parameter.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "the removal failed and EDS gave no message".to_owned())
}
