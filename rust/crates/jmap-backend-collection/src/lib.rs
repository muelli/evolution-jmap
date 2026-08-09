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
//!   the vfunc slots, and a panic guard in front of each.
//!
//! Still missing, and the reason there is no module entry point yet: the
//! `authenticate_sync` slot, which runs [`fan_out::fan_out`] with the
//! [`Collection`](fan_out::Collection) the instance implements. It is small, and
//! it cannot be driven here — it needs a live `ECollectionBackend`, which needs a
//! running `evolution-source-registry` on a session bus.
//! `dup_resource_id` came first because [`populate`] cannot be written without it
//! — EDS loads the cached children and asks their resource ids *before* it calls
//! `populate`, and a populate that ran against a mis-loaded child list would
//! create duplicates of children that are already there.
//!
//! [`Child`]: jmap_collection_sync::Child
//! [`Child::settings`]: jmap_collection_sync::Child::settings
//! [`Login`]: authenticate::Login
//! [`Fanout::discover`]: jmap_collection_sync::Fanout::discover
//! [`Fanout::is_obsolete`]: jmap_collection_sync::Fanout::is_obsolete
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers and so
//! stays out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod authenticate;
pub mod backend;
pub mod child_source;
pub mod collection_source;
pub mod fan_out;
pub mod populate;
pub mod removal;
pub mod resource_id;
