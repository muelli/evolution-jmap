// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the resource id that EDS uses to identify a child source.
//!
//! EDS calls the `dup_resource_id` vfunc for each `.source` file in the cache
//! directory of the backend. It does this once per start, before `populate` runs.
//! `collection_backend_load_resources()` uses the answer as the key of the
//! unclaimed-resources table.
//! If the answer is `NULL`, or is equal to the answer of a source that EDS
//! loaded before, EDS **deletes** the cache file. The child then loses its uid and
//! its offline copy of the collection. No error path exists: the deletion is
//! the only result.
//!
//! Thus this module reads two fields and does not guess either:
//!
//! - **The kind**, from the extension of the source. EDS uses the `[Address Book]`
//!   and `[Calendar]` extensions in `collection_backend_child_is_contacts()` and
//!   `collection_backend_child_is_calendar()`, and [`Child::settings`] writes them.
//! - **The identity**, from `[Resource] Identity`. This is the JMAP id of the
//!   collection. The address book and calendar backends read the same field to
//!   find the object to fetch.
//!
//! [`resource_id_for`] combines the two fields. It is in `jmap-collection-sync`
//! with the rest of the decision. This module contains only the `ESource` reads.
//!
//! ## Why the identity is not read directly
//!
//! `e_source_get_extension()` *creates* the extension that it gets. A direct read
//! of `[Resource]` would thus add an empty extension to each source that this
//! vfunc gets, which includes sources of other backends (EDS calls
//! `dup_resource_id` for each file that it finds). Thus the code tests for the
//! extension with `e_source_has_extension()` before it reads it. The default
//! implementation in EDS does the same, for the same reason.
//!
//! ## Why the answer contains the kind
//!
//! The EDS default returns the identity without change. This is sufficient for
//! `EWebDAVCollectionBackend` only because it writes a string with a kind prefix
//! into `[Resource] Identity`. This backend writes the bare JMAP id into
//! `Identity`, because the book and calendar backends read that field as the id
//! (see the module comment of [`child_source`]). JMAP ids are unique per data
//! type, not per account (RFC 8620 §1.2). Thus an address book and a calendar can
//! both have the id `X1`. With the default vfunc, EDS then finds the second
//! source redundant and deletes it. This module adds the prefix when it derives
//! the resource id, and the stored field keeps the bare id.
//!
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`child_source`]: jmap_collection_sync::child_source
//! [`resource_id_for`]: jmap_collection_sync::resource_id_for

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_has_extension, e_source_resource_dup_identity,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::{extension_if_present, take_string};
use jmap_collection_sync::child_source::{EXTENSION_ADDRESS_BOOK, EXTENSION_CALENDAR};
use jmap_collection_sync::{ChildKind, resource_id_for};

/// The `ESource` extensions of the children of this backend. Each extension has
/// its spelling in `jmap-collection-sync` next to it.
///
/// This table keeps two spellings of each string in one place. The crate that
/// decides the kind of a child must build without the EDS headers, so it has its
/// own literals. This crate is the only one that sees both spellings, and
/// `tests/resource_id.rs` compares each pair with the EDS `#define`. A typo does
/// not cause a failure. It causes a child with no kind, and EDS then deletes the
/// cache file of that child.
///
/// The code tests the kinds in this order. The order is important only for a
/// source that has both extensions. This backend never writes such a source. A
/// file that a user edits to have both extensions is read as an address book.
/// This is a choice, not a rule.
/// EDS `collection_backend_child_added()` uses the opposite order: it
/// tests for a calendar first.
pub const KIND_EXTENSIONS: [(&CStr, &str); 2] = [
    (E_SOURCE_EXTENSION_ADDRESS_BOOK, EXTENSION_ADDRESS_BOOK),
    (E_SOURCE_EXTENSION_CALENDAR, EXTENSION_CALENDAR),
];

/// Returns the resource id of `child_source`, or `None` for a source that this
/// backend did not create.
///
/// The vfunc returns `None` as `NULL`. For a file in the cache directory of this
/// backend, `NULL` causes EDS to delete the file. Thus this function must return
/// `Some` for each child that
/// [`Child::settings`](jmap_collection_sync::Child::settings) describes, and
/// `None` for all other sources.
///
/// # Safety
///
/// `child_source` must be NULL or a valid `ESource` that outlives the call.
pub unsafe fn resource_id_of(child_source: *mut ESource) -> Option<String> {
    // SAFETY: the contract of the caller is the contract of this function.
    // `kind_extension_of` handles NULL.
    let extension = unsafe { kind_extension_of(child_source) }?;

    // Test for the extension, do not get it: `e_source_get_extension` creates it.
    // SAFETY: `child_source` is not NULL because of the check above. It is valid
    // because of the contract of this function. The extension name is the name
    // of `ESourceResource`.
    let resource = unsafe {
        extension_if_present::<ESourceResource>(child_source, E_SOURCE_EXTENSION_RESOURCE)
    }?;
    // Copy the identity: the deferred delete thread calls this function, and the
    // main loop can free a borrowed string during the read.
    // SAFETY: because of the guard above, `resource` is a live extension that the
    // source owns. The duplicate is NULL or a NUL-terminated string that this
    // function owns and frees.
    let identity = unsafe { take_string(e_source_resource_dup_identity(resource)) }?;

    resource_id_for(extension, &identity)
}

/// Returns the kind of collection of `source`, from its extension. Returns
/// `None` for a source that is not an address book or a calendar.
///
/// This is the part of [`resource_id_of`] that does not need
/// `[Resource] Identity`. A `create_resource_sync` has only this part: EDS gives
/// that vfunc a *scratch* source. That source has the kind from the Evolution
/// dialog, but no identity, because the create gets the identity from the server.
///
/// A source with **both** extensions returns `AddressBook`. This is the order
/// that [`KIND_EXTENSIONS`] documents, not a second rule. The caller decides
/// whether to refuse an ambiguous scratch source. This backend never writes one.
///
/// # Safety
///
/// As [`resource_id_of`].
pub unsafe fn kind_of(source: *mut ESource) -> Option<ChildKind> {
    // SAFETY: the contract of the caller is the contract of this function.
    // `kind_extension_of` handles NULL.
    let extension = unsafe { kind_extension_of(source) }?;
    ChildKind::from_extension(extension)
}

/// Returns the [`KIND_EXTENSIONS`] entry of `source`, in the spelling of
/// `jmap-collection-sync`.
///
/// # Safety
///
/// As [`resource_id_of`].
unsafe fn kind_extension_of(source: *mut ESource) -> Option<&'static str> {
    if source.is_null() {
        return None;
    }

    KIND_EXTENSIONS
        .iter()
        .find(|(defined, _)| {
            // SAFETY: the source is valid because of the contract above, and the
            // name is a header constant.
            unsafe { e_source_has_extension(source, defined.as_ptr()) != GFALSE }
        })
        .map(|(_, extension)| *extension)
}
