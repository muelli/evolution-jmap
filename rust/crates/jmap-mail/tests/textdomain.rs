// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! That the module binds this project's gettext domain before Camel can look
//! a string up in it.
//!
//! The provider carries a `translation_domain`, and Camel calls `dgettext`
//! with it whenever it displays the provider's name or description — in the
//! account assistant's list of account types, which is the first thing a user
//! setting up a JMAP account sees. `dgettext` looks in whatever directory the
//! domain is bound to, and the process doing the looking is Evolution or
//! `evolution-source-registry`, neither of which has ever heard of this
//! project. So the binding has to be made by the only code of ours that is
//! guaranteed to have run: the entry point Camel dlopened the module for.
//!
//! It has to be made *there* rather than lazily at the first lookup, because
//! that lookup is Camel's and there is no hook in it. By the time Camel asks,
//! the answer is already whatever the binding says.
//!
//! ## Why this is a test binary of its own
//!
//! It starts from a deliberately wrong binding, so that "the domain is bound
//! where the build put the catalogues" can only become true by the entry point
//! making it true. That is process-global state, and `bind` is a `OnceLock`
//! that a sibling test reaching the entry point first would spend — leaving
//! the decoy in place and failing this one for the wrong reason. Cargo gives
//! each file in `tests/` its own process; being the only test in this file is
//! the isolation.

use jmap_backend_core::i18n::{DOMAIN, LOCALE_DIR, bind_to, binding};
use jmap_mail::module::camel_provider_module_init;
use jmap_mail::provider::register;

/// Calling the entry point moves the domain's binding to the installed
/// catalogue directory.
///
/// The decoy directory is what makes this an assertion about the entry point
/// rather than about the machine. On an uninstalled build [`LOCALE_DIR`] is
/// gettext's own compiled-in default, so a process that had never bound
/// anything would report it too, and the test would pass against a module that
/// did nothing at all.
///
/// [`register`] is asserted to name the same domain because the binding is
/// only worth making for the strings that are looked up in it. The two are one
/// constant now, but they are read by different libraries — Camel reads the
/// provider field, glibc holds the binding — and nothing but this connects
/// them.
#[test]
fn the_entry_point_binds_the_domain_the_provider_names() {
    let decoy = c"/nonexistent/jmap-decoy-locale";
    assert_eq!(
        bind_to(decoy).as_c_str(),
        decoy,
        "the decoy binding took, so the assertion below can fail"
    );

    camel_provider_module_init();

    assert_eq!(
        binding().as_c_str(),
        LOCALE_DIR,
        "the entry point did not bind the domain, so the provider's name and \
         description would be looked up wherever the host process happened to \
         point"
    );

    let provider = register();
    assert!(!provider.translation_domain.is_null(), "no domain named");
    // SAFETY: the field is a 'static NUL-terminated string this crate put
    // there; the provider is leaked and never written to after registration.
    let named = unsafe { std::ffi::CStr::from_ptr(provider.translation_domain) };
    assert_eq!(named, DOMAIN, "the strings are looked up in another domain");
}
