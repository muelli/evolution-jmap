// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `module-jmap-backend.so`: the two C symbols `evolution-source-registry`
//! resolves, and nothing else.
//!
//! # Why this is a separate crate
//!
//! `EModule`'s `load` dlopens a shared object and looks up `e_module_load` by
//! that exact name, so every module this repository installs — the address book
//! backend, the calendar backend, this one, and Evolution's setup module —
//! exports the same pair of symbols. Between shared objects that is fine and in
//! fact required: each `.so` is opened separately, by a different process, and
//! the definitions never meet.
//!
//! In a *static* link they do meet. A `#[unsafe(no_mangle)]` Rust function is
//! not a Rust function that also has a C name; it *is* the C symbol, and the
//! Rust path that names it compiles to a call to that symbol. So two rlibs that
//! each defined `e_module_load` did not give a link two entry points to choose
//! between — they gave it one definition and one duplicate, and every caller,
//! whichever crate it thought it was calling, reached whichever definition
//! survived.
//!
//! That is not hypothetical here. `jmap-config` dev-depends on
//! `jmap-backend-collection`, because the account the setup UI writes is only
//! right if this backend reads it back as the account that was written, so both
//! rlibs land in one test binary. Under the linker's default codegen the two
//! definitions land in one object and it is a hard `duplicate symbol` error
//! (which is what CMake's `rust-test-eds` saw, running with
//! `CARGO_INCREMENTAL=0`); under incremental codegen one of them happened to
//! land in a codegen unit nothing pulled out of the archive, the link succeeded,
//! and `jmap_config::module::e_module_load` silently registered *this* crate's
//! types. `jmap-config`'s `tests/entry_points.rs` is the test that says so.
//!
//! The fix is that the C symbol belongs to the shared object rather than to the
//! library: the rlibs export ordinary Rust functions, and each `.so` is built
//! from a crate like this one that is a `cdylib` and nothing else. Two of these
//! are never linked together, because nothing links a cdylib.
//!
//! # Why there is nothing below but two calls
//!
//! The old arrangement built both crate types from one source file so that "the
//! thing the tests call" and "the thing EDS calls" could not drift apart. That
//! property is kept, and strengthened: the bodies live in
//! [`jmap_backend_collection::module`], the tests call them there, and what is
//! left here is a delegation with no behaviour of its own to drift with. There
//! is deliberately no `guard` here either — the bodies are guarded where they
//! are written, and wrapping them twice would only mean two places to get the
//! panic boundary wrong.

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
