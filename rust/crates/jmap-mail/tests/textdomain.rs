// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The module binds this project's gettext domain before Camel can look up a
//! string in it.
//!
//! The provider carries a `translation_domain`. Camel calls `dgettext` with
//! it to display the provider's name and description, in the account
//! assistant's list of account types. This is the first thing a user sees
//! when setting up a JMAP account. `dgettext` looks in the directory the
//! domain is bound to. The process doing the lookup is Evolution or
//! `evolution-source-registry`, and neither knows about this project. So the
//! binding must happen in the only code of ours that is sure to run: the
//! entry point Camel dlopens the module for.
//!
//! The binding cannot happen lazily at the first lookup, because that lookup
//! belongs to Camel and has no hook. By the time Camel asks, the binding
//! already holds its answer.
//!
//! ## Why this is a test binary of its own
//!
//! The test starts from a deliberately wrong binding. Only the entry point
//! can make "the domain is bound where the build put the catalogues" true.
//! This is process-global state, and `bind` is a `OnceLock`. A sibling test
//! that reaches the entry point first would consume it, leaving the decoy in
//! place and failing this test for the wrong reason. Cargo gives each file in
//! `tests/` its own process. Being the only test in this file provides the
//! isolation.

use jmap_backend_core::i18n::{DOMAIN, LOCALE_DIR, bind_to, binding};
use jmap_mail::module::camel_provider_module_init;
use jmap_mail::provider::register;

/// Calling the entry point moves the domain's binding to the installed
/// catalogue directory.
///
/// The decoy directory makes this an assertion about the entry point, not
/// about the machine. On an uninstalled build, [`LOCALE_DIR`] is gettext's
/// own compiled-in default. A process that never bound anything would report
/// the same default, so the test could pass for a module that did nothing.
///
/// [`register`] must name the same domain, because the binding is worth
/// making only for strings that are looked up in it. The two values are one
/// constant now, but different libraries read them: Camel reads the provider
/// field, glibc holds the binding. Nothing else connects them.
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
    // there. The provider is leaked and never written to after registration.
    let named = unsafe { std::ffi::CStr::from_ptr(provider.translation_domain) };
    assert_eq!(named, DOMAIN, "the strings are looked up in another domain");
}
