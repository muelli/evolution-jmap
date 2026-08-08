// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared machinery for the JMAP backends that plug into Evolution Data
//! Server: the address book (M3), the calendar (M4) and the Camel provider
//! (M5) all subclass a GObject, are called from C, are handed a
//! `GCancellable`, and have to report failures as a `GError`.
//!
//! Doing each of those four things wrong has the same symptom — a backend
//! that "sometimes" misbehaves — so they live here once, with tests, instead
//! of three times by hand:
//!
//! - [`subclass`] registers a Rust-declared type with the GObject type system,
//!   statically or against a `GTypeModule`.
//! - [`trampoline`] stops a Rust panic from unwinding into C, which is
//!   undefined behaviour.
//! - [`cancel`] bridges a `GCancellable` to the client's [`CancelFlag`].
//! - [`error`] maps [`jmap_client::error::Error`] onto the `GError` domains
//!   and codes Evolution actually routes on.
//!
//! [`CancelFlag`]: jmap_client::transport::CancelFlag
//!
//! This crate depends on `eds-sys` and therefore on the installed EDS
//! headers, so it stays out of the workspace's `default-members`; CMake runs
//! its tests via the `rust-test-eds` target.

pub mod cancel;
pub mod error;
pub mod source;
pub mod subclass;
pub mod trampoline;
