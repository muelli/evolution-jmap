// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared machinery for the JMAP backends that plug into Evolution Data
//! Server: the address book (M3), the calendar (M4) and the Camel provider
//! (M5) all subclass a GObject, get calls from C, get a `GCancellable`, and
//! must report failures as a `GError`.
//!
//! An error in each of those four things has the same symptom: a backend
//! that "sometimes" misbehaves. Thus they are here one time, with tests,
//! instead of three times by hand:
//!
//! - [`subclass`] registers a Rust-declared type with the GObject type system,
//!   statically or against a `GTypeModule`.
//! - [`instance`] lets such a type own a Rust value with a destructor.
//!   Without it, GObject's zero-then-free instance memory prevents this.
//! - [`trampoline`] stops a Rust panic from unwinding into C, which is
//!   undefined behaviour.
//! - [`cancel`] bridges a `GCancellable` to the client's [`CancelFlag`].
//! - [`connect`] is `connect_sync` without the collection it opens: the
//!   credentials, the `out_auth_result` classification, and the resolution
//!   of which server-side collection a source represents.
//! - [`error`] maps [`jmap_client::Error`] onto the `GError` domains
//!   and codes that Evolution uses for routing.
//! - [`marshal`] reads and writes the strings and lists in a vfunc's
//!   out-parameters. Every backend does this identically.
//! - [`resolver`] does the `_jmap._tcp` SRV lookup that RFC 8620 §2.2 wants.
//!   The client crate defines a seam for it but intentionally cannot do it.
//! - [`retry`] is the single "refresh a stale OAuth 2.0 token and retry once"
//!   control flow, generic over the backend and error type that call it.
//! - [`i18n`] binds this project's gettext domain, which is necessary to
//!   translate the strings that a user reads.
//! - [`push`] turns a server-pushed JMAP `StateChange` into the refresh that
//!   EDS already runs. [`weak`] lets its thread reach a backend it does not
//!   own, without a race against the backend's destruction.
//! - [`rebase`] is the per-account opt-in to rebase a session's advertised
//!   URLs onto the origin of the actual connection.
//!
//! [`CancelFlag`]: jmap_client::transport::CancelFlag
//!
//! This crate depends on `eds-sys` and thus on the installed EDS headers, so
//! it is not in the workspace's `default-members`. CMake runs its tests
//! through the `rust-test-eds` target.

pub mod api_token;
pub mod callback;
pub mod cancel;
pub mod connect;
pub mod error;
pub mod forever;
pub mod i18n;
pub mod instance;
pub mod logging;
pub mod marshal;
pub mod mime;
pub mod oauth2;
pub mod owned;
pub mod push;
pub mod rebase;
pub mod resolver;
pub mod resolver_thread;
pub mod retry;
pub mod secret_store;
pub mod setting_extension;
pub mod source;
pub mod subclass;
pub mod trampoline;
pub mod weak;
