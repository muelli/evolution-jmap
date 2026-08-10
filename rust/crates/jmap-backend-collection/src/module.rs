// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols `evolution-source-registry` resolves out of the built shared
//! object.
//!
//! The registry server is an `EDBusServer` whose `module_directory` is
//! libebackend's `registry-modules` — overridable at runtime with
//! `EDS_REGISTRY_MODULES`, which is what makes the manual test recipe possible
//! without installing into `/usr`. It wraps each `.so` it finds there in an
//! `EModule` and `g_type_module_use`s it; `EModule`'s `load` dlopens the file,
//! looks up `e_module_load`, and calls it with itself as the `GTypeModule`.
//! Whatever types are registered against that module by the time the call
//! returns are the module's contribution — for us, the backend and the factory
//! that builds it.
//!
//! Both entry points are guarded. Nothing here should be able to panic, but a
//! panic that unwound out of `e_module_load` would abort the one process that
//! owns every data source in the session — every account, of every kind, not
//! just this one.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies; the `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them live in
//! the companion `jmap-backend-collection-module` crate, which is built as a
//! `cdylib` and nothing else. This crate is the one where the distinction was
//! forced: `jmap-config` dev-depends on it, so both rlibs land in one test
//! binary, and a `no_mangle` function *is* its C symbol — the two definitions
//! collapsed into one entry point that answered for both crates.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapCollectionBackend;
use crate::factory::{JmapCollectionFactory, remember_backend_type};

/// Binds this project's gettext domain, and registers the backend and its
/// factory against `type_module`.
///
/// Called once per use of the module, not once per process: the registry unuses
/// a module when the last backend it provided goes away, which marks every type
/// it registered as unloaded, and calls this again when the next account wants
/// one. So registering is what happens on *every* call, and `register_dynamic`
/// is idempotent for exactly that reason. [`bind`] is idempotent too, and for
/// one more: a process can hold several of this repository's modules at once,
/// and each has to assume it might be the first.
///
/// The binding comes first because it has to be in place before anything can
/// ask for a translated string, and this is the only code of ours the registry
/// is guaranteed to run. This module is the one with the most to gain from it:
/// the child sources it creates carry the display names the user reads in
/// Evolution's sidebar under the account.
///
/// The backend then goes before the factory, because the factory's `class_init`
/// needs the type it produced.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` the registry passed to
/// `e_module_load`; it has to stay alive for the duration of the call.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        bind();
        // SAFETY: the module is the registry's, by this function's contract.
        unsafe {
            remember_backend_type(register_dynamic::<JmapCollectionBackend>(type_module));
            register_dynamic::<JmapCollectionFactory>(type_module);
        }
    });
}

/// Called just before the module is unloaded again.
///
/// There is nothing to undo: GLib unregisters the types the module registered on
/// its own, and this crate keeps no other process-wide state — the backend type
/// remembered for the factory is re-registered, and re-recorded, by the next
/// [`load`].
///
/// # Safety
///
/// As [`load`].
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
