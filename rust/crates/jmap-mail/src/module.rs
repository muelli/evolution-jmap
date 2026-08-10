// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one symbol Camel resolves out of the built shared object.
//!
//! Camel's provider loader reads `libcameljmap.urls` beside the module to learn
//! that this object claims the `jmap` protocol, and dlopens the object only when
//! something asks for that protocol. Having opened it, it looks up
//! `camel_provider_module_init` and calls it with no arguments; whatever the
//! call registers through `camel_provider_register` is the module's entire
//! contribution.
//!
//! There is no counterpart to `e_module_unload` here. Camel never closes a
//! provider module — the provider struct it keeps names types that must stay
//! instantiable — so this is the whole of the C surface.

use jmap_backend_core::i18n::bind;
use jmap_backend_core::trampoline::guard;

/// Registers the JMAP provider with Camel, and binds the domain its strings
/// are translated in.
///
/// Called once per process in the field, but written to tolerate being reached
/// again: [`crate::provider::register`] is idempotent, and the alternative is a
/// second provider struct for the same protocol. [`bind`] is idempotent for the
/// same reason and one more — a process can hold several of this repository's
/// modules at once.
///
/// The binding comes first because the provider registered by the line after it
/// is immediately visible to Camel, and the provider's name and description are
/// looked up in that domain by Camel rather than by us. There is no later point
/// at which we are called and could still get in front of the first lookup.
///
/// Guarded, like every other C entry point in this repository. Nothing in here
/// should be able to panic, but a panic unwinding out of this symbol would
/// unwind into Camel's provider loader — inside `camel_provider_init`, in a
/// process that is also serving every other mail account the user has.
///
/// Safe rather than `unsafe`: Camel declares it as taking no arguments, so
/// there is no pointer whose validity the caller has to promise. The
/// declaration is still in scope through `eds-sys`, which is what makes this
/// definition a signature the compiler checks — `tests/provider.rs` pins that
/// the two are the same function.
#[unsafe(no_mangle)]
pub extern "C" fn camel_provider_module_init() {
    guard("camel_provider_module_init", (), || {
        bind();
        crate::provider::register();
    });
}
