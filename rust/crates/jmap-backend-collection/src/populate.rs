// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The offline part of a populate: the cached children of previous sessions
//! go back into the sidebar before the backend contacts the server.
//!
//! [`crate::fan_out`] changes the children of a collection after a successful
//! login. This module runs before that, also when no login occurs.
//! `evolution-source-registry` schedules `ECollectionBackendClass::populate`
//! on an idle when an account is added, when one of its three parts is enabled
//! or disabled, and when the session goes online. [`crate::source_changed`]
//! also runs it after an edit of the account, which EDS does not do. It
//! returns `void`, gets no arguments, and has no `GError`. Thus it cannot fan
//! out and it cannot report errors. It does the two things that need no
//! server:
//!
//! 1. **Export the children of previous sessions.** At construction, EDS loads
//!    the cached `.source` files of a collection into an unclaimed table and
//!    exports none of them. Until a populate claims them and gives each to
//!    `e_source_registry_server_add_source()`, Evolution cannot see the address
//!    books and calendars of the account. This makes an account work offline,
//!    and it is the reason why `populate` runs before a password exists.
//! 2. **Ask EDS to authenticate the account.** This eventually causes a
//!    fan-out. See [`crate::authenticate`] for why the credentials come through
//!    `authenticate_sync` and not here.
//!
//! ## Why this module exports the claimed sources directly
//!
//! `EWebDAVCollectionBackend` of EDS calls `e_collection_backend_new_child()`
//! for each claimed source before it exports it. This module intentionally does
//! not. `e_collection_backend_claim_all_resources()` empties its unclaimed
//! table ("previously used sources can only be claimed once"). The claimed
//! sources are also not in the `children` table of the backend yet, because
//! the `child-added` signal fills that table after an export. Thus a
//! `new_child()` after the claim finds neither. It makes a new
//! `EServerSideSource` with a new uid and a new file name. The WebDAV backend
//! then exports the claimed source and unreferences the new one, which stays
//! only as an entry in the `new_sources` table of the backend. That approach
//! makes and discards one source for each cached child on each populate. This
//! module does what the documentation of `claim_all_resources()` asks: "export
//! the remaining instances with `e_source_registry_server_add_source()`".
//!
//! The export also pairs the child with its resource id again, so
//! `new_child()` is not necessary for that. The export makes the registry
//! server emit `source-added`. `ECollectionBackend` receives it, finds a source
//! whose parent is its collection, emits `child-added`, and adds the child to
//! its `children` table. A later `new_child(resource_id)` from the fan-out
//! asks `dup_resource_id` about each entry of that table. It finds this child
//! and uses it again, and does not make a second source for the same
//! collection.
//!
//! ## Each freeze needs a thaw
//!
//! `e_collection_backend_freeze_populate()` increments a counter and tells
//! whether this caller got the freeze: `return !g_atomic_int_add (&count, 1)`.
//! The increment occurs in both cases, so a populate that did not get the
//! freeze must also thaw. Thus the backends of EDS write the guard as
//! `if (!freeze) { thaw (); return; }` and not as a plain early return. An
//! error here is not visible until it is permanent. A missing thaw stops the
//! populate of the account for the life of the process. An extra thaw lets two
//! populates change the same children at the same time.
//!
//! Thus the thaw here is a destructor and not a statement at the end. A
//! populate runs from an idle callback in `evolution-source-registry`. The
//! panic guard in front of the vfunc keeps a Rust panic out of C, but it cannot
//! undo a freeze. A panic between the freeze and the thaw would stop the
//! populate of this account permanently.
//!
//! ## Which accounts this module asks EDS to authenticate, and how
//!
//! These decisions prevent prompts that have no purpose:
//!
//! - **Only an account with a part for which this backend makes children.**
//!   These parts are contacts and calendars. A mail-only account has nothing
//!   for a login to discover, because this backend makes no mail children (see
//!   `jmap_collection_sync::children`). The WebDAV backend of EDS uses the same
//!   two parts as its gate, for the same reason.
//! - **A password only for an account that names a user.** A backend calls
//!   `e_backend_schedule_credentials_required()` to ask EDS for a password
//!   (libsecret, or a prompt through Evolution). It calls
//!   `e_backend_schedule_authenticate()` to ask for authentication now with
//!   the credentials that it has. [`credentials`] intentionally treats an
//!   account that names no user as anonymous. A password request for such an
//!   account shows an unnecessary prompt and then ignores the answer.
//! - **No password request for an OAuth 2.0 account.** For such an account,
//!   [`populate`] calls `e_backend_schedule_authenticate()` (see
//!   [`Asked::Silently`]). Then
//!   [`authenticate_with`](crate::authenticate::authenticate_with) reads
//!   `[Authentication] Method` and gets the access token.
//!
//! ## What is not here
//!
//! Removals. [`crate::removal`] needs a [`Fanout`](jmap_collection_sync::Fanout)
//! to ask `is_obsolete`, and a populate has none. Also, "the server did not
//! say" is not a deletion. See that module and `jmap_collection_sync::parts`.
//!
//! [`credentials`]: jmap_backend_core::connect::credentials

