// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols EDS resolves out of the built shared object.
//!
//! `evolution-addressbook-factory` walks its backend directory, wraps each
//! `.so` it finds in an `EModule`, and `g_type_module_use`s it; `EModule`'s
//! `load` dlopens the file, looks up `e_module_load`, and calls it with itself
//! as the `GTypeModule`. Whatever types are registered against that module by
//! the time the call returns are the module's contribution — for us, the
//! backend and the factory that builds it.
//!
//! Both entry points are guarded. Nothing here should be able to panic, but a
//! panic that unwound out of `e_module_load` would abort a process that is
//! also serving every other address book the user has.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies; the `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them live in
//! the companion `jmap-backend-book-module` crate, which is built as a
//! `cdylib` and nothing else. See that crate for why — briefly, every module in
//! this repository exports the same pair of symbols, a `no_mangle` function
//! *is* its C symbol, and two of these rlibs in one link therefore collapse
//! into one entry point that answers for both.
//!
//! ## Why the OAuth2 service registers here too
//!
//! An OAuth2 collection's `[Authentication] Method` names
//! `jmap_config::oauth2_service::NAME`, not the generic `"OAuth2"` string
//! (`jmap-config/src/backend.rs`'s `AUTH_CHOICES`), so
//! `jmap_backend_core::oauth2::source_uses_oauth2` resolves it through
//! `e_oauth2_services_is_oauth2_alias` — a query against the
//! `EOAuth2Services` singleton *of this process*, which only answers for a
//! service registered here. Unlike EWS's Office 365 service, which ships
//! inside `libedataserver` itself and so already exists in every process,
//! this project's service is a plugin that has to be registered into each
//! process that asks — `jmap-backend-collection`'s own module (the registry
//! process) and `jmap-config`'s (the shell process, `f83e04b`) already do
//! this for the processes they run in; `evolution-addressbook-factory`,
//! where every JMAP address book actually connects, was the remaining gap:
//! without it, `source_uses_oauth2` answered `false` here even for an
//! account whose token EDS could fetch just fine, so this backend fell back
//! to the password path and asked for credentials an OAuth2-only account
//! does not have.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapBookBackend;
use crate::factory::{JmapBookFactory, remember_backend_type};

/// Sets up this project's `tracing` dispatcher and gettext domain, and
/// registers the backend and its factory against `type_module`.
///
/// Called once per use of the module, not once per process: EDS unuses a
/// module when the last backend it provided goes away, which marks every type
/// it registered as unloaded, and calls this again when the next account wants
/// one. So registering is what happens on *every* call, and
/// `register_dynamic` is idempotent for exactly that reason.
/// [`jmap_backend_core::logging::init`] and [`bind`] are idempotent too, and
/// for one more: a process can hold several of this repository's modules at
/// once, and each has to assume it might be the first.
///
/// The binding comes first because it has to be in place before anything can
/// ask for a translated string, and this is the only code of ours that
/// `evolution-addressbook-factory` is guaranteed to run. Doing it lazily at the
/// first lookup would need us to own every lookup, and we do not — the strings
/// this backend hands back travel out in `GError`s that Evolution displays,
/// by which point the text has been chosen.
///
/// The backend then goes before the factory, because the factory's `class_init`
/// needs the type it produced.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` EDS passed to `e_module_load`; it
/// has to stay alive for the duration of the call.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        jmap_backend_core::logging::init();
        bind();
        // SAFETY: the module is EDS's, by this function's contract.
        unsafe {
            remember_backend_type(register_dynamic::<JmapBookBackend>(type_module));
            register_dynamic::<JmapBookFactory>(type_module);
            register_dynamic::<jmap_config::oauth2_service::Service>(type_module);
        }
    });
}

/// Called just before the module is unloaded again.
///
/// There is nothing to undo: GLib unregisters the types the module registered
/// on its own, and this crate keeps no other process-wide state — the backend
/// type remembered for the factory is re-registered, and re-recorded, by the
/// next [`load`].
///
/// # Safety
///
/// As [`load`].
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
