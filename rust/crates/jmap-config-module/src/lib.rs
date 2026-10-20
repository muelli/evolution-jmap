// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `module-jmap-configuration.so`: only the two C symbols that the Evolution
//! shell resolves.
//!
//! This is only a `cdylib`. It holds two `#[unsafe(no_mangle)]` functions that
//! call [`jmap_config::module`] and [`jmap_ui::module`]. The account setup
//! extensions and the UI-feature extensions are in one module, as in
//! evolution-ews. `jmap-backend-collection-module` gives the full reason for
//! the split, because the collision that made it necessary occurred there. The
//! library of `jmap-config` is the other half of that collision: its tests
//! link the rlib of `jmap-backend-collection`. Both rlibs defined
//! `e_module_load`, so in one link the two became one entry point for both.
//!
//! This crate only delegates, so it cannot drift from the bodies that the
//! tests call. It needs no `guard`, because the bodies have their own guards.

use gobject_sys::GTypeModule;

/// `e_module_load`, as the Evolution shell resolves it.
///
/// # Safety
///
/// As [`jmap_config::module::load`], which this function calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the obligation of the caller passes directly through.
    unsafe {
        jmap_config::module::load(type_module);
        jmap_ui::module::load(type_module);
    }
}

/// `e_module_unload`, as the Evolution shell resolves it.
///
/// # Safety
///
/// As [`jmap_config::module::unload`], which this function calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as `e_module_load`.
    unsafe {
        jmap_config::module::unload(type_module);
        jmap_ui::module::unload(type_module);
    }
}