use eds_sys::ESource;
use jmap_backend_core::owned::Owned;
use jmap_collection_sync::{ChildKind, Parts};

use crate::resource_id::resource_id_of;

/// The calls on a live collection that a populate makes.
///
/// This is a trait for the same reason as
/// [`Collection`](crate::fan_out::Collection). A real `ECollectionBackend`
/// needs a running `evolution-source-registry` on the session bus, and the
/// development machine and CI do not have one. Without it, the tests can
/// examine the decisions above (the order, the thaw, and the authentication
/// requests) against real `ESource`s. Only one line for each method is not
/// tested.
///
/// # Safety
///
/// [`claim_all_resources`](Populating::claim_all_resources) must return valid
/// `ESource` pointers or NULL. Each pointer must carry a **reference of its
/// own** that the caller consumes. `e_collection_backend_claim_all_resources()`
/// does this (`(transfer full)`, one reference for each source).
/// [`freeze`](Populating::freeze) and [`thaw`](Populating::thaw) must operate
/// on the same counter. If not, the thaw of this module does not match its
/// freeze.
pub unsafe trait Populating {
    /// `e_collection_backend_freeze_populate (backend)`: whether this caller got
    /// the freeze. False means that another populate of this account runs. The
    /// counter still increments, so a thaw is still necessary.
    fn freeze(&self) -> bool;

    /// `e_collection_backend_thaw_populate (backend)`: releases one freeze.
    fn thaw(&self);

    /// `E_COLLECTION_BACKEND_CLASS (parent_class)->populate (backend)`.
    ///
    /// In EDS 3.52 this is a placeholder ("so subclasses can safely chain up").
    /// A populate that does not call it works today, but fails on the EDS
    /// release that implements it.
    fn chain_up(&self);

    /// `e_collection_backend_claim_all_resources (backend)`: claims the cached
    /// children of previous sessions. Each call after the first returns an
    /// empty list. This is the EDS contract, not an error.
    fn claim_all_resources(&self) -> Vec<*mut ESource>;

    /// `e_source_registry_server_add_source (server, child)`: exports the child
    /// over D-Bus, so that Evolution can see it.
    ///
    /// This method takes the pointer and not its reference. The caller keeps the
    /// reference from [`claim_all_resources`](Populating::claim_all_resources)
    /// and releases it after the call.
    fn publish(&self, child: *mut ESource);

    /// `e_backend_schedule_credentials_required (backend,
    /// E_SOURCE_CREDENTIALS_REASON_REQUIRED, …)`: asks EDS to get the password
    /// of the account and call `authenticate_sync` with it.
    fn request_credentials(&self);

    /// `e_backend_schedule_authenticate (backend, NULL)`: asks EDS to call
    /// `authenticate_sync` now, with no credentials.
    fn authenticate_anonymously(&self);

    /// `e_server_side_source_set_remote_creatable (account_source, offer)`:
    /// whether Evolution can offer this account as the location for a new
    /// address book or calendar.
    ///
    /// This is the only value that a populate writes on the account and not on
    /// a child. It is also the gate for `create_resource_sync`.
    /// `server_side_source_remote_create_sync()` refuses a collection source
    /// without this flag, so without it the vfunc never runs.
    ///
    /// Each populate calls it with the current value for the account, `true` or
    /// `false`. See [`populate`] for why it is not set one time at construction.
    fn offer_creation(&self, offer: bool);

    /// `e_source_collection_set_allow_sources_rename (collection_extension,
    /// TRUE)`: tells the account editor of Evolution that the user can rename
    /// the child address books and calendars of this account. The `constructed`
    /// of evolution-ews sets this one time, always. This backend has no
    /// `constructed` override, so populate calls it, as for
    /// [`offer_creation`](Populating::offer_creation). The value never changes,
    /// and [`populate`] always calls it.
    fn allow_rename(&self);
}

/// How this populate asked for authentication. The authentication causes the
/// fan-out, in a later vfunc.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// No request: the account enables no part for which this backend makes
    /// children, so a login has nothing to discover.
    #[default]
    Nothing,
    /// `e_backend_schedule_credentials_required`: the account names a user, so
    /// EDS gets its password before it calls the backend.
    Credentials,
    /// `e_backend_schedule_authenticate` with NULL: the account names no user,
    /// so there is no password and the fan-out is anonymous.
    Anonymously,
    /// `e_backend_schedule_authenticate` with NULL, as for
    /// [`Asked::Anonymously`]. Here the reason is that the account uses OAuth
    /// 2.0, and [`crate::authenticate::login_of`] gets the token itself. A
    /// credentials-required request sends an OAuth 2.0 source directly to the
    /// consent window. On that path, the OAuth2 credentials provider of EDS
    /// cannot find a token, it can only prompt
    /// (e-source-credentials-provider-impl-oauth2.c: can_store FALSE,
    /// can_prompt TRUE). The consent window then shows on each populate. Each
    /// silent path gets the token correctly.
    Silently,
}

