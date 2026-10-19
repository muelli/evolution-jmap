// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `module-jmap-backend.so`: the two C symbols `evolution-source-registry`
//! resolves, and nothing else.
//!
//! # Why this is a separate crate
//!
//! `EModule`'s `load` function calls `dlopen` on a shared object, then looks
//! up the symbol `e_module_load` by that exact name. Every module this
//! repository installs exports the same pair of symbols: the address book
//! backend, the calendar backend, this one, and Evolution's setup module.
//! Between separate shared objects that is fine, and even required. Each
//! `.so` loads on its own, in its own process, so the definitions never meet.
//!
//! A static link is different: there the definitions do meet. A
//! `#[unsafe(no_mangle)]` Rust function is not a Rust function that also has
//! a C name. It *is* the C symbol, and the Rust path that names it compiles
//! to a call to that symbol. Two rlibs that each defined `e_module_load` do
//! not give a static link two entry points to choose between. They give it
//! one definition and one duplicate. Every caller then reaches whichever
//! definition remains, regardless of which crate it meant to call.
//!
//! This has happened in this repository. `jmap-config` dev-depends on
//! `jmap-backend-collection`, because the setup UI writes an account and
//! this backend must read it back unchanged. That dependency puts both
//! rlibs in one test binary. Under the linker's default codegen, the two
//! definitions land in one object, which is a hard `duplicate symbol`
//! error: CMake's `rust-test-eds` target hit it, running with
//! `CARGO_INCREMENTAL=0`. Under incremental codegen, one definition instead
//! landed in a codegen unit that nothing pulled out of the archive. The
//! link then succeeded, and `jmap_config::module::e_module_load` silently
//! registered this crate's types instead of its own. `jmap-config`'s
//! `tests/entry_points.rs` is the test that catches this.
//!
//! The fix: the C symbol belongs to the shared object, not to the library.
//! Each rlib exports ordinary Rust functions, and each `.so` is built from
//! a crate like this one, which is a `cdylib` and nothing else. Nothing
//! links two cdylibs together, so two of them never meet.
//!
//! # Why there is nothing below but two calls
//!
//! The old arrangement built both crate types from one source file, to keep
//! "the thing the tests call" and "the thing EDS calls" identical. This
//! crate keeps that property and strengthens it: the bodies live in
//! [`jmap_backend_collection::module`], the tests call them there, and what
//! is left here is a delegation with no behaviour of its own. There is
//! deliberately no `guard` here either. The bodies are guarded where they
//! are written; wrapping them a second time would only add a second place
//! to get the panic boundary wrong.

use gobject_sys::GTypeModule;

/// `e_module_load`, as the registry resolves it.
///
/// # Safety
///
/// As [`jmap_backend_collection::module::load`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the caller's obligation is passed straight through.
    unsafe { jmap_backend_collection::module::load(type_module) }
}

/// `e_module_unload`, as the registry resolves it.
///
/// # Safety
///
/// As [`jmap_backend_collection::module::unload`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as `e_module_load`.
    unsafe { jmap_backend_collection::module::unload(type_module) }
}
