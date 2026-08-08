// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP calendar backend (M4): the `ECalMetaBackend` side of the calendar,
//! layer by layer, mirroring `jmap-backend-book`.
//!
//! `jmap-cal-sync` already answers every question the meta backend asks, in
//! Rust terms, and `jmap-backend-core::source` reads the account out of an
//! `ESource`. What is left is the two ends of that pipe — and the calendar's C
//! end is the more dangerous of the two, which is why it comes first:
//!
//! - [`marshal`] converts between the Rust values and the C ones the vfuncs
//!   traffic in. Where the address book handed vCard *strings* across in both
//!   directions, here a component crosses as an `ICalComponent *`, a save
//!   arrives as a `GSList` of `ECalComponent *` that EDS still owns, and even
//!   the removals are `ECalMetaBackendInfo`s.
//!
//! The vfunc bodies, the subclass and the module entry point follow.
//!
//! Like `jmap-backend-core`, this crate needs the installed EDS headers and so
//! stays out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod marshal;
