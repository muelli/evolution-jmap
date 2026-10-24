// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shared head of the module entry points (`e_module_load` and
//! `camel_provider_module_init`).

use crate::trampoline::guard;

/// Runs `register` in a panic guard, after the setup of logging and of the
/// gettext binding. `context` names the entry point in the log.
///
/// A panic must not unwind into the C loader, because that process also
/// serves the other accounts of the user. [`crate::logging::init`] and
/// [`crate::i18n::bind`] are idempotent: a process can load more than one
/// module of this repository, and each module must operate as if it is the
/// first. The binding comes before `register`, because a type that `register`
/// makes visible can ask for a translated string immediately.
pub fn load(context: &str, register: impl FnOnce()) {
    guard(context, (), || {
        crate::logging::init();
        crate::i18n::bind();
        register();
    });
}
