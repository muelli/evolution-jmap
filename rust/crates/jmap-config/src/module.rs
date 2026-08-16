// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols Evolution resolves out of the built shared object.
//!
//! Evolution's shell calls `e_module_load_all_in_directory` over its own module
//! directory (`pkg-config --variable=moduledir evolution-shell-3.0`) at
//! startup, wraps each `.so` it finds in an `EModule` and `g_type_module_use`s
//! it; `EModule`'s `load` dlopens the file, resolves `e_module_load`, and calls
//! it with itself as the `GTypeModule`. Whatever types are registered against
//! that module by the time the call returns are the module's contribution —
//! for us, two: the [`EMailConfigServiceBackend`
//! subclass](crate::backend::JmapConfigServiceBackend), and
//! [`JmapConfigLookup`](crate::config_lookup::JmapConfigLookup), an
//! `EConfigLookupWorker` the account assistant's "Look Up Account Details"
//! step finds the same way.
//!
//! ## Nothing is registered *with* anything
//!
//! This is the part that differs from the three EDS modules this repository
//! already installs, and it is worth saying because the absence looks like an
//! omission. The address book, calendar and collection backends each register
//! a *factory* as well, because their hosts look a backend up by name in a
//! table the factory puts it in. Evolution's account editor has no such table.
//! `EMailConfigServicePage` and `EConfigLookup` are both `EExtensible`s whose
//! `constructed` calls `e_extensible_load_extensions`, which walks the
//! children of `EExtension` that exist at that moment and instantiates every
//! one whose class `extensible_type` is the extensible's own type. Our
//! backend class inherits that field from `EMailConfigServiceBackend`, and
//! `JmapConfigLookup`'s own `class_init` sets it explicitly to
//! `E_TYPE_CONFIG_LOOKUP` — either way, putting the type in the type system
//! *is* the registration; there is nowhere to add it and nobody to tell.
//!
//! The consequence is a timing one rather than a wiring one: the module has to
//! be loaded before the page is constructed, which is what the shell's
//! load-everything-at-startup arranges, and why this is a module in Evolution's
//! directory rather than something the mail provider brings with it.
//!
//! ## One thing *is* registered here: the `[JMAP OAuth2]` extension type
//!
//! `jmap_config::oauth2::ensure_registered`'s own doc names this module load
//! as the one caller that has to think about it. `apply`/`read` register
//! `[JMAP OAuth2]`'s extension type themselves, so the account editor's own
//! reads and writes are covered — but Evolution's shell (this process) parses
//! every `ESource` it receives from the registry into its own local objects
//! using the same `e_source_get_extension` name lookup, and it does that
//! before the account editor ever opens a dialog on one. Left unregistered,
//! an existing OAuth2 account's `[JMAP OAuth2]` group would be silently
//! dropped by this process's own source parsing, not merely unread by this
//! crate's code. `tests/oauth2_module.rs` drives exactly that path.
//!
//! ## Guarded, like the others
//!
//! Nothing here should be able to panic, but a panic that unwound out of
//! `e_module_load` would cross into C from the process that owns the user's
//! whole session — the mail view, every open composer, every unsaved draft —
//! not just the account dialog that has yet to be opened.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies; the `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them live in
//! the companion `jmap-config-module` crate, which is built as a `cdylib` and
//! nothing else. This crate is one of the two where the distinction was forced:
//! its tests link `jmap-backend-collection`'s rlib, which used to define the
//! same two C symbols, and a `no_mangle` function *is* its C symbol — so the
//! two definitions collapsed into one entry point that answered for both.

use gobject_sys::GTypeModule;
use jmap_backend_core::i18n::bind;
use jmap_backend_core::subclass::register_dynamic;
use jmap_backend_core::trampoline::guard;

use crate::backend::JmapConfigServiceBackend;
use crate::config_lookup::JmapConfigLookup;

/// Binds this project's gettext domain, and registers the setup backend against
/// `type_module`.
///
/// Called once per use of the module, not once per process: GLib marks every
/// type a module registered as unloaded when the last user goes away, and calls
/// this again when the next one arrives. So registering is what happens on
/// *every* call, and [`register_dynamic`] is idempotent for exactly that
/// reason. [`bind`] is idempotent too, and for one more: a process can hold
/// several of this repository's modules at once — Evolution loads this one and
/// uses the Camel provider in the same address space — and each has to assume
/// it might be the first.
///
/// The binding comes first because it has to be in place before anything can
/// ask for a translated string, and this is the only code of ours Evolution's
/// shell is guaranteed to run. Of the five modules here this is the one whose
/// strings a user reads while *looking* at a dialog rather than while something
/// goes wrong: the account setup page's labels, which
/// [`insert_widgets`](crate::backend) has yet to put on screen.
///
/// [`crate::oauth2::ensure_registered`] does not take `type_module`: unlike
/// the backend above, `[JMAP OAuth2]`'s extension type is registered
/// statically, once, for the life of the process, the same way EDS's own
/// `.source`-parsing extension types are — see the module docs for why this
/// load path has to be the one to do it.
///
/// [`JmapConfigLookup`] is registered dynamically, like the backend: its
/// `EExtension` is instantiated per `EConfigLookup` (Evolution's account
/// assistant, not this module's own `GTypeModule`), but it still has to be a
/// type that module can find, which means registered against it before the
/// assistant's own `e_extensible_load_extensions` walk runs — the same
/// loaded-before-constructed timing the backend relies on.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` Evolution passed to
/// `e_module_load`; it has to stay alive for the duration of the call.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    guard("e_module_load", (), || {
        bind();
        // SAFETY: the module is Evolution's, by this function's contract.
        unsafe {
            register_dynamic::<JmapConfigServiceBackend>(type_module);
            register_dynamic::<JmapConfigLookup>(type_module);
        }
        crate::oauth2::ensure_registered();
    });
}

/// Called just before the module is unloaded again.
///
/// There is nothing to undo: GLib unregisters the types the module registered
/// on its own, and this crate keeps no other process-wide state.
///
/// # Safety
///
/// As [`load`].
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
