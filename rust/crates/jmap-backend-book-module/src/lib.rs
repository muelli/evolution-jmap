// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `libebookbackendjmap.so`: C entry points for `evolution-addressbook-factory`.
//!
//! Exposes `e_module_load` and `e_module_unload`, delegating to [`jmap_backend_book::module`].

use gobject_sys::GTypeModule;

/// `e_module_load` resolved by EDS.
///
/// # Safety
///
/// As [`jmap_backend_book::module::load`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: Caller obligation forwards directly to module::load.
    unsafe { jmap_backend_book::module::load(type_module) }
}

/// `e_module_unload` resolved by EDS.
///
/// # Safety
///
/// As [`jmap_backend_book::module::unload`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: As e_module_load.
    unsafe { jmap_backend_book::module::unload(type_module) }
}
