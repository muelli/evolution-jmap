// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Module entry points executed when EDS loads `libebookbackendjmap.so`.
//!
//! [`load`] registers the backend, factory, and OAuth 2.0 service; [`unload`] handles teardown.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapBookBackend;
use crate::factory::{JmapBookFactory, remember_backend_type};

/// Initializes logging and i18n, and registers backend types on `type_module`.
///
/// # Safety
///
/// `type_module` must be a valid, live `GTypeModule` passed by EDS.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        jmap_backend_core::logging::init();
        bind();
        // SAFETY: `type_module` is a valid GTypeModule pointer per caller contract.
        unsafe {
            remember_backend_type(register_dynamic::<JmapBookBackend>(type_module));
            register_dynamic::<JmapBookFactory>(type_module);
            jmap_config::oauth2_service::register(type_module);
        }
    });
}

/// Handles module unloading; GLib cleans up registered dynamic types.
///
/// # Safety
///
/// `type_module` must be a valid, live `GTypeModule` passed by EDS.
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
