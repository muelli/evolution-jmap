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
use jmap_backend_core::module;
use jmap_backend_core::subclass::register_dynamic;

use crate::{external_photo, filters, send_later, snooze, undo_delete, vacation};

/// Register the types of this crate against `type_module`.
///
/// [`module::load`] supplies the panic guard, logging and the gettext binding.
/// `jmap_config::module::load` also calls it. This load calls it again, so
/// that each load path operates correctly when it is the first.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` that Evolution gave to
/// `e_module_load`. It must stay alive for the duration of the call.
pub unsafe fn load(type_module: *mut GTypeModule) {
    module::load("e_module_load (jmap-ui)", || {
        // SAFETY: the module is the module of Evolution, by the contract of this function.
        unsafe {
            // Register the page type before the extension that makes pages.
            // Then the name lookup in `page::create` always finds the type.
            register_dynamic::<vacation::page::VacationPage>(type_module);
            register_dynamic::<vacation::extension::JmapVacationExtension>(type_module);
            register_dynamic::<filters::page::FiltersPage>(type_module);
            register_dynamic::<filters::extension::JmapFiltersExtension>(type_module);
            register_dynamic::<send_later::extension::JmapSendLaterExtension>(type_module);
            register_dynamic::<snooze::reader_ext::JmapSnoozeReaderExtension>(type_module);
            register_dynamic::<undo_delete::shell_ext::JmapUndoDeleteShellExtension>(type_module);
            register_dynamic::<external_photo::display_ext::JmapExternalPhotoExtension>(
                type_module,
            );
        }
        tracing::trace!(
            "jmap-ui loaded: vacation, server filters, scheduled send, snooze, undo of a delete and the external photo placeholder registered"
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
