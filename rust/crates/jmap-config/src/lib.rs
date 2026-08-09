// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP account setup (M7): what Evolution's account editor writes, and
//! eventually the `module-jmap-configuration.so` it writes it from.
//!
//! Everything in this repository so far *reads* an account. `jmap-backend-book`
//! and `jmap-backend-cal` read the child source they are handed,
//! `jmap-backend-collection` reads the account source the registry hands it and
//! writes the children that hang off it — and every one of those tests starts
//! from a `.source` keyfile written by hand, because in Evolution the account
//! file itself is written by the setup UI and this project had none. That is
//! what this crate is for.
//!
//! - [`account`] is the first piece and the one the rest hangs off: the
//!   collection `ESource` an account *is*, written from what the user typed.
//!   It is the exact inverse of the collection backend's
//!   `collection_source`, which is not a coincidence and is not left to be
//!   one — `tests/account.rs` writes an
//!   account with this and reads it back with that, because two descriptions of
//!   one keyfile that are only checked separately are two descriptions that
//!   drift.
//!
//! ## Why an rlib with no module in it yet
//!
//! M7's deliverable is a GObject module Evolution dlopens out of *its* module
//! directory (`pkg-config --variable=moduledir evolution-shell-3.0`), one
//! directory over from the registry module M6 installs. But the parts of it
//! that decide anything — which properties an account is, and what the user's
//! answers turn into — need no Evolution headers at all: they are `ESource`
//! writes, and an `ESource` can be built and read back in a plain test with no
//! display, no session bus and no running Evolution.
//!
//! So those come first and are tested, and the `EMailConfigServiceBackend`
//! subclass that calls them comes after. The roadmap's rule about this
//! milestone is the reason for the order: GUI code cannot be verified on the
//! machine this is developed on, so the smaller the part of M7 that is GUI, the
//! more of M7 is actually *checked* rather than merely compiled.
//!
//! ## What is not here yet
//!
//! The three mail sources — `[Mail Account]`, `[Mail Identity]`,
//! `[Mail Transport]` — which are the sources
//! `jmap-backend-collection`'s `prepare_mail` fills in and which nothing yet
//! creates. They are a separate increment because they are separate sources:
//! they belong to the registry's own directory rather than to the collection's
//! cache (see that module for why they cannot be cached children), so writing
//! them is a different operation from writing the account, not a longer version
//! of it.
//!
//! Like the backends, this crate needs the installed EDS headers and so stays
//! out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod account;
