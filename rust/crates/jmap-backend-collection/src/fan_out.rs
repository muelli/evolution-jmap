// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The body of the fan-out: one authenticated login becomes the children of a
//! collection.
//!
//! [`crate::authenticate`] stops where the instance starts. It gives a
//! [`Login`] to a closure and classifies the result. This module is that
//! closure. It does two tasks: it makes and writes the children that this login
//! gives, and it removes the children that the login no longer gives.
//!
//! ## What EDS expects of a collection backend, from the EDS source code
//!
//! The documentation of `e_collection_backend_new_child()` states two facts
//! that the header does not show:
//!
//! - The source that it returns is a **full reference** ("(transfer full): a
//!   newly-created data source"), so each one must be unreferenced again.
//! - It comes "from a cache of previously used sources indexed by
//!   @resource_id so that locally cached data from previous sessions can be
//!   reused", and "the returned data source should be passed to
//!   `e_source_registry_server_add_source()` to export it over D-Bus".
//!
//! Thus nothing can see a child that is only created. The creation pairs a
//! resource id with an `ESource` in the backend. The export is a second,
//! separate call. `EWebDAVCollectionBackend` of EDS makes that call under one
//! condition only: `if (is_new)`, from `e_collection_backend_is_new_source()`.
//! The `populate` that claimed a cached child already exported it. This module
//! uses the same condition.
//!
//! ## What this module does not do
//!
//! `e_collection_backend_claim_all_resources()` exports the cached children of
//! previous sessions again before the backend contacts a server. It belongs to
//! `populate`, not here. It makes the address books of an account show in the
//! sidebar offline, before a password exists. It must occur also when this
//! fan-out does not run. This module tells what a successful login says about
//! those children.
//!
//! ## The instance is a trait, for the same reason that it is a closure
//!
//! This module needs five calls on a live collection, and nothing else of a
//! GObject. Three are methods of `ECollectionBackend`, one is on the
//! `ESourceRegistryServer` from `e_collection_backend_ref_server()`, and one
//! is on an `EServerSideSource` child (`set_remote_deletable`). A real instance
//! needs a running `evolution-source-registry` on the session bus, and the
//! development machine and CI do not have one. [`Collection`] contains only
//! those calls. Thus the tests can examine the decisions against real
//! `ESource`s and a real `jmap-mockd`: which children, which order, write
//! before export, and which removals. Only five one-line method bodies are not
//! tested.
//!
//! ## The order, and why it is a decision
//!
//! 1. **List the existing children first**, before this module creates a
//!    child. `Fanout::is_obsolete` asks whether the listing did not contain a
//!    child. A list made after the creation would contain the new children of
//!    this fan-out. They would not be obsolete, because they are in the fan-out.
//!    But their safety would then depend on the answer of `is_obsolete`, not on
//!    the question. The WebDAV collection backend of EDS copies its
//!    `known_sources` before discovery for the same reason.
//! 2. **Then create and write the children**, each one independently. When EDS
//!    refuses one resource id, only one child is missing, and the fan-out
//!    continues.
//! 3. **Then remove the obsolete children.** See [`crate::removal`] for what
//!    that deletes and why a refusal is a report, not an error.
//!
//! ## This module never exports a partly written child
//!
//! [`adopt`] writes all settings of a child before it exports the child. If it
//! cannot write a setting, it drops the child and does not export it. This is
//! the consequence of the rule of [`crate::child_source`]. Two settings are
//! important. Without `[Resource] Identity`, EDS deletes the cache of the
//! child. Without `[Authentication] Host`, the child points to no server. A
//! child that is never exported has neither problem, because Evolution never
//! sees it. But `populate` already exported a child from the cache. For that
//! child, this module can only report the problem, because the write has no
//! transaction.
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
/// This is one trait and not five closures, because the calls operate on one
/// object in a specified order. See the module documentation for why the
/// instance is behind a trait.
///
/// # Safety
///
/// An implementation must return valid `ESource` pointers or NULL. Each
/// pointer that is not NULL must carry a **reference of its own** that the
/// caller consumes. The two EDS functions behind these methods do this
/// (`(transfer full)`). The caller unreferences a borrowed pointer all the
/// same.
pub unsafe trait Collection {
    /// `e_collection_backend_new_child (backend, resource_id)`: the child source
    /// for this resource id, new or from the cache of previous sessions.
    ///
    /// NULL means that EDS refuses: it logs a warning and returns NULL when it
    /// cannot claim the resource. This is not an error of the fan-out.
    fn new_child(&self, resource_id: &str) -> *mut ESource;

    /// `e_collection_backend_is_new_source (backend, child)`: whether
    /// [`Collection::new_child`] created this child, and did not take it from
    /// the cache.
    ///
    /// This is the only condition for [`Collection::publish`]. The `populate`
    /// that claimed a cached child already exported it, and this code must not
    /// export it a second time.
    fn is_new_child(&self, child: *mut ESource) -> bool;

    /// `e_source_registry_server_add_source (server, child)`: exports the child
    /// over D-Bus, so that Evolution can see it.
    ///
    /// This method takes the pointer and not its reference. The caller keeps the
    /// reference from [`Collection::new_child`] and releases it after the call.
    fn publish(&self, child: *mut ESource);

    /// `e_collection_backend_list_contacts_sources()` followed by
    /// `e_collection_backend_list_calendar_sources()`, as one list: each child
    /// of this collection that the book factory or the calendar factory loads.
    ///
    /// Mail sources are intentionally not in the list. This backend creates no
    /// mail sources (see `jmap_collection_sync::children`), so it must not
    /// remove them.
    fn existing_children(&self) -> Vec<*mut ESource>;

