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
use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::{send_later, snooze, vacation};

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
        // SAFETY: the module is Evolution's, by this function's contract.
        unsafe {
            // The page type before the extension that instantiates it, so
            // `page::create`'s name lookup cannot lose a race with the first
            // notebook.
            register_dynamic::<vacation::page::VacationPage>(type_module);
            register_dynamic::<vacation::extension::JmapVacationExtension>(type_module);
            register_dynamic::<send_later::extension::JmapSendLaterExtension>(type_module);
            register_dynamic::<snooze::shell_ext::JmapSnoozeShellExtension>(type_module);
            register_dynamic::<snooze::browser_ext::JmapSnoozeBrowserExtension>(type_module);
        }
        tracing::trace!("jmap-ui loaded: vacation, scheduled send and snooze registered");
    });
}

/// Called just before the module is unloaded; nothing to undo, as in
/// `jmap_config::module::unload`.
///
/// # Safety
///
/// As [`load`].
pub unsafe fn unload(_type_module: *mut GTypeModule) {}
