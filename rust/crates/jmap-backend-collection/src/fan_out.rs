// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The body of the fan-out: one authenticated login, turned into the children
//! of a collection.
//!
//! [`crate::authenticate`] ends where the instance begins — it hands a [`Login`]
//! to a closure and classifies whatever comes back. This is that closure. It is
//! the last piece of M6 that is not a vfunc slot, and the only one whose work is
//! *both* halves at once: the children this login warrants, created and written,
//! and the children it no longer warrants, removed.
//!
//! ## What EDS expects of a collection backend, read off its own source
//!
//! `e_collection_backend_new_child()`'s documentation is explicit about two
//! things this module has to get right, neither of which is visible in the
//! header:
//!
//! - the source it returns is a **full reference** — "(transfer full): a
//!   newly-created data source" — so every one has to be unreferenced again;
//! - it is drawn "from a cache of previously used sources indexed by
//!   @resource_id so that locally cached data from previous sessions can be
//!   reused", and "the returned data source should be passed to
//!   `e_source_registry_server_add_source()` to export it over D-Bus".
//!
//! So a child that is merely created is a child nothing can see: creating it
//! pairs a resource id with an `ESource` inside the backend, and exporting it is
//! a second, separate call. EDS's own `EWebDAVCollectionBackend` makes that call
//! under exactly one condition — `if (is_new)`, i.e.
//! `e_collection_backend_is_new_source()` — because a child drawn from the cache
//! was already exported by the `populate` that claimed it, and this module
//! copies that condition rather than inventing one.
//!
//! ## Which questions this module does not ask
//!
//! `e_collection_backend_claim_all_resources()` — the cached children of
//! previous sessions, re-exported before any server is contacted — belongs to
//! `populate`, not here: it is what makes an account's address books appear in
//! the sidebar *offline*, before a password has been resolved, and it must
//! happen whether or not this fan-out ever runs. This module is the other side
//! of that: what a login that *did* succeed says about those children.
//!
//! ## The instance is a trait, for the same reason it was a closure
//!
//! Everything below needs five calls on a live collection — three of
//! `ECollectionBackend`'s own, one on the `ESourceRegistryServer` behind
//! `e_collection_backend_ref_server()`, and one directly on an
//! `EServerSideSource` child (`set_remote_deletable`) — and nothing else of a
//! GObject, while a real instance needs a running `evolution-source-registry`
//! on the session bus, which neither this machine nor CI has. [`Collection`]
//! is those calls and only those, so that the decisions — which children, in
//! which order, written before exported, and which removals — are testable
//! against real `ESource`s and a real `jmap-mockd`, and the untestable part
//! is five one-line method bodies.
//!
//! ## The order, and why it is a decision
//!
//! 1. **The existing children are listed first**, before a single one is
//!    created. `Fanout::is_obsolete` asks whether a child is one the listing did
//!    not contain, and a list taken *after* the new children were added would
//!    contain children this same fan-out had just created. They would not be
//!    judged obsolete — they are in the fan-out by construction — but the
//!    property that keeps them safe would be an accident of what
//!    `is_obsolete` happens to answer rather than of what was asked. EDS's
//!    WebDAV collection backend snapshots its `known_sources` before discovery
//!    for the same reason.
//! 2. **Then the children are created and written**, each independently: one
//!    resource id EDS refuses is one child missing, not a fan-out abandoned.
//! 3. **Then the obsolete ones are removed** — see [`crate::removal`] for what
//!    that destroys and why a refusal is reported rather than raised.
//!
//! ## Nothing half-written is ever exported
//!
//! [`adopt`] writes every setting of a child before it publishes any of it, and
//! a setting it cannot write means the child is dropped unexported rather than
//! exported incomplete. That is [`crate::child_source`]'s rule followed through
//! to its consequence: the two settings whose absence matters are
//! `[Resource] Identity`, whose absence makes EDS delete the child's cache, and
//! `[Authentication] Host`, whose absence points the child at no server — and a
//! child that is never exported has neither problem, because Evolution never
//! sees it. A child *drawn from the cache* was already exported by `populate`,
//! so for that one the damage is already done and all this can do is report it;
//! that is the honest limit of a write that has no transaction.
//!
//! [`Login`]: crate::authenticate::Login
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete

use eds_sys::ESource;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::source;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Fanout, Setting};

use crate::authenticate::Login;
use crate::child_source::{UnwritableSetting, apply};
use crate::removal::{NotRemoved, remove_obsolete};

