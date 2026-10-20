// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw, unsafe FFI bindings to the Evolution Data Server backend libraries
//! (`libebackend-1.2`, `libedata-book-1.2`, `libedata-cal-2.0`) and to Camel
//! (`camel-1.2`), which contains the mail provider.
//!
//! Bindgen generates the bindings at build time from the installed EDS
//! headers. `build.rs` has the allowlist. This crate adds no safety, no
//! lifetimes and no `Drop`: [`jmap-backend-core`] supplies them. This crate
//! is the one place in the tree to audit against the C ABI.
//!
//! This crate re-exports the GLib, GObject and GIO types from `glib-sys`,
//! `gobject-sys` and `gio-sys` and does not regenerate them. Thus an
//! `EBookMetaBackend *` is a `GObject *` that the gtk-rs ecosystem also
//! recognises.
//!
//! The bindings come from the headers that pkg-config finds, so this crate is
//! not in the `default-members` of the workspace. `cargo test` must work on a
//! machine without EDS development files.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// Bindgen emits opaque structs without `#[doc = ...]` or safety docs, and
// unions and pointers that it cannot prove `Send`. This crate cannot fix them.
#![allow(clippy::all)]
#![allow(rustdoc::all)]

/// The EDS version that pkg-config resolved when `build.rs` chose the include
/// paths for these bindings.
///
/// Three versions apply: the headers give `EDS_MAJOR_VERSION` and its
/// siblings, the loaded library gives `eds_major_version` and its siblings,
/// and the `.pc` file gives this one. Only this one is not in the bindings.
/// With it, `tests/layout.rs` finds a `.pc` file that does not agree with its
/// headers, and a failed leg of the version matrix identifies its EDS. See
/// `docs/eds-versions.md`.
pub const EDS_HEADER_VERSION: &str = env!("EDS_HEADER_VERSION");

pub mod compat;

// Suppress the unnecessary_transmutes warnings that bindgen causes for the
// _IO_FILE bitfield accessor of glibc in newer container and rustc environments.
#[allow(unnecessary_transmutes)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
pub use generated::*;
