// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What EDS knows a child source by, read back off the source.
//!
//! The `dup_resource_id` vfunc is asked about every `.source` file in the
//! backend's cache directory, once per start, before `populate` ever runs. Its
//! answer is not advice: `collection_backend_load_resources()` keys the
//! unclaimed-resources table by it, and a source that answers `NULL` — or that
//! answers what a previously-loaded source already answered — has its cache file
//! **deleted**, taking the child's uid and its offline copy of the collection
//! with it. There is no error path; the deletion is the error path.
//!
//! So this reads two fields, and refuses to guess at either:
//!
//! - **The kind**, from the extension the source carries. `[Address Book]` and
//!   `[Calendar]` are exactly the pair EDS itself keys
//!   `collection_backend_child_is_contacts()` and `…_is_calendar()` off, and
//!   they are what [`Child::settings`] writes.
//! - **The identity**, from `[Resource] Identity` — the JMAP id of the
//!   collection, the same field the address book and calendar backends read as
//!   the object to fetch.
//!
//! [`resource_id_for`] puts them together, and it lives in `jmap-collection-sync`
//! with the rest of the decision rather than here; this module is the `ESource`
//! reads and nothing else.
//!
//! ## Why the identity is not simply read
//!
//! `e_source_get_extension()` *creates* the extension it is asked for. Reaching
//! straight for `[Resource]` would therefore give an empty one to every source
//! this vfunc is handed — including the sources of other backends, since EDS
//! calls `dup_resource_id` on each file it finds — so the extension is tested
//! for with `e_source_has_extension()` before it is read. EDS's own default
//! implementation does the same, for the same reason.
//!
//! ## Why the kind is in the answer at all
//!
//! EDS's default returns the identity verbatim, which is enough for
//! `EWebDAVCollectionBackend` only because it writes a kind-prefixed string into
//! `[Resource] Identity` in the first place. This backend does not: `Identity`
//! is the bare JMAP id, because that is the field the book and calendar backends
//! already read as one (see [`child_source`]'s module comment). JMAP ids are
//! unique per data type and not per account — RFC 8620 §1.2 — so an address book
//! and a calendar may both be `X1`, and under the default vfunc the second of
//! them to load is redundant and is deleted. The prefix lives here, where it is
//! derived, rather than in the stored field.
//!
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`child_source`]: jmap_collection_sync::child_source
//! [`resource_id_for`]: jmap_collection_sync::resource_id_for

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_has_extension, e_source_resource_get_identity,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_collection_sync::child_source::{EXTENSION_ADDRESS_BOOK, EXTENSION_CALENDAR};
use jmap_collection_sync::{ChildKind, resource_id_for};

/// The `ESource` extensions this backend's children carry, each paired with the
/// spelling `jmap-collection-sync` knows it by.
///
/// Two spellings of one string, held together in one place. The crate that
/// decides what a child is has to build without the EDS headers, so it carries
/// its own literals; this crate is the only one that sees both, and
/// `tests/resource_id.rs` holds every pair against EDS's `#define`. A typo in
/// either would not fail — it would be a child that is silently of no kind, and
/// therefore a child whose cache file is deleted.
///
/// Order is the order the kinds are tested in, and it only matters for a source
/// carrying both extensions. This backend never writes one; a hand-edited file
/// that is both an address book and a calendar is read as an address book,
/// which is a choice and not a rule, but it is the same precedence EDS's
/// `collection_backend_child_added()` applies.
pub const KIND_EXTENSIONS: [(&CStr, &str); 2] = [
    (E_SOURCE_EXTENSION_ADDRESS_BOOK, EXTENSION_ADDRESS_BOOK),
    (E_SOURCE_EXTENSION_CALENDAR, EXTENSION_CALENDAR),
];

/// The resource id of `child_source`, or `None` for a source this backend did
/// not create.
///
/// `None` becomes the `NULL` the vfunc returns, which for a file in this
/// backend's cache directory is an instruction to delete it — so every child
/// [`Child::settings`](jmap_collection_sync::Child::settings) describes must
/// come back out of here, and nothing else may.
///
/// # Safety
///
/// `child_source` must be NULL or a valid `ESource` that outlives the call.
pub unsafe fn resource_id_of(child_source: *mut ESource) -> Option<String> {
    // SAFETY: the caller's contract is this function's; NULL is handled there.
    let extension = unsafe { kind_extension_of(child_source) }?;

    // Tested for rather than fetched: `e_source_get_extension` would create it.
    // SAFETY: `child_source` is non-NULL by the check above, valid by this
    // function's contract, and the extension named is `ESourceResource`'s own.
    let resource = unsafe {
        extension_if_present::<ESourceResource>(child_source, E_SOURCE_EXTENSION_RESOURCE)
    }?;
    // SAFETY: `resource` is a live extension the source owns, by the guard
    // above; the identity it holds is NULL or a NUL-terminated string with
    // the same lifetime.
    let identity = unsafe { read_string(e_source_resource_get_identity(resource)) }?;

    resource_id_for(extension, &identity)
}

/// Which kind of collection `source` is, by the extension it carries — or
/// `None` for a source that is neither an address book nor a calendar.
///
/// The half of [`resource_id_of`] that does not need `[Resource] Identity`, and
/// the half a `create_resource_sync` has: EDS hands that vfunc a *scratch*
/// source, which carries the kind Evolution's dialog gave it and no identity at
/// all — the identity is what the create is about to obtain from the server.
///
/// A source carrying **both** extensions answers `AddressBook`, which is
/// [`KIND_EXTENSIONS`]'s documented precedence rather than a second rule; the
/// caller decides whether an ambiguous scratch source is worth refusing. This
/// backend never writes one.
///
/// # Safety
///
/// As [`resource_id_of`].
pub unsafe fn kind_of(source: *mut ESource) -> Option<ChildKind> {
    // SAFETY: the caller's contract is this function's; NULL is handled there.
    let extension = unsafe { kind_extension_of(source) }?;
    ChildKind::from_extension(extension)
}

/// The [`KIND_EXTENSIONS`] entry `source` carries, as `jmap-collection-sync`
/// spells it.
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
            // SAFETY: a valid source by the contract above, and a header
            // constant.
            unsafe { e_source_has_extension(source, defined.as_ptr()) != GFALSE }
        })
        .map(|(_, extension)| *extension)
}
