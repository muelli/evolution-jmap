// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols EDS resolves out of the built shared object.
//!
//! `evolution-calendar-factory` walks its backend directory, wraps each `.so` it
//! finds in an `EModule`, and `g_type_module_use`s it; `EModule`'s `load`
//! dlopens the file, looks up `e_module_load`, and calls it with itself as the
//! `GTypeModule`. Whatever types are registered against that module by the time
//! the call returns are the module's contribution — for us, the backend and the
//! factory that builds it.
//!
//! The symbol names are the same two `jmap-backend-book` exports, which is fine
//! and in fact required: they are per-shared-object entry points, and each of
//! the two `.so`s is dlopened by a different factory process.
//!
//! Both entry points are guarded. Nothing here should be able to panic, but a
//! panic that unwound out of `e_module_load` would abort a process that is also
//! serving every other calendar the user has.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies; the `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them live in
//! the companion `jmap-backend-cal-module` crate, which is built as a `cdylib`
//! and nothing else. See that crate for why — briefly, a `no_mangle` function
//! *is* its C symbol, so the sentence above about the book backend exporting
//! the same two names stops being harmless the moment two of these rlibs meet
//! in one link.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapCalBackend;
use crate::factory::{JmapCalFactory, remember_backend_type};

/// Sets up this project's `tracing` dispatcher and gettext domain, and
/// registers the backend and its factory against `type_module`.
///
/// Called once per use of the module, not once per process: EDS unuses a module
/// when the last backend it provided goes away, which marks every type it
/// registered as unloaded, and calls this again when the next account wants one.
/// So registering is what happens on *every* call, and `register_dynamic` is
/// idempotent for exactly that reason. [`jmap_backend_core::logging::init`] and
/// [`bind`] are idempotent too, and for one more: a process can hold several of
/// this repository's modules at once, and each has to assume it might be the
/// first.
///
/// The binding comes first because it has to be in place before anything can
/// ask for a translated string, and this is the only code of ours that
/// `evolution-calendar-factory` is guaranteed to run. It is made here as well
/// as in the address book module rather than in one of them, because the two
/// shared objects are dlopened by different processes — a calendar-only account
/// never loads the book module.
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
            remember_backend_type(register_dynamic::<JmapCalBackend>(type_module));
            register_dynamic::<JmapCalFactory>(type_module);
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
