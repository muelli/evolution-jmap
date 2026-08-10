// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `module-jmap-configuration.so`: the two C symbols Evolution's shell
//! resolves, and nothing else.
//!
//! A `cdylib` and nothing else, holding two `#[unsafe(no_mangle)]` functions
//! that delegate to [`jmap_config::module`]. The full argument for the split is
//! in `jmap-backend-collection-module`, where the collision that forced it
//! happened — and this crate's library is the other half of that collision: its
//! tests link `jmap-backend-collection`'s rlib, both rlibs defined
//! `e_module_load`, and in one link the two became one entry point answering
//! for both.
//!
//! Nothing but delegation lives here, so there is nothing to drift from the
//! bodies the tests call — and no `guard`, because the bodies are guarded where
//! they are written.

use gobject_sys::GTypeModule;

/// `e_module_load`, as Evolution's shell resolves it.
///
/// # Safety
///
/// As [`jmap_config::module::load`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    // SAFETY: the caller's obligation is passed straight through.
    unsafe { jmap_config::module::load(type_module) }
}

/// `e_module_unload`, as Evolution's shell resolves it.
///
/// # Safety
///
/// As [`jmap_config::module::unload`], which this is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(type_module: *mut GTypeModule) {
    // SAFETY: as `e_module_load`.
    unsafe { jmap_config::module::unload(type_module) }
}
