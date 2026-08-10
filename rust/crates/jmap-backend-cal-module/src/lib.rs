// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `libecalbackendjmap.so`: the two C symbols `evolution-calendar-factory`
//! resolves, and nothing else.
//!
//! A `cdylib` and nothing else, holding two `#[unsafe(no_mangle)]` functions
//! that delegate to [`jmap_backend_cal::module`]. The full
//! argument for the split is in `jmap-backend-collection-module`, where the
//! collision that forced it happened; in one line: `e_module_load` is a fixed
//! name *every* module here has to export, a `no_mangle` function *is* its C
//! symbol, and two rlibs that both defined it in one link became one entry
//! point answering for both crates. A symbol that exists only in a shared
//! object cannot do that, because nothing links a cdylib.
//!
//! Nothing but delegation lives here, so there is nothing to drift from the
//! bodies the tests call — and no `guard`, because the bodies are guarded where
//! they are written.

use gobject_sys::GTypeModule;

/// `e_module_load`, as EDS resolves it.
///
/// # Safety
///
/// As [`jmap_backend_cal::module::load`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the caller's obligation is passed straight through.
    unsafe { jmap_backend_cal::module::load(type_module) }
}

/// `e_module_unload`, as EDS resolves it.
///
/// # Safety
///
/// As [`jmap_backend_cal::module::unload`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as `e_module_load`.
    unsafe { jmap_backend_cal::module::unload(type_module) }
}
