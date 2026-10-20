// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `module-jmap-backend.so`: only the two C symbols that
//! `evolution-source-registry` resolves.
//!
//! # Why this is a separate crate
//!
//! The `load` function of `EModule` opens a shared object with `dlopen` and
//! looks up the symbol `e_module_load` by that exact name. Every module in
//! this repository exports the same pair of symbols. These modules are the
//! address book backend, the calendar backend, this backend, and the setup
//! module of Evolution. Separate shared objects load independently, so their
//! definitions never meet.
//!
//! In a static link, the definitions meet. A `#[unsafe(no_mangle)]` Rust
//! function *is* the C symbol, and its Rust path compiles to a call to that
//! symbol. If two rlibs each define `e_module_load`, the static link keeps
//! one definition. Every caller then reaches that definition, whichever crate
//! it meant to call.
//!
//! `jmap-config` dev-depends on `jmap-backend-collection`, because this
//! backend must read back unchanged the account that the setup UI writes.
//! Thus both rlibs are in one test binary. With `CARGO_INCREMENTAL=0`, the
//! link fails with `duplicate symbol`. With incremental codegen, the link can
//! succeed, and `jmap_config::module::e_module_load` then registers the types
//! of this crate. `tests/entry_points.rs` in `jmap-config` catches this.
//!
//! The solution: the C symbol belongs to the shared object, not to the
//! library. Each rlib exports ordinary Rust functions. Each `.so` comes from a
//! crate like this one, which is only a `cdylib`. Nothing links two cdylibs
//! together, so their symbols never meet.
//!
//! # Why the functions below only delegate
//!
//! The tests and EDS must call the same code. The bodies are in
//! [`jmap_backend_collection::module`], and the tests call them there. This
//! crate adds no behaviour. It also adds no `guard`: the bodies have their
//! guard, and a second guard is a second place to get the panic boundary
//! wrong.

use gobject_sys::GTypeModule;

/// The `e_module_load` symbol that the registry resolves.
///
/// # Safety
///
/// As for [`jmap_backend_collection::module::load`], which this function calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the obligation of the caller passes through unchanged.
    unsafe { jmap_backend_collection::module::load(type_module) }
}

/// The `e_module_unload` symbol that the registry resolves.
///
/// # Safety
///
/// As for [`jmap_backend_collection::module::unload`], which this function calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as for `e_module_load`.
    unsafe { jmap_backend_collection::module::unload(type_module) }
}
