// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP collection backend: the `ECollectionBackend` subclass through
//! which `evolution-source-registry` fans out one account.
//!
//! `jmap-collection-sync` makes the decisions in Rust. It decides which JMAP
//! account serves mail, contacts and calendars for a login, and which
//! collections those accounts have. It also decides which child source each
//! collection becomes, what to set on it, and which parts of the account the
//! user enabled. This crate supplies
//! the two ends: the `ESource` that the decisions are read from and written
//! to, and the class struct through which EDS dispatches.
//!
//! - [`resource_id`] gives the name by which EDS knows a child source, read
//!   from the source itself. A wrong answer here **deletes a cache file**.
//!   The module tells what EDS does with a `NULL`.
//! - [`collection_source`] reads the *account* source: which parts the user
//!   enabled, and where the server is. That is all that `populate` knows
//!   before it contacts a server.
//! - [`child_source`] is the write half and the mirror of [`resource_id`]: it
//!   writes [`Child::settings`] onto the `ESource` that
//!   `e_collection_backend_new_child` returns. This includes the two
//!   properties without which the cache file is lost or the child has no
//!   server.
//! - [`child_added`] keeps that write correct. A child gets the server, port,
//!   user and TLS setting of the account once, at creation, and the user can
//!   edit them later. Thus the vfunc that EDS calls for each source under the
//!   collection binds these properties, as evolution-ews does.
//! - [`mail_child`] serves the mail account and mail transport of the account.
//!   This backend does not create or cache them. Evolution hides the sending
//!   page for a store-and-transport provider, so nothing else writes a server
//!   onto the transport. Thus this module creates the group from the account.
//!   It writes `[Security]` as the `CamelNetworkSecurityMethod` nick that the
//!   mail side reads, not as the word that the EDS boolean writes.
//! - [`removal`] removes the children that this collection has but no longer
//!   needs, read from their sources. [`Fanout::is_obsolete`] makes the
//!   decision. This module joins it to the `ESource`, calls
//!   `e_source_remove_sync`, and handles a removal that EDS refuses (the vfunc
//!   returns `void`).
//! - [`authenticate`] supplies the credentials of the collection backend.
//!   They do not come from `populate`, which returns `void` and gets no
//!   arguments. They come from `EBackendClass::authenticate_sync`, which EDS
//!   calls with an `ENamedParameters` when the registry has the password of
//!   the account. Thus the fan-out starts there. That module also has the enum
//!   that decides if Evolution prompts again, stops, or shows nothing.
//! - [`fan_out`] combines these parts in the closure that
//!   [`authenticate::authenticate_with`] gets: [`Fanout::discover`] with the
//!   given [`Login`], `e_collection_backend_new_child` plus
//!   [`child_source::apply`] for each [`Child`], and
//!   [`removal::remove_obsolete`] over the existing children. A trait holds the
//!   four calls on a live collection. Thus tests check the order and the
//!   decisions against a real `jmap-mockd` and real `ESource`s, with no
//!   `evolution-source-registry`.
//! - [`create_resource`] makes the server hold something new, when the user
//!   selects "New Address Book" or "New Calendar" in Evolution. The vfunc is
//!   `ECollectionBackendClass::create_resource_sync`. Its EDS default is a
//!   *refusal* (`G_IO_ERROR_NOT_SUPPORTED`), so this override must not chain
//!   up. This module reads the kind and name from the scratch `ESource` that
//!   EDS gives, and writes the same [`Child::settings`] as a fan-out. Thus a
//!   created child and a discovered child are the same source.
//!   [`jmap_collection_sync::create`] makes the decision.
//! - [`delete_resource`] is the destructive mirror: "Delete" on an address
//!   book or calendar of this account. The EDS default of
//!   `ECollectionBackendClass::delete_resource_sync` is also a refusal, so it
//!   does not chain up. This module finds the collection of a child `ESource`
//!   through the same [`resource_id`] string that `dup_resource_id` gives EDS.
//!   Thus the delete cannot name a different object than the child does.
//!   [`child_added`] sets the `remote-deletable` flag, which shows the menu
//!   item, on each child of this collection and on nothing else. [`removal`]
//!   then removes the source, as in a populate.
//! - [`deferred_delete`] holds the destroy back for an undo window. The delete
//!   disables the child at once, and the destroy goes out at the due time.
//! - [`populate`] is the vfunc through which EDS first reaches all of this,
//!   and it needs no server. It claims and exports the cached children of
//!   previous sessions, so that an account works *offline*. Then it asks EDS
//!   to authenticate the account, which later causes a fan-out.
//! - [`source_changed`] keeps the `"changed"` connection of the account and
//!   the state that decides if an edit of the account starts a new populate.
//!   EDS does not populate again after an ordinary edit.
//! - [`backend`] is the subclass: an instance struct, the vfunc slots
//!   (`dup_resource_id`, `populate`, `child_added`, `create_resource_sync`,
//!   `delete_resource_sync`, `authenticate_sync` and
//!   `get_destination_address`), and a panic guard in front of each. It also
//!   has the live `ECollectionBackend`, the only implementation of
//!   [`populate::Populating`] and [`fan_out::Collection`] that is not in a
//!   test.
//! - [`prepare_mail`] is the mail half. The mail account, identity and
//!   transport sources cannot be cached children: `dup_resource_id` must claim
//!   each cached child, and EDS deletes the cache file of each source that it
//!   does not claim. Thus these sources are in the source directory of the
//!   registry and belong to the setup UI. The collection factory supplies the
//!   vfunc that names their service, which for JMAP is one Camel protocol for
//!   both.
//! - [`factory`] is what the registry looks up, because the registry never
//!   creates a backend directly. It is an `ECollectionBackendFactory` subclass
//!   with two fields: the `BackendName` and the type to build. Both fields
//!   have defaults. Without this factory, the account belongs to a different
//!   backend or fans out to nothing.
//! - [`module`] has the bodies of the two entry points that the registry
//!   calls by name in `module-jmap-backend.so`. The C symbols
//!   `e_module_load` and `e_module_unload` are in the
//!   `jmap-backend-collection-module` cdylib, which is that shared object.
//!   The module tells why.
//!
//! [`populate`] needs `dup_resource_id`. EDS loads the cached children and
//! asks for their resource ids *before* it calls `populate`. A populate on a
//! wrongly loaded child list makes duplicates of existing children.
//!
//! [`Child`]: jmap_collection_sync::Child
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`Login`]: authenticate::Login
//! [`Fanout::discover`]: jmap_collection_sync::Fanout::discover
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete
//!
//! This backend does not **create** the mail sources. [`prepare_mail`] only
//! fills them in. The account assistant of Evolution creates them, as in each
//! reference implementation. For the same reason,
//! [`fan_out::Collection::existing_children`] ignores mail: this backend may
//! not remove a source that it does not create or cache.
//! `docs/manual-test-collection-backend.md` is the manual test procedure, and
//! its first account has `MailEnabled=false` for that reason.
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers. Thus
//! it is not in the `default-members` of the workspace, and CMake runs its
//! tests through the `rust-test-eds` target.

pub mod authenticate;
pub mod backend;
pub mod child_added;
pub mod child_source;
pub mod collection_source;
pub mod create_resource;
pub mod deferred_delete;
pub mod delete_resource;
pub mod factory;
pub mod fan_out;
pub mod mail_child;
pub mod module;
pub mod populate;
pub mod prepare_mail;
pub mod removal;
pub mod resource_id;
pub mod source_changed;

// Test binaries initialise the parent classes before their tests start threads.
#[cfg(feature = "testing")]
jmap_backend_core::initialise_parents_before_main!(
    backend::JmapCollectionBackend,
    factory::JmapCollectionFactory,
);
