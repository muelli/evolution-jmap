// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! This crate's half of the module load.
//!
//! There is no `.so` of its own: `jmap-config-module`'s `e_module_load` calls
//! [`load`] beside `jmap_config::module::load`, so both crates' types ride in
//! `module-jmap-configuration.so` — one module, the EWS precedent. The
//! reasoning about extension timing is jmap-config's (its `module` docs,
//! "nothing is registered *with* anything"): putting a type in the type
//! system before Evolution constructs the extensible *is* the wiring.
//!
//! No extension types yet: each feature registers its `EExtension` here as it
//! lands — the vacation page first.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::trampoline::guard;

/// Register this crate's types against `type_module`, guarded like every
/// other entry point Evolution calls.
///
/// `logging::init` and [`bind`] are idempotent and also run in
/// `jmap_config::module::load`; repeated here because each load path has to
/// assume it might be the first (and stay correct if the two crates ever
/// part ways again).
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` Evolution passed to
/// `e_module_load`; it has to stay alive for the duration of the call.
pub unsafe fn load(type_module: *mut GTypeModule) {
    guard("e_module_load (jmap-ui)", (), || {
        jmap_backend_core::logging::init();
        bind();
        let _ = type_module;
        tracing::trace!("jmap-ui loaded; no extensions registered yet");
    });
}

/// Called just before the module is unloaded; nothing to undo, as in
/// `jmap_config::module::unload`.
///
/// # Safety
///
/// As [`load`].
pub unsafe fn unload(_type_module: *mut GTypeModule) {}