    /// `e_server_side_source_set_remote_deletable (child, deletable)`: whether
    /// Evolution offers "Delete" on this child.
    ///
    /// [`adopt`] calls it for each child that it writes, new or from the cache,
    /// on each fan-out. [`Collection::publish`] runs only for a new child, but
    /// this method must also run on each later discovery. Then a change of
    /// `myRights.mayDelete` on the server reaches the child. It runs after
    /// [`Collection::publish`] intentionally. A new child also goes through
    /// [`offer_deletion`](crate::delete_resource::offer_deletion) in
    /// `child_added`, which always sets the flag to deletable as a permissive
    /// default. This call then changes that default to the real value for the
    /// resource, so it must run last.
    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool);
}

/// What one fan-out did, with all the data for a log line.
///
/// The caller can only log the three failure lists. A failure of one child
/// does not make the login fail (see [`fan_out`]). The next fan-out finds the
/// same state and tries again.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Populated {
    /// The resource ids that this fan-out wrote, in the order of creation.
    pub children: Vec<String>,
    /// The resource ids for which EDS did not create a source.
    pub uncreated: Vec<String>,
    /// The children that this fan-out dropped without an export, because it
    /// could not write a setting.
    pub abandoned: Vec<Abandoned>,
    /// The obsolete children that this fan-out could not remove.
    pub not_removed: Vec<NotRemoved>,
}

/// A child that this fan-out created and then dropped without an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandoned {
    pub resource_id: String,
    /// The setting that stopped the write. This is always a fault in this code,
    /// not in the account of the user. See [`UnwritableSetting`].
    pub setting: UnwritableSetting,
}

/// The result for one child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// Fully written. `published` is false for a child from the cache, which
    /// `populate` already exported.
    Written { published: bool },
    /// EDS returned NULL: there is no source to write.
    Uncreated,
    /// A setting was not writable, so the child was not exported.
    Abandoned(UnwritableSetting),
}

/// Connects with `login`, discovers the resources, and makes the children.
///
/// This is the closure that [`crate::authenticate::authenticate_with`] gets.
/// The error type is that of `jmap_client`, because each connection failure
/// comes from a `jmap_client` call. The caller classifies that type into the
/// enum that controls the EDS prompt. A failure for one child goes into
/// [`Populated`]. A successful login does not fail because one address book is
/// not writable.
///
/// # Safety
///
/// The methods of `collection` must satisfy the contract of [`Collection`].
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

    // SAFETY: the contract of this function is the contract of the callee.
    Ok(unsafe { apply_fanout(collection, &fanout, &login.server.connection) })
}

/// The fan-out without the network: all changes that a discovered [`Fanout`]
/// makes to the children of a collection.
///
/// `connection` is the server location of the account. Each child must contain
/// it to reach the same server. It is the [`Server::connection`] of
/// [`Login::server`], never a second read of the `ESource`.
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
    // List before the first creation. See the module documentation on the order.
    let existing = collection.existing_children();

    let mut report = Populated::default();
    for child in fanout.children() {
        // SAFETY: the contract of this function is the contract of `adopt`.
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

    // SAFETY: each pointer came from `existing_children`. Thus each one is NULL
    // or a valid source, and this scope holds a reference to it.
    report.not_removed = unsafe { remove_obsolete(fanout, &existing) };
    for source in existing {
        // SAFETY: the reference from `existing_children`, which this drop
        // releases.
        drop(unsafe { Owned::<ESource>::from_raw(source) });
    }

    report
}

/// One child: created, fully written, and exported if it is new.
///
/// This is separate from [`apply_fanout`] because it contains the rule (export
/// only a fully written child). The `settings` parameter lets the tests examine
/// that rule. [`crate::child_source::apply`] can write all the settings of a
/// [`Child`](jmap_collection_sync::Child), so [`apply_fanout`] cannot cause
/// [`Adopted::Abandoned`] now. It occurs when `jmap-collection-sync` gets a
/// setting that this crate cannot write. Then the child must not be exported.
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
    // SAFETY: `new_child` is `(transfer full)`. `Owned` releases this reference
    // at the end of this function on all paths. The registry server keeps its
    // own reference to an exported child. Nothing must keep an abandoned child.
    let Some(source) = (unsafe { Owned::<ESource>::from_raw(collection.new_child(resource_id)) })
    else {
        return Adopted::Uncreated;
    };

    // Ask before the write, so that the answer is about the source from EDS, not
    // about a source that this code changed.
    let is_new = collection.is_new_child(source.as_ptr());

    // SAFETY: a source that is not NULL, which EDS created or took from its
    // cache. It is alive while this scope has its reference.
    let written = unsafe { apply(source.as_ptr(), settings) };

    match written {
        Ok(()) => {
            if is_new {
                collection.publish(source.as_ptr());
            }
            // After the publish, not before. For a new child, `child_added` has
            // already set the "deletable" default of `offer_deletion`. This call
            // changes it to the real value for the resource. See
            // `Collection::set_remote_deletable` for why the order is
            // important. A child from the cache has no publish and no
            // `child_added`. Thus only this call gives it the changed rights.
            collection.set_remote_deletable(source.as_ptr(), remote_deletable);
            Adopted::Written { published: is_new }
        }
        Err(setting) => Adopted::Abandoned(setting),
    }
}