/// The five calls on a live collection that a fan-out needs.
///
/// One trait rather than four closures because they are four views of one
/// object and are called in an order that matters; see the module comment on
/// why the instance is behind an abstraction at all.
///
/// # Safety
///
/// An implementation must answer with valid `ESource` pointers or NULL, and
/// every non-NULL pointer it hands out must carry a **reference of its own**
/// that the caller consumes — which is what both EDS functions behind these
/// methods do (`(transfer full)`). An implementation that hands out a borrowed
/// pointer will have it unreferenced anyway.
pub unsafe trait Collection {
    /// `e_collection_backend_new_child (backend, resource_id)`: the child source
    /// for this resource id, created or drawn from the cache of previous
    /// sessions.
    ///
    /// NULL is EDS refusing — it warns and answers NULL when it cannot claim the
    /// resource — and is not an error of the fan-out.
    fn new_child(&self, resource_id: &str) -> *mut ESource;

    /// `e_collection_backend_is_new_source (backend, child)`: whether this child
    /// was created by this populate rather than drawn from the cache.
    ///
    /// The one condition [`Collection::publish`] is called under, because a
    /// cached child was exported by the `populate` that claimed it and exporting
    /// it twice is a question this code has no business answering.
    fn is_new_child(&self, child: *mut ESource) -> bool;

    /// `e_source_registry_server_add_source (server, child)`: exports the child
    /// over D-Bus, which is what makes it a source Evolution can see.
    ///
    /// Takes the pointer rather than a reference to it: the caller keeps the
    /// reference it was given by [`Collection::new_child`] and drops it after.
    fn publish(&self, child: *mut ESource);

    /// `e_collection_backend_list_contacts_sources()` followed by
    /// `e_collection_backend_list_calendar_sources()`, concatenated: every child
    /// of this collection that either factory would load.
    ///
    /// Mail sources are deliberately not included. This backend creates none
    /// (see `jmap_collection_sync::children`), so it has no opinion about them,
    /// and a source it has no opinion about is one it must not remove.
    fn existing_children(&self) -> Vec<*mut ESource>;

    /// `e_server_side_source_set_remote_deletable (child, deletable)`: whether
    /// Evolution offers "Delete" on this child at all.
    ///
    /// Called for every child [`adopt`] writes, new or drawn from a previous
    /// fan-out — unlike the other four methods, which only run once per child's
    /// life in this backend, this one has to run on every rediscovery too, so
    /// that a collection whose `myRights.mayDelete` changes on the server is
    /// not stuck with whatever it answered the first time. It runs *after*
    /// [`Collection::publish`], deliberately: a newly published child also goes
    /// through `child_added`'s
    /// [`offer_deletion`](crate::delete_resource::offer_deletion), which sets
    /// the flag unconditionally deletable as the permissive default for a
    /// child fanned out with no rights opinion yet — this call is what
    /// corrects that default to the real per-resource answer, and it must lose
    /// that race, not win it.
    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool);
}

/// What one fan-out did, and everything about it that is worth a log line.
///
/// There is nothing to be done with any of the three failure lists but log
/// them: `populate` returns `void`, so a fan-out has nobody to raise them to,
/// and the recovery available is the next populate finding the same state and
/// trying again.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Populated {
    /// The resource ids written and exported, in the order they were created.
    pub children: Vec<String>,
    /// The resource ids EDS would not create a source for.
    pub uncreated: Vec<String>,
    /// The children dropped unexported because a setting could not be written.
    pub abandoned: Vec<Abandoned>,
    /// The children that should have been removed and could not.
    pub not_removed: Vec<NotRemoved>,
}

/// A child that was created and then dropped without being exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandoned {
    pub resource_id: String,
    /// Which setting stopped the write. Always a fault in this code rather than
    /// in the user's account — see [`UnwritableSetting`].
    pub setting: UnwritableSetting,
}

/// What became of one child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// Written in full. `published` is false for a child drawn from the cache,
    /// which `populate` already exported.
    Written { published: bool },
    /// EDS answered NULL: there is no source to write.
    Uncreated,
    /// A setting could not be written, so nothing was exported.
    Abandoned(UnwritableSetting),
}

