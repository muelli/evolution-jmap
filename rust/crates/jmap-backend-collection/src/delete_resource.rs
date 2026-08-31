// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deleting an address book or a calendar *from the server* —
//! [`crate::create_resource`]'s mirror, and the destructive half of Track D1.
//!
//! Everything in this crate except the create mirrors what the server holds.
//! The create makes the server hold something new; this makes it stop holding
//! something, because the user chose "Delete" on an address book or a calendar
//! of a JMAP account. `ECollectionBackendClass::delete_resource_sync` is that
//! request, and [`crate::backend`] is where the slot is filled.
//!
//! As with the create, what is here is the EDS ends: which collection the child
//! `ESource` stands for, and the flag that makes the menu item exist at all. The
//! decision in between — which JMAP account, which `/set` destroy — is
//! [`jmap_collection_sync::delete`]'s, where it needs no headers and is tested
//! against a running `jmap-mockd`.
//!
//! ## The obligation, taken from EDS's own documentation of the vfunc
//!
//! `e_collection_backend_delete_resource_sync()`: *"After the server-side
//! resource is successfully deleted, the implementor must also remove @source
//! from the @backend's #ECollectionBackend:server."* Hence
//! [`crate::removal::remove_source`] after the destroy and never before it — the
//! same `e_source_remove_sync` a populate removes an obsolete child with, and the
//! same one `ews_backend_delete_resource_sync` ends on.
//!
//! The order is the whole of the error handling. A source removed first and a
//! destroy that then failed is an address book gone from the sidebar and still
//! on the server, which the next populate silently puts back under a *new* uid —
//! losing the old source's offline cache for nothing. A destroy that worked and
//! a removal that then failed is the recoverable direction: the child is stale,
//! the next populate finds the collection gone and removes it through
//! [`crate::removal`], and in the meantime the user has been told.
//!
//! ## The parent's implementation is not chained up to
//!
//! For the same reason [`crate::create_resource`] gives, found in the same
//! place: `collection_backend_delete_resource()` does exactly one thing,
//! `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
//! deleting remote resources")`. The parent *is* the refusal this override
//! replaces.
//!
//! ## `remote-deletable` is the child's flag, not the account's
//!
//! The opposite of `remote-creatable`, which [`crate::populate`] sets on the
//! account source. `server_side_source_remote_delete_sync()` refuses on the
//! *child's* own flag before it ever looks for a backend, so without
//! [`offer_deletion`] this vfunc is unreachable dead code — and with it set on
//! the wrong sources it is worse than that.
//!
//! It is set from [`crate::backend`]'s `child_added`, which is EDS's own funnel
//! for every source that appears under a collection: the children a fan-out just
//! wrote, the cached ones a populate exported before any server was contacted,
//! and the one a create just published. That is also where EDS itself sets
//! `removable = FALSE` on every child, for the same "one place, every child"
//! reason. evolution-ews instead sets it at each of the three sites that mint a
//! child; one funnel is the same behaviour with no site left to forget.
//!
//! **This funnel writes the permissive default, not the last word.** It
//! answers `deletable` for the *identity* question above — is this source a
//! collection this backend wrote at all — and has no opinion on `myRights`,
//! because a cached or newly-published child reaches it before or without any
//! server contact. [`crate::fan_out::adopt`] corrects that default to the
//! real per-resource `myRights.mayDelete` immediately after a fan-out writes
//! a child — new or rediscovered — through
//! [`Collection::set_remote_deletable`](crate::fan_out::Collection::set_remote_deletable),
//! which is also the only place a rights *change* on an already-existing
//! child ever reaches it, since `child_added` fires exactly once per source's
//! life in a running registry.
//!
//! ## What may be deleted is decided by what the source says it is
//!
//! [`doomed_of`] answers `None` for every source that is not a child this
//! backend wrote — no `[Address Book]`/`[Calendar]` extension, or no
//! `[Resource] Identity` — and both the flag and the vfunc are gated on it. That
//! is not defensive coding: `child_added` fires for this account's mail sources
//! too, and the vfunc is handed whichever source the user clicked on. A guess
//! here is not a wrong error message, it is a destroy sent to a JMAP server
//! naming an id read out of somebody else's keyfile.
//!
//! The kind is half of that identity and never inferred from the id — see
//! [`jmap_collection_sync::delete`] on why an address book and a calendar may
//! share one.

use std::fmt;

use eds_sys::{ESource, e_server_side_source_set_remote_deletable};
use glib_sys::{GError, GTRUE};
use jmap_backend_core::error::{invalid_arg_gerror, to_gerror};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::source::{self, ConnectTarget};
use jmap_client::Credentials;
use jmap_collection_sync::{DeleteFailure, Doomed, delete_collection, parse_resource_id};

use crate::create_resource::collection_of;
use crate::resource_id::resource_id_of;

/// Why a delete could not be done.
#[derive(Debug)]
pub enum DeleteError {
    /// The source is not a collection this backend wrote, so there is nothing
    /// of this account's to destroy for it. Unreachable through Evolution — the
    /// same question gates [`offer_deletion`], so a source that answers this
    /// never offered the menu item — and an error rather than a silent success
    /// precisely because reaching it means something else is wrong.
    NotOurs,
    /// The account could not be turned into a login — a server it may not
    /// contact, or no credentials for it.
    Login(crate::authenticate::LoginError),
    /// The connection or the destroy itself failed, or the account's server
    /// holds no place this kind of collection could have come from.
    Server(DeleteFailure),
    /// The collection is gone from the server and EDS would not remove the
    /// source describing it. Carries EDS's own reason.
    Stale(String),
}

impl From<crate::authenticate::LoginError> for DeleteError {
    fn from(failure: crate::authenticate::LoginError) -> Self {
        Self::Login(failure)
    }
}

