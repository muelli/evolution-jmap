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
//! - [`backend`] is the subclass those functions exist for: an instance struct,
//!   the vfunc slots, and a panic guard in front of each.
//!
//! Still missing, and the reason there is no module entry point yet: `populate`
//! itself, which is where the fan-out actually happens — `Fanout::discover`
//! against the server [`collection_source::server_of`] names, an
//! `e_collection_backend_new_child` per [`Child`] it warrants, and an
//! `e_source_remove_sync` for each child `Fanout::is_obsolete` names.
//! `dup_resource_id` came first because `populate` cannot be written without it
//! — EDS loads the cached children and asks their resource ids *before* it calls
//! `populate`, and a populate that ran against a mis-loaded child list would
//! create duplicates of children that are already there.
//!
//! [`Child`]: jmap_collection_sync::Child
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers and so
//! stays out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod backend;
pub mod collection_source;
pub mod resource_id;
