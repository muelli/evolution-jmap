// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols that `evolution-source-registry` finds in the built shared
//! object.
//!
//! The registry server is an `EDBusServer`. Its `module_directory` is the
//! `registry-modules` directory of libebackend. `EDS_REGISTRY_MODULES` changes
//! it at runtime, so the manual test procedure needs no installation in `/usr`.
//! The server puts each `.so` in that directory into an `EModule` and calls
//! `g_type_module_use` on it. The `load` of `EModule` opens the file with
//! dlopen, finds `e_module_load`, and calls it with the `EModule` as the
//! `GTypeModule`. The types that this call registers against that module are
//! the contribution of the module. Here, they are the backend and its factory.
//!
//! Both entry points have a panic guard. Nothing here is expected to panic. But
//! a panic out of `e_module_load` would stop the process that owns all data
//! sources of the session, for all accounts of all types.
//!
//! ## Why the OAuth2 service also registers here
//!
//! `evolution-source-registry` (the process of this module) refreshes the
//! OAuth2 token of a JMAP account, not the account-setup module.
//! `EOAuth2ServiceBase` is an `EExtension` whose `extensible_type` is
//! `E_TYPE_OAUTH2_SERVICES` (see `e-oauth2-service-base.c`, which the
//! documentation of `jmap_config::oauth2_service` names). The constructor of
//! `EOAuth2Services` calls `e_extensible_load_extensions()` on itself. Thus a
//! process that later makes that singleton gets an instance of each service
//! that is registered at that time. This module needs no manual `g_object_new`
//! or `e_oauth2_services_add` call.
//!
//! Each process that uses `EOAuth2Services` must register the service itself.
//! A registration only in this process caused an infinite OAuth2 consent loop
//! in the book and calendar backends. evolution-ews also registers
//! `EOAuth2ServiceOffice365` with
//! `e_oauth2_service_office365_type_register(type_module)` in four modules:
//! `src/EWS/registry/module-ews-backend.c`,
//! `src/EWS/evolution/module-ews-configuration.c` (the shell process),
//! `src/EWS/addressbook/e-book-backend-ews-factory.c`, and
//! `src/EWS/calendar/e-cal-backend-ews-factory.c`. This project has the same
//! four modules: `jmap-config`, this crate, `jmap-backend-book` and
//! `jmap-backend-cal`. `EOAuth2ServiceOffice365` is a plugin type, as is the
//! JMAP service. `libedataserver` does not contain it.
//!
//! ## Why the `[JMAP OAuth2]` extension also registers here
//!
//! The documentation of `jmap_config::oauth2::ensure_registered` states that
//! the module load must call it. The `.source` keyfile parser of EDS restores
//! the extensions of an account with the same `e_source_get_extension` name
//! lookup as `jmap_config::oauth2::apply` and `read`. This process is the first
//! to parse a saved source from disk. `apply` and `read` register the type
//! themselves, but a module load calls neither. Without this registration, EDS
//! silently drops the `[JMAP OAuth2]` group of a `.source` file that it reads
//! here first. `tests/oauth2_extension.rs` tests this path:
//! `e_source_get_extension` on a new source, directly after the module load,
//! without a call to `apply` or `read`.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies. The `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them are in
//! the companion crate `jmap-backend-collection-module`, which builds only a
//! `cdylib`. This crate made the split necessary: `jmap-config` has a
//! dev-dependency on it, so both rlibs are in one test binary. A `no_mangle`
//! function is its C symbol, so the two definitions became one entry point for
//! both crates.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapCollectionBackend;
use crate::factory::{JmapCollectionFactory, remember_backend_type};

/// Sets up the `tracing` dispatcher and the gettext domain of this project.
/// Registers the backend, its factory and the OAuth2 service against
/// `type_module`.
///
/// The registry calls this for each use of the module, not one time for each
/// process. When the last backend of a module goes away, the registry stops
/// the use of the module, and all its types become unloaded. The registry
/// calls this again when the next account needs a backend. Thus each call
/// registers the types, and `register_dynamic` is idempotent for that reason.
/// [`jmap_backend_core::logging::init`] and [`bind`] are also idempotent,
/// because a process can load more than one module of this repository. Each
/// module must operate as if it is the first.
///
/// The binding comes first because it must exist before a request for a
/// translated string. The registry always runs this code. This module needs the
/// binding most: its child sources have the display names that the user sees
/// under the account in the Evolution sidebar.
///
/// The backend comes before the factory, because the `class_init` of the
/// factory needs the backend type.
///
/// [`jmap_config::oauth2::ensure_registered`] comes last and does not take
/// `type_module`. The `[JMAP OAuth2]` extension type has a static registration,
/// one time for the life of the process, as the `.source` extension types of
/// EDS do. The lookup of `e_source_get_extension` does not use the
/// `GTypeModule` of an `ESourceExtension` subclass. It only needs a type
/// registered under the requested name. This call is necessary here because
/// this process parses the `.source` file of a saved account before a call to
/// `jmap_config::oauth2::apply` or `read`.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` that the registry gave to
/// `e_module_load`. It must stay alive for the duration of the call.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        jmap_backend_core::logging::init();
        bind();
        // SAFETY: the contract of this function makes the module the module of
        // the registry.
        unsafe {
            remember_backend_type(register_dynamic::<JmapCollectionBackend>(type_module));
            register_dynamic::<JmapCollectionFactory>(type_module);
            jmap_config::oauth2_service::register(type_module);
        }
        jmap_config::oauth2::ensure_registered();
    });
}

/// The registry calls this immediately before it unloads the module.
///
/// There is nothing to undo. GLib unregisters the types of the module. This
/// crate keeps no other process-wide state that needs cleanup: the next
/// [`load`] registers and records the backend type for the factory again.
///
/// # Safety
///
/// As [`load`].
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