/// Connects as `login` says, discovers what it holds, and fans it out.
///
/// The whole of what [`crate::authenticate::authenticate_with`] is handed a
/// closure for. The error is `jmap_client`'s own, because everything that can
/// fail *as a connection* is one of its calls and the layer above classifies
/// that type into the enum EDS re-prompts on; everything that fails per child is
/// in [`Populated`] instead, since a login that worked is not a failure because
/// one address book of it could not be written.
///
/// # Safety
///
/// `collection`'s methods must satisfy [`Collection`]'s contract.
pub unsafe fn fan_out<C: Collection + ?Sized>(
    collection: &C,
    login: &Login,
) -> Result<Populated, jmap_client::Error> {
    let client = source::connect(
        &login.server.target,
        login.server.rebase_urls,
        login.credentials.clone(),
    )?;
    let fanout = Fanout::discover(&client, login.parts)?;
    tracing::debug!(
        address_books_count = fanout.address_books.len(),
        calendars_count = fanout.calendars.len(),
        "discovered collection fan-out resources"
    );

    // SAFETY: the caller's contract is this function's.
    Ok(unsafe { apply_fanout(collection, &fanout, &login.server.connection) })
}

/// The fan-out minus the network: everything a discovered [`Fanout`] does to the
/// children of a collection.
///
/// `connection` is where the account says its server is, which every child has
/// to repeat in order to reach the same one — [`Login::server`]'s
/// [`Server::connection`], never a second read of the `ESource`.
///
/// [`Login::server`]: crate::authenticate::Login::server
/// [`Server::connection`]: crate::collection_source::Server::connection
///
/// # Safety
///
/// As [`fan_out`].
pub unsafe fn apply_fanout<C: Collection + ?Sized>(
    collection: &C,
    fanout: &Fanout,
    connection: &Connection,
) -> Populated {
    // Listed before anything is created — see the module comment on the order.
    let existing = collection.existing_children();

    let mut report = Populated::default();
    for child in fanout.children() {
        // SAFETY: the caller's contract is this function's.
        match unsafe {
            adopt(
                collection,
                &child.resource_id,
                &child.settings(connection),
                child.remote_deletable,
            )
        } {
            Adopted::Written { .. } => report.children.push(child.resource_id),
            Adopted::Uncreated => report.uncreated.push(child.resource_id),
            Adopted::Abandoned(setting) => report.abandoned.push(Abandoned {
                resource_id: child.resource_id,
                setting,
            }),
        }
    }

    // SAFETY: every pointer came from `existing_children`, so each is NULL or a
    // valid source this scope holds a reference to.
    report.not_removed = unsafe { remove_obsolete(fanout, &existing) };
    for source in existing {
        // SAFETY: the reference `existing_children` handed over, released as
        // this drops.
        drop(unsafe { Owned::<ESource>::from_raw(source) });
    }

    report
}

/// One child: created, written in full, and exported if it is new.
///
/// Separate from [`apply_fanout`] because it is the step with the rule in it —
/// nothing half-written is exported — and because `settings` being a parameter
/// is what lets that rule be tested. A [`Child`](jmap_collection_sync::Child)'s
/// own settings are a closed set that
/// [`crate::child_source::apply`] can write all of, so
/// [`Adopted::Abandoned`] cannot be reached through [`apply_fanout`] today; it
/// is reachable the moment `jmap-collection-sync` grows a setting this crate was
/// not taught to write, which is precisely when the child must not be exported.
///
/// # Safety
///
/// As [`fan_out`].
pub unsafe fn adopt<C: Collection + ?Sized>(
    collection: &C,
    resource_id: &str,
    settings: &[Setting],
    remote_deletable: bool,
) -> Adopted {
    // SAFETY: `new_child` is `(transfer full)`; the reference `Owned` takes here
    // is released at the end of this function whichever path is taken below —
    // an exported child is held by the registry server's own reference, and an
    // abandoned one is held by nothing and should be.
    let Some(source) = (unsafe { Owned::<ESource>::from_raw(collection.new_child(resource_id)) })
    else {
        return Adopted::Uncreated;
    };

    // Asked before the write, so that the answer is about the source EDS handed
    // over rather than about one this code has already changed.
    let is_new = collection.is_new_child(source.as_ptr());

    // SAFETY: a non-NULL source EDS created or drew from its cache, alive for
    // as long as this scope's reference to it.
    let written = unsafe { apply(source.as_ptr(), settings) };

    match written {
        Ok(()) => {
            if is_new {
                collection.publish(source.as_ptr());
            }
            // After the publish, not before: a newly published child's
            // `child_added` already ran `offer_deletion`'s unconditional
            // "deletable" default by this point, and this is what corrects it
            // to the real per-resource answer — see `Collection::set_remote_
            // deletable`'s own doc on why the order is load-bearing. For a
            // child drawn from the cache there was no publish and therefore no
            // `child_added` this time either, so this call is the only place a
            // rediscovery's changed rights ever reach an existing child.
            collection.set_remote_deletable(source.as_ptr(), remote_deletable);
            Adopted::Written { published: is_new }
        }
        Err(setting) => Adopted::Abandoned(setting),
    }
}
