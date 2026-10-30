// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Server Filters page in the Evolution account editor.
//!
//! JMAP has this function in RFC 9661 (`SieveScript`, one set of scripts
//! for each account, at most one active). Evolution has no equivalent and
//! no backend hook; the pattern is [`crate::vacation`].
//!
//! [`rules`] and [`script`] are the pure model: a rule, independent of
//! Sieve, and the generator and parser that turn a list of rules into
//! managed Sieve script text and back. Both have no FFI and are
//! unit-tested directly. The rest of the page (load and save over a
//! connected client, the widgets, the `EExtension`) follows in later
//! commits, the same way [`crate::vacation`] is laid out.

pub mod rules;
pub mod script;
