// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP collection backend (M6): the `ECollectionBackend` subclass
//! `evolution-source-registry` fans one account out through.
//!
//! `jmap-collection-sync` already answers, in Rust terms, every question this
//! backend has to answer: which JMAP account serves mail, contacts and
//! calendars for a login, which collections are in those accounts, what child
//! source each becomes, what has to be set on it, and which of the account's
//! parts the user left switched on. What is left is the two ends of that pipe —
//! the `ESource` the decisions are read off and written to, and the class struct
//! EDS dispatches through — and that is what this crate is.
//!
//! - [`resource_id`] is the first of them, and the one with teeth: the name EDS
//!   knows a child source by, read back off the source itself. A wrong answer
//!   here does not fail an operation, it **deletes a cache file** — see the
//!   module for what EDS does with a `NULL`.
//! - [`collection_source`] is the other direction and the other source: what the
//!   *account* says — which parts the user switched on, and where its server is
//!   — which is everything `populate` knows before it contacts anything.
//! - [`child_source`] is the write half, and the mirror of [`resource_id`]:
//!   [`Child::settings`] onto the `ESource` `e_collection_backend_new_child`
//!   hands back. Everything a child is, it is because this wrote it there —
//!   including the two properties whose absence loses the cache file or points
//!   the child at no server.
//! - [`child_added`] is what keeps that write true afterwards, and the answer to
//!   the question [`child_source`] does not ask: an account's server, port, user
//!   and TLS setting are copied onto a child once, when the child is created,
//!   and the user may edit any of them the next day. So the copy is turned into
//!   a binding at the vfunc EDS calls for every source that appears under the
//!   collection — the same thing evolution-ews does, over the properties a JMAP
//!   child's connection is made of.
//! - [`mail_child`] is the one child that following does not fit: this
//!   account's mail account and mail transport, which are sources of this
//!   account that this backend neither creates nor caches and that nothing else
//!   writes a server onto — Evolution hides the sending page for a
//!   store-and-transport provider, so the setup UI is never asked where the
//!   account submits through. So the group is created here, from the account, and
//!   `[Security]` is written as the `CamelNetworkSecurityMethod` nick the mail
//!   side reads rather than as the word EDS's own boolean writes.
//! - [`removal`] is the other thing a populate does with a fan-out, and the one
//!   that destroys rather than creates: the children this collection has and no
//!   longer warrants, read back off their sources and removed. The decision is
//!   [`Fanout::is_obsolete`]'s; what is here is the join to the `ESource` and
//!   the `e_source_remove_sync` — including what a populate does with a removal
//!   EDS refuses, which is all it can do, since the vfunc returns `void`.
//! - [`authenticate`] is the piece between the account and the fan-out, and the
//!   answer to where a collection backend's credentials come from: not from
//!   `populate`, which returns `void` and is handed nothing, but from
//!   `EBackendClass::authenticate_sync`, which EDS calls back into with an
//!   `ENamedParameters` once the registry has resolved the account's password.
//!   So it is also where the fan-out belongs — and where the one enum that
//!   decides whether Evolution prompts again, gives up, or says nothing is
//!   written.
//! - [`fan_out`] is all of those put together, and the closure
//!   [`authenticate::authenticate_with`] is handed: [`Fanout::discover`] against
//!   the [`Login`] it is given, an `e_collection_backend_new_child` plus
//!   [`child_source::apply`] per [`Child`] it warrants, and
//!   [`removal::remove_obsolete`] over the children the collection already has.
//!   The four calls on a live collection it needs are a trait, so the order and
//!   the decisions are testable against a real `jmap-mockd` and real `ESource`s
//!   on a machine with no `evolution-source-registry`.
//! - [`populate`] is the vfunc EDS reaches all of that through first, and the
//!   half of it that needs no server: the cached children of previous sessions,
//!   claimed and exported so that an account works *offline*, and then the one
//!   call that asks EDS to authenticate the account — which is what eventually
//!   produces a fan-out, one vfunc later.
//! - [`backend`] is the subclass those functions exist for: an instance struct,
//!   the three vfunc slots — `dup_resource_id`, `populate` and
//!   `authenticate_sync` — and a panic guard in front of each. It is also where
//!   the live `ECollectionBackend` finally appears, as the one implementation of
//!   [`populate::Populating`] and [`fan_out::Collection`] that is not a test's.
//! - [`prepare_mail`] is the mail half, and the one part of this crate that is
//!   not about children of this collection: the mail account, identity and
//!   transport sources cannot be cached children — `dup_resource_id` would have
//!   to claim them and EDS deletes the cache file of every source it does not —
//!   so they belong to the registry's own source directory and to the setup UI.
//!   What is left for a collection factory is the vfunc that says which service
//!   they are, which for JMAP is one Camel protocol on both.
//! - [`factory`] is what the registry actually looks up, because it never
//!   instantiates a backend itself: an `ECollectionBackendFactory` subclass whose
//!   two fields say which `BackendName` this is and which type to build. Both
//!   have working defaults underneath them, so an unwritten factory is an account
//!   that belongs to somebody else or one that fans out to nothing.
//! - [`module`] is the pair of entry points the registry dlopens the built
//!   `module-jmap-backend.so` for, and the only code in this crate the registry
//!   calls by name. The C symbols `e_module_load`/`e_module_unload` themselves
//!   live in the `jmap-backend-collection-module` cdylib, which is that shared
//!   object; the module says why.
//!
//! `dup_resource_id` came first because [`populate`] cannot be written without
//! it — EDS loads the cached children and asks their resource ids *before* it
//! calls `populate`, and a populate that ran against a mis-loaded child list
//! would create duplicates of children that are already there.
//!
//! [`Child`]: jmap_collection_sync::Child
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`Login`]: authenticate::Login
//! [`Fanout::discover`]: jmap_collection_sync::Fanout::discover
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete
//!
//! Still missing, and the one part of M6 the roadmap asks for that is not here:
//! the mail sources are *filled in* by [`prepare_mail`] but nothing yet
//! **creates** them. That is the setup UI's job (M7) in every reference
//! implementation, and it is also why [`fan_out::Collection::existing_children`]
//! has no opinion about mail: a source this backend neither creates nor caches
//! is not one it may remove. `docs/manual-test-collection-backend.md` is the
//! recipe for what does work, and the account it documents has
//! `MailEnabled=false` for that reason.
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers and so
//! stays out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod authenticate;
pub mod backend;
pub mod child_added;
pub mod child_source;
pub mod collection_source;
pub mod factory;
pub mod fan_out;
pub mod mail_child;
pub mod module;
pub mod populate;
pub mod prepare_mail;
pub mod removal;
pub mod resource_id;
