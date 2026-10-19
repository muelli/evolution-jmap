// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `libecalbackendjmap.so`: C entry points resolved by `evolution-calendar-factory`.
//!
//! Exposes `e_module_load` and `e_module_unload`, delegating to [`jmap_backend_cal::module`].

use gobject_sys::GTypeModule;

/// Implements `e_module_load` resolved by EDS.
///
/// # Safety
///
/// Follows safety requirements of [`jmap_backend_cal::module::load`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: Caller safety obligations forward directly to jmap_backend_cal::module::load.
    unsafe { jmap_backend_cal::module::load(type_module) }
}

/// Implements `e_module_unload` resolved by EDS.
///
/// # Safety
///
/// Follows safety requirements of [`jmap_backend_cal::module::unload`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: Caller safety obligations forward directly to jmap_backend_cal::module::unload.
    unsafe { jmap_backend_cal::module::unload(type_module) }
}
