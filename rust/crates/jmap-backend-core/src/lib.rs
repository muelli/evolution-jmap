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
//! - [`instance`] lets such a type own a Rust value with a destructor, which
//!   GObject's zero-it-then-free-it instance memory otherwise does not allow.
//! - [`trampoline`] stops a Rust panic from unwinding into C, which is
//!   undefined behaviour.
//! - [`cancel`] bridges a `GCancellable` to the client's [`CancelFlag`].
//! - [`connect`] is `connect_sync` minus the collection it opens: the
//!   credentials, the `out_auth_result` classification and the resolution of
//!   which server-side collection a source stands for.
//! - [`error`] maps [`jmap_client::Error`] onto the `GError` domains
//!   and codes Evolution actually routes on.
//! - [`marshal`] reads and writes the strings and lists a vfunc's
//!   out-parameters carry, which every backend does identically.
//! - [`resolver`] performs the `_jmap._tcp` SRV lookup RFC 8620 §2.2 wants,
//!   which the client crate defines a seam for but deliberately cannot do.
//! - [`i18n`] binds this project's gettext domain, so that the strings a user
//!   reads can be translated at all.
//!
//! [`CancelFlag`]: jmap_client::transport::CancelFlag
//!
//! This crate depends on `eds-sys` and therefore on the installed EDS
//! headers, so it stays out of the workspace's `default-members`; CMake runs
//! its tests via the `rust-test-eds` target.

pub mod api_token;
pub mod cancel;
pub mod connect;
pub mod error;
pub mod i18n;
pub mod instance;
pub mod logging;
pub mod marshal;
pub mod oauth2;
pub mod owned;
pub mod resolver;
pub mod secret_store;
pub mod source;
pub mod subclass;
pub mod trampoline;