/// What one populate did, with all the data for a log line.
///
/// A populate returns `void`, so this is a report and not a result. No caller
/// receives an error. The next populate finds the same state and tries again.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The resource ids of the cached children that this populate exported
    /// again, in the order of the claim.
    pub children: Vec<String>,
    /// The number of claimed sources that this populate did not export,
    /// because they have no resource id. EDS cannot cause this (see
    /// [`populate`]). This is a count and not a list, because there is no name.
    pub unidentified: usize,
    /// How this populate asked for authentication, if it did.
    pub asked: Asked,
    /// Whether Evolution can create an address book or a calendar in this
    /// account. See [`Populating::offer_creation`].
    pub creatable: bool,
}

/// `ECollectionBackendClass::populate` for a JMAP collection, without the
/// instance.
///
/// `parts`, `user` and `uses_oauth2` are the values of the account, from
/// [`parts_of`](crate::collection_source::parts_of),
/// [`user_of`](crate::collection_source::user_of) and
/// [`source_uses_oauth2`](jmap_backend_core::oauth2::source_uses_oauth2). A
/// populate gets no other data and cannot contact a server.
///
/// `None` means that another populate had the freeze. This populate then only
/// released its freeze.
///
/// This function counts a claimed source without a readable resource id, and
/// does not export it. EDS cannot cause this, because EDS only caches a source
/// for which `dup_resource_id` gave a value. But an export would put a child in
/// the sidebar that no resource id can match again.
/// `e_collection_backend_new_child()` finds an existing child with
/// `dup_resource_id`, so each later fan-out makes a new source for a child
/// without a resource id. This is the rule of [`adopt`](crate::fan_out::adopt)
/// (export only what is fully written), applied to the read.
///
/// # Safety
///
/// The methods of `collection` must satisfy the contract of [`Populating`].
pub unsafe fn populate<P: Populating + ?Sized>(
    collection: &P,
    parts: Parts,
    user: Option<&str>,
    uses_oauth2: bool,
) -> Option<Restored> {
    tracing::debug!(
        contacts_wanted = parts.contacts,
        calendars_wanted = parts.calendars,
        has_user = user.is_some(),
        uses_oauth2,
        "populating collection from cache"
    );

    let held = collection.freeze();
    // Make the guard for both answers, before the early return: the freeze
    // always increments the counter. A destructor makes sure that a panic does
    // not stop the populate of this account permanently.
    let _frozen = Frozen(collection);
    if !held {
        return None;
    }

    collection.chain_up();

    let mut report = Restored::default();
    for source in collection.claim_all_resources() {
        // SAFETY: `claim_all_resources` is `(transfer full)`. `Owned` releases
        // this reference at the end of the loop body on both branches. The
        // registry server keeps its own reference to an exported child. Nothing
        // must keep an unexported child.
        let Some(source) = (unsafe { Owned::<ESource>::from_raw(source) }) else {
            continue;
        };

        // SAFETY: a source that is not NULL, which EDS loaded from the cache
        // directory of this collection. It is alive while this scope has its
        // reference.
        match unsafe { resource_id_of(source.as_ptr()) } {
            Some(resource_id) => {
                collection.publish(source.as_ptr());
                report.children.push(resource_id);
            }
            None => report.unidentified += 1,
        }
    }

    // The same condition controls creation and authentication. An account
    // without contacts and calendars has no children of this backend. A created
    // source in it would be dormant on the next populate.
    //
    // Write it on each populate, as `true` or `false`, because the user can
    // change the setting. When the user enables contacts again, the account
    // must become creatable again without a new account. The EDS setter does
    // nothing when the value does not change.
    report.creatable = parts.wants(ChildKind::AddressBook) || parts.wants(ChildKind::Calendar);
    collection.offer_creation(report.creatable);
    collection.allow_rename();

    // Do this last: the cached children must be in the sidebar before a login
    // that can fail. See the numbered list in the module documentation.
    report.asked = if report.creatable {
        if uses_oauth2 {
            // The same EDS call as the anonymous branch, for the reason in
            // [`Asked::Silently`]: `authenticate_sync` gets the token itself.
            // For an OAuth 2.0 source, the credentials-required path only shows
            // the consent window.
            collection.authenticate_anonymously();
            Asked::Silently
        } else {
            match user {
                Some(_) => {
                    collection.request_credentials();
                    Asked::Credentials
                }
                None => {
                    collection.authenticate_anonymously();
                    Asked::Anonymously
                }
            }
        }
    } else {
        Asked::Nothing
    };

    tracing::debug!(
        contacts_wanted = parts.contacts,
        calendars_wanted = parts.calendars,
        cached_count = report.children.len(),
        unidentified_count = report.unidentified,
        asked_auth = ?report.asked,
        "populated collection from cache"
    );

    Some(report)
}

/// One freeze, which this guard thaws when it drops.
struct Frozen<'a, P: Populating + ?Sized>(&'a P);

impl<P: Populating + ?Sized> Drop for Frozen<'_, P> {
    fn drop(&mut self) {
        self.0.thaw();
    }
}
