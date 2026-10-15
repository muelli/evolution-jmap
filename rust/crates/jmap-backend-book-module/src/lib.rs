// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `libebookbackendjmap.so`: C entry points resolved by `evolution-addressbook-factory`.
//!
//! Exposes `e_module_load` and `e_module_unload`, delegating to [`jmap_backend_book::module`].

use gobject_sys::GTypeModule;

/// `e_module_load`, as EDS resolves it.
///
/// # Safety
///
/// As [`jmap_backend_book::module::load`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the caller's obligation is passed straight through.
    unsafe { jmap_backend_book::module::load(type_module) }
}

/// `e_module_unload`, as EDS resolves it.
///
/// # Safety
///
/// As [`jmap_backend_book::module::unload`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as `e_module_load`.
    unsafe { jmap_backend_book::module::unload(type_module) }
}
