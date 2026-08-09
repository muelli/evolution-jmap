// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw, unsafe FFI bindings to the Evolution *application* libraries
//! (`evolution-shell-3.0`, `evolution-mail-3.0`) — as [`eds-sys`] is to the
//! Evolution *Data Server* ones.
//!
//! Two libraries, two crates, because they are two different things to depend
//! on. The backends M3–M6 install are loaded by `evolution-source-registry` and
//! by the data factories, which are EDS processes that never link GTK; M7's
//! setup module is loaded by Evolution itself, which does. Generating both
//! surfaces into one crate would put GTK, WebKit and Evolution's own libraries
//! behind every address book backend that only ever wanted `ESource`.
//!
//! What is in here is deliberately one class wide: `EMailConfigServiceBackend`,
//! the `EExtension` Evolution's *Receiving Email* page instantiates one of per
//! known mail provider, and whose vfuncs are where an account setup gets to
//! say anything. The decisions those vfuncs make are already written and
//! tested, in plain Rust over an `ESource`, in [`jmap-config`]; this crate is
//! how they will be reached.
//!
//! Every GLib, EDS and Camel type these headers mention is re-exported from
//! [`eds-sys`] rather than regenerated, so an `ESource *` on this side of the
//! ABI is the same Rust type as an `ESource *` on that one. `build.rs` says
//! what that blocklist is and why GTK is the exception.
//!
//! Like [`eds-sys`], this crate is kept out of the workspace's
//! `default-members`: it needs Evolution's development headers, and `cargo
//! test` on a machine without them must still work. CMake runs its tests via
//! the `rust-test-eds` target.
//!
//! [`eds-sys`]: ../eds_sys/index.html
//! [`jmap-config`]: ../jmap_config/index.html

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// As in eds-sys: bindgen's output is not ours to lint.
#![allow(clippy::all)]
#![allow(rustdoc::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
