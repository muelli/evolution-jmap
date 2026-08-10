// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols Evolution resolves out of the built shared object.
//!
//! Evolution's shell calls `e_module_load_all_in_directory` over its own module
//! directory (`pkg-config --variable=moduledir evolution-shell-3.0`) at
//! startup, wraps each `.so` it finds in an `EModule` and `g_type_module_use`s
//! it; `EModule`'s `load` dlopens the file, resolves `e_module_load`, and calls
//! it with itself as the `GTypeModule`. Whatever types are registered against
//! that module by the time the call returns are the module's contribution — for
//! us, one: the [`EMailConfigServiceBackend`
//! subclass](crate::backend::JmapConfigServiceBackend).
//!
//! ## Nothing is registered *with* anything
//!
//! This is the part that differs from the three EDS modules this repository
//! already installs, and it is worth saying because the absence looks like an
//! omission. The address book, calendar and collection backends each register
//! a *factory* as well, because their hosts look a backend up by name in a
//! table the factory puts it in. Evolution's account editor has no such table.
//! `EMailConfigServicePage` is an `EExtensible`, and its `constructed` calls
//! `e_extensible_load_extensions`, which walks the children of `EExtension`
//! that exist at that moment and instantiates every one whose class
//! `extensible_type` is the page's own type. Our class inherits that field from
//! `EMailConfigServiceBackend`, so putting the type in the type system *is* the
//! registration — there is nowhere to add it and nobody to tell.
//!
//! The consequence is a timing one rather than a wiring one: the module has to
//! be loaded before the page is constructed, which is what the shell's
//! load-everything-at-startup arranges, and why this is a module in Evolution's
//! directory rather than something the mail provider brings with it.
//!
//! ## Guarded, like the others
//!
//! Nothing here should be able to panic, but a panic that unwound out of
//! `e_module_load` would cross into C from the process that owns the user's
//! whole session — the mail view, every open composer, every unsaved draft —
//! not just the account dialog that has yet to be opened.

use gobject_sys::GTypeModule;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapConfigServiceBackend;

/// Registers the setup backend against `type_module`.
///
/// Called once per use of the module, not once per process: GLib marks every
/// type a module registered as unloaded when the last user goes away, and calls
/// this again when the next one arrives. So registering is what happens on
/// *every* call, and [`register_dynamic`] is idempotent for exactly that
/// reason.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` Evolution passed to this symbol;
/// it has to stay alive for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        // SAFETY: the module is Evolution's, by this function's contract.
        unsafe { register_dynamic::<JmapConfigServiceBackend>(type_module) };
    });
}

/// Called just before the module is unloaded again.
///
/// There is nothing to undo: GLib unregisters the types the module registered
/// on its own, and this crate keeps no other process-wide state.
///
/// # Safety
///
/// As [`e_module_load`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e_module_unload(_type_module: *mut GTypeModule) {}
