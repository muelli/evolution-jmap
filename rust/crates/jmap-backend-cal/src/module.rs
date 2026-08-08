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

use gobject_sys::GTypeModule;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapCalBackend;
use crate::factory::{JmapCalFactory, remember_backend_type};

/// Registers the backend and its factory against `type_module`.
///
/// Called once per use of the module, not once per process: EDS unuses a module
/// when the last backend it provided goes away, which marks every type it
/// registered as unloaded, and calls this again when the next account wants one.
/// So registering is what happens on *every* call, and `register_dynamic` is
/// idempotent for exactly that reason.
///
/// The backend goes first, because the factory's `class_init` needs the type it
/// produced.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` EDS passed to this symbol; it has
/// to stay alive for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
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
/// [`e_module_load`].
///
/// # Safety
///
/// As [`e_module_load`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(_type_module: *mut GTypeModule) {}
