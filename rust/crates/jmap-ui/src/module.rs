// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The part of the module load that belongs to this crate.
//!
//! This crate has no `.so` of its own. The `e_module_load` of
//! `jmap-config-module` calls [`load`] and `jmap_config::module::load`. Thus
//! the types of the two crates are in one module,
//! `module-jmap-configuration.so`, as in EWS.
//!
//! The `module` docs of jmap-config explain the extension timing ("Nothing is
//! registered *with* anything"). A type that is in the type system before
//! Evolution constructs the extensible *is* the connection.
//!
use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::{send_later, snooze, undo_delete, vacation};

/// Register the types of this crate against `type_module`.
///
/// A panic guard wraps this function, as it wraps all entry points that
/// Evolution calls.
///
/// `logging::init` and [`bind`] are idempotent, and
/// `jmap_config::module::load` also calls them. This function calls them
/// again because each load path must operate correctly when it is the first.
/// This also keeps the load correct if the two crates become separate again.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` that Evolution gave to
/// `e_module_load`. It must stay alive for the duration of the call.
pub unsafe fn load(type_module: *mut GTypeModule) {
    guard("e_module_load (jmap-ui)", (), || {
        jmap_backend_core::logging::init();
        bind();
        // SAFETY: the module is the module of Evolution, by the contract of this function.
        unsafe {
            // Register the page type before the extension that makes pages.
            // Then the name lookup in `page::create` always finds the type.
            register_dynamic::<vacation::page::VacationPage>(type_module);
            register_dynamic::<vacation::extension::JmapVacationExtension>(type_module);
            register_dynamic::<send_later::extension::JmapSendLaterExtension>(type_module);
            register_dynamic::<snooze::shell_ext::JmapSnoozeShellExtension>(type_module);
            register_dynamic::<snooze::browser_ext::JmapSnoozeBrowserExtension>(type_module);
            register_dynamic::<undo_delete::shell_ext::JmapUndoDeleteShellExtension>(type_module);
        }
        tracing::trace!(
            "jmap-ui loaded: vacation, scheduled send, snooze and undo of a delete registered"
        );
    });
}

/// Evolution calls this function immediately before it unloads the module.
/// There is nothing to undo, as in `jmap_config::module::unload`.
///
/// # Safety
///
/// As [`load`].
pub unsafe fn unload(_type_module: *mut GTypeModule) {}