impl From<DeleteFailure> for DeleteError {
    fn from(failure: DeleteFailure) -> Self {
        Self::Server(failure)
    }
}

impl From<jmap_client::Error> for DeleteError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Server(DeleteFailure::Client(error))
    }
}

impl fmt::Display for DeleteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOurs => f.write_str(&translate(
                // TRANSLATORS: shown when Evolution asks a JMAP account to
                // delete something that is not one of the address books or
                // calendars that account created.
                c"this is not an address book or a calendar of this JMAP account",
            )),
            Self::Login(failure) => failure.fmt(f),
            // `Unserved` is `jmap-collection-sync`'s, and that crate builds
            // without gettext, so its own `Display` is the developer-facing
            // spelling and the user-facing one is written here — with the same
            // translated nouns every other account error uses.
            Self::Server(DeleteFailure::Unserved(kind)) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar". Shown when
                // the JMAP server behind an account offers no account of that
                // kind at all, so the collection cannot be there to delete.
                c"this account's JMAP server has no %1$s to delete",
                &[collection_of(*kind).noun().as_str()],
            )),
            Self::Server(failure) => failure.fmt(f),
            // The msgid below is one long line on purpose: Rust strips a
            // `\`-continuation's newline *and* the next line's leading spaces,
            // xgettext strips neither, so a wrapped literal is a msgid no
            // translation ever matches.
            Self::Stale(reason) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is a developer-facing reason from Evolution
                // Data Server. Shown when the collection was deleted on the
                // server and the local source describing it stayed behind.
                c"the collection was deleted on the server, but the source for it could not be removed: %1$s",
                &[reason.as_str()],
            )),
        }
    }
}

impl std::error::Error for DeleteError {}

impl DeleteError {
    /// The `GError` the vfunc reports this through.
    ///
    /// The same split [`CreateError::to_gerror`] makes, for the same reason: a
    /// `jmap_client::Error` goes through the mapping every other operation in
    /// these backends uses, so a 401 on a delete becomes the Evolution error
    /// code a 401 always becomes, and everything else is
    /// `E_CLIENT_ERROR_INVALID_ARG`.
    ///
    /// [`CreateError::to_gerror`]: crate::create_resource::CreateError::to_gerror
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Login(failure) => failure.to_gerror(),
            Self::Server(DeleteFailure::Client(error)) => to_gerror(error),
            other => invalid_arg_gerror(&other.to_string()),
        }
    }
}

/// Which collection `source` stands for, or `None` for a source this backend
/// did not write.
///
/// Read through [`resource_id_of`] and [`parse_resource_id`] rather than off the
/// extensions directly, so that the string the delete acts on is the very one
/// `dup_resource_id` hands EDS for this child. A second spelling of that read
/// could disagree with it, and the way it would disagree is by naming a
/// different object to destroy.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` that outlives the call. It is only
/// read from — no extension is created on it, which matters because this is
/// asked about sources belonging to other parts of Evolution.
pub unsafe fn doomed_of(source: *mut ESource) -> Option<Doomed> {
    // SAFETY: the caller's contract is `resource_id_of`'s.
    let resource_id = unsafe { resource_id_of(source) }?;
    let (kind, collection_id) = parse_resource_id(&resource_id)?;
    Some(Doomed {
        kind,
        collection_id,
    })
}

/// Offers `child` for deletion if it is a collection this backend wrote, and
/// says whether it did.
///
/// Called from `child_added` for every source that appears under this
/// collection — see the module comment on why that is the one place, and on why
/// a source that is not ours must not be offered.
///
/// The flag is only ever *set*: a source that answers `None` is left exactly as
/// it was rather than being written `FALSE`, because it may belong to another
/// part of Evolution that set it `TRUE` for reasons of its own. Nothing here has
/// any business withdrawing that.
///
/// # Safety
///
/// `child` must be NULL or a valid `EServerSideSource` that outlives the call —
/// which every child of a collection is, since `evolution-source-registry` holds
/// no other kind. A plain `ESource` would earn a `g_return_if_fail` critical from
/// EDS rather than undefined behaviour, but it is still not this function's
/// contract.
pub unsafe fn offer_deletion(child: *mut ESource) -> bool {
    // SAFETY: the caller's contract is `doomed_of`'s, NULL included.
    let doomed = unsafe { doomed_of(child) };
    let deletable = doomed.is_some();
    tracing::debug!(
        remote_deletable = deletable,
        "evaluating child source remote-deletable offer"
    );
    if !deletable {
        return false;
    }

    // SAFETY: a valid `EServerSideSource` by this function's contract, non-NULL
    // because `doomed_of` answered `Some` for it.
    unsafe { e_server_side_source_set_remote_deletable(child.cast(), GTRUE) };
    true
}

/// Connects as `target`/`credentials` say and destroys `doomed` there.
///
/// [`crate::create_resource::create_on_server`]'s mirror, and split out for the
/// same reason: the vfunc gets one thing to do with a
/// [`Login`](crate::authenticate::Login) and one error type to report. The
/// connect is [`jmap_backend_core::source::connect`]'s — so a bare-domain
/// account gets the same `_jmap._tcp` autodiscovery a fan-out gets — and the
/// destroy is [`delete_collection`]'s.
pub fn delete_on_server(
    target: &ConnectTarget,
    rebase_urls: bool,
    credentials: Credentials,
    doomed: &Doomed,
) -> Result<(), DeleteError> {
    let client = source::connect(target, rebase_urls, credentials)?;
    delete_collection(&client, doomed)?;
    tracing::debug!(
        collection_id = doomed.collection_id.as_str(),
        kind = ?doomed.kind,
        "deleted collection resource on server"
    );
    Ok(())
}
