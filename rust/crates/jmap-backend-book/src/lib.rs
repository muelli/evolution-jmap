// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP address book backend (M3): an `EBookMetaBackend` subclass and the
//! two layers underneath it.
//!
//! `jmap-book-sync` already answers every question the meta backend asks, in
//! Rust terms, and `jmap-backend-core::source` reads the account out of an
//! `ESource`. What is left is the two ends of that pipe, and they are the
//! parts where a mistake is a crash in `evolution-addressbook-factory` rather
//! than a failing assertion:
//!
//! - [`connect`] opens a [`BookSync`] — resolving which JMAP address book the
//!   source stands for, and deciding whether a failure means "ask the user for
//!   a password again" or "give up".
//! - [`marshal`] converts between the Rust values and the C ones the vfuncs
//!   traffic in: `GSList`s of `EBookMetaBackendInfo`, `EContact`, and the
//!   `ENamedParameters` EDS fills from libsecret.
//! - [`ops`] is the vfunc bodies themselves, in the shape EDS calls them but
//!   over a [`BookSync`] rather than an `EBookMetaBackend *` — which is what
//!   makes them testable at all, since constructing one of those needs a
//!   running `evolution-source-registry`.
//!
//! All three are exercised directly by the test suite, so [`backend`] — the
//! `EBookMetaBackend` subclass they exist for — stays what it was meant to be:
//! an instance struct, seven vfunc slots, and a panic guard around a lookup of
//! the connection the instance holds.
//!
//! [`BookSync`]: jmap_book_sync::BookSync
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers and so
//! stays out of the workspace's `default-members`; CMake runs its tests via
//! the `rust-test-eds` target.

pub mod backend;
pub mod connect;
pub mod marshal;
pub mod ops;
