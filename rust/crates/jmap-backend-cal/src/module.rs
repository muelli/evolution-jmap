// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Module entry points executed when EDS loads `libecalbackendjmap.so`.
//!
//! [`load`] registers backend, factory, and OAuth 2.0 service. [`unload`] handles teardown.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapCalBackend;
use crate::factory::{JmapCalFactory, remember_backend_type};

/// Initializes logging and i18n, fills the zone table, and registers backend types on `type_module`.
///
/// # Safety
///
/// `type_module` must be a valid, live `GTypeModule` passed by EDS.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        jmap_backend_core::logging::init();
        bind();
        crate::marshal::load_builtin_zones();
        // SAFETY: `type_module` is a valid GTypeModule pointer per function contract.
        unsafe {
            remember_backend_type(register_dynamic::<JmapCalBackend>(type_module));
            register_dynamic::<JmapCalFactory>(type_module);
            jmap_config::oauth2_service::register(type_module);
        }
    });
}

/// Handles module unloading. GLib cleans up registered dynamic types.
///
/// # Safety
///
/// `type_module` must be a valid, live `GTypeModule` passed by EDS.
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
