// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two symbols that Evolution resolves from the built shared object.
//!
//! At startup, the Evolution shell calls `e_module_load_all_in_directory` on
//! its module directory (`pkg-config --variable=moduledir evolution-shell-3.0`).
//! It puts each `.so` in an `EModule` and calls `g_type_module_use` on it. The
//! `load` of `EModule` dlopens the file, resolves `e_module_load`, and calls it
//! with the `EModule` as the `GTypeModule`. The types registered against that
//! module when the call returns are the contribution of the module. Here,
//! there are four:
//!
//! - the [`EMailConfigServiceBackend`
//!   subclass](crate::backend::JmapConfigServiceBackend),
//! - [`JmapConfigLookup`](crate::config_lookup::JmapConfigLookup), an
//!   `EConfigLookupWorker` that the "Look Up Account Details" step of the
//!   account assistant finds in the same way,
//! - the two [`ESourceConfigBackend`s](crate::source_config) that show JMAP in
//!   the New Address Book and New Calendar dialogs.
//!
//! The `EOAuth2Service` below is a fifth type (only if EDS has no
//! `EOAuth2ServiceDynamic`), registered for a different reason.
//!
//! ## Nothing is registered *with* anything
//!
//! This is different from the three EDS modules that this repository also
//! installs. It looks like an omission, so this section tells why. The address book,
//! calendar and collection backends also register a *factory*, because their
//! hosts find a backend by name in a table that the factory fills. The account
//! editor of Evolution has no such table. `EMailConfigServicePage` and
//! `EConfigLookup` are both `EExtensible`s. Their `constructed` calls
//! `e_extensible_load_extensions`, which examines the children of `EExtension`
//! that exist at that time. It creates each child whose class
//! `extensible_type` is the type of the extensible. The backend class of this
//! crate inherits that field from `EMailConfigServiceBackend`. The `class_init`
//! of `JmapConfigLookup` sets it explicitly to `E_TYPE_CONFIG_LOOKUP`. In both
//! cases, the type in the type system *is* the registration. There is no list
//! to add it to and no object to tell.
//!
//! The result is a timing requirement: the module must load before the page is
//! constructed. The shell loads all modules at startup, which satisfies this.
//! For this reason, this is a module in the Evolution directory and not part of
//! the mail provider.
//!
//! ## One item *is* registered here: the `[JMAP OAuth2]` extension type
//!
//! The doc of `jmap_config::oauth2::ensure_registered` names this module load as
//! the one caller that must think about it. `apply`/`read` register the
//! extension type of `[JMAP OAuth2]` themselves, so the reads and writes of the
//! account editor are safe. But the Evolution shell (this process) parses each
//! `ESource` from the registry into its own local objects with the same
//! `e_source_get_extension` name lookup. It does this before the account editor
//! opens a dialog on the source. If the type is not registered, the source
//! parsing of this process silently drops the `[JMAP OAuth2]` group of an
//! existing OAuth2 account (not only this crate leaves it unread).
//! `tests/oauth2_module.rs` tests this path.
//! The `[JMAP Pending Delete]` extension type registers here for the same reason.
//!
//! ## Guarded, as the others are
//!
//! No code here should panic. But a panic that unwinds out of `e_module_load`
//! crosses into C in the process that owns all of the user's session (the mail
//! view, each open composer, each unsaved draft), not only the account dialog,
//! which is not open yet.
//!
//! ## The C symbols are not here
//!
//! [`load`] and [`unload`] are the bodies. The `#[unsafe(no_mangle)]`
//! definitions of `e_module_load` and `e_module_unload` that call them are in
//! the companion `jmap-config-module` crate, which is only a `cdylib`. This
//! crate is one of the two crates where this split was necessary. Its tests
//! link the rlib of `jmap-backend-collection`, which defined the same two C
//! symbols at one time. A `no_mangle` function *is* its C symbol, so the two
//! definitions became one entry point for both crates.

use gobject_sys::GTypeModule;
use jmap_backend_core::module;
use jmap_backend_core::subclass::register_dynamic;

use crate::backend::JmapConfigServiceBackend;
use crate::config_lookup::JmapConfigLookup;
use crate::source_config::{JmapBookConfig, JmapCalConfig};

/// Configures the `tracing` dispatcher and gettext domain of this project
/// through [`module::load`], and registers the types of this module against
/// `type_module`.
///
/// GLib calls this once per use of the module, not once per process. When the
/// last user of the module stops, GLib marks each type that it registered as
/// unloaded, and calls this function again for the next user. Thus each call
/// registers again, and for this reason [`register_dynamic`] is idempotent.
///
/// The binding comes first because it must exist before code asks for a
/// translated string. This is the only code of this project that the Evolution
/// shell always runs. Of the five modules here, this is the one whose strings a
/// user reads in a dialog in normal operation, not only when an error occurs:
/// the labels of the account setup page, which
/// [`insert_widgets`](crate::backend) shows.
///
/// [`crate::oauth2::ensure_registered`] does not take `type_module`. Unlike the
/// backend above, the extension type of `[JMAP OAuth2]` is registered
/// statically, one time, for the life of the process, as the `.source`-parsing
/// extension types of EDS are. The module docs tell why this load path must
/// do it.
///
/// [`JmapConfigLookup`] is registered dynamically, as the backend is. Each
/// `EConfigLookup` (the account assistant of Evolution, not the `GTypeModule`
/// of this module) creates its own `EExtension` instance. But that module must
/// find the type, so the type must be registered against it before the
/// `e_extensible_load_extensions` call of the assistant. This is the same
/// "loaded before constructed" timing that the backend uses.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` that Evolution gave to
/// `e_module_load`. It must stay alive for the duration of the call.
pub unsafe extern "C" fn load(type_module: *mut GTypeModule) {
    module::load("e_module_load", || {
        // SAFETY: by the contract of this function, the module is Evolution's.
        unsafe {
            register_dynamic::<JmapConfigServiceBackend>(type_module);
            register_dynamic::<JmapConfigLookup>(type_module);
            // The OAuth2 service must also exist in THIS process, not only in
            // the source registry (the collection module registers it there).
            // The Evolution shell hosts the credentials prompter. Its OAuth2
            // prompter impl sends an `[Authentication] Method=JMAP` source to
            // the sign-in window only if an EOAuth2Service with that name is
            // registered in the process of the prompter. Without this line, the
            // prompter shows the password dialog. Seen live (2026-08-23,
            // Fastmail): a new discovered OAuth account asked for a password
            // that nothing could satisfy.
            crate::oauth2_service::register(type_module);
            // The two `ESourceConfigBackend`s: the New Address Book and New
            // Calendar dialogs show JMAP because of them (see
            // `crate::source_config`). They are registered here, not from a
            // separate module, because the Evolution shell already loads this
            // module from the same `moduledir`. `e_extensible_load_extensions`
            // finds an extension in the type system, not in a table. Thus the
            // registration before the dialog is constructed *is* the wiring.
            register_dynamic::<JmapBookConfig>(type_module);
            register_dynamic::<JmapCalConfig>(type_module);
        }
        crate::oauth2::ensure_registered();
        crate::pending_delete::ensure_registered();
    });
}

/// Called immediately before the module unloads.
///
/// There is nothing to undo. GLib unregisters the types of the module, and this
/// crate keeps no other process-wide state.
///
/// # Safety
///
/// As [`load`].
pub unsafe extern "C" fn unload(_type_module: *mut GTypeModule) {}
