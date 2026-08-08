// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one symbol Camel resolves out of `libcameljmap.so`, and the provider it
//! registers.
//!
//! Camel does not find a mail provider the way EDS finds a backend. There is no
//! `EModule`, no `GTypeModule`, and no scan for subclasses of anything: on
//! `camel_provider_init()` Camel reads the `.urls` file next to each `.so` in
//! its provider directory, remembers which protocols that file claims, and only
//! when one of those protocols is asked for does it dlopen the module and call
//! `camel_provider_module_init`. The module's job in that call is to hand back
//! a `CamelProvider` — a plain struct, not an object — through
//! `camel_provider_register`.
//!
//! So the test drives the second half of that path directly: call the entry
//! point, then ask Camel for the protocol, which is exactly what
//! `camel_provider_get` does for a module it loaded itself. The first half —
//! that the `.urls` file next to the installed module names the same protocol —
//! is not something Camel can be asked about without an install tree, so
//! [`the_urls_file_claims_the_protocol_the_provider_registers`] reads the file
//! in the source tree instead, and CTest's `install-camel-provider` checks the
//! same file reaches the directory Camel scans.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_NUM_PROVIDER_TYPES, CAMEL_PROVIDER_IS_REMOTE, CAMEL_PROVIDER_IS_SOURCE,
    CAMEL_PROVIDER_IS_STORAGE, CAMEL_PROVIDER_STORE, CAMEL_PROVIDER_SUPPORTS_SSL,
    CAMEL_PROVIDER_TRANSPORT, CAMEL_URL_NEED_HOST, CamelProvider, CamelProviderFlags,
    CamelProviderURLFlags, GError, camel_offline_store_get_type, camel_provider_get,
    camel_provider_init, camel_store_get_type,
};
use glib_sys::GType;
use gobject_sys::{
    G_TYPE_INVALID, g_type_class_ref, g_type_class_unref, g_type_from_name, g_type_is_a,
};
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_mail::module::camel_provider_module_init;
use jmap_mail::provider::PROTOCOL;
use jmap_mail::store::JmapStore;

/// The entry point, called the way Camel calls it, and the provider read back
/// out of Camel's registry afterwards.
///
/// `camel_provider_init()` first, because that is the state a dlopened
/// `libcameljmap.so` finds itself in — Camel calls the entry point from inside
/// its own provider loading. Registering works without it, so this is about
/// standing where the module stands rather than about a dependency.
fn registered() -> &'static CamelProvider {
    // SAFETY: neither call takes arguments; `camel_provider_init` is
    // idempotent, and calling the entry point twice is what a second load of
    // the module would do anyway.
    unsafe {
        camel_provider_init();
        camel_provider_module_init();
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: PROTOCOL is a 'static NUL-terminated string, and `error` is a
    // location we own. The returned pointer is Camel's, and lives as long as
    // the process — the provider is never unregistered.
    let provider = unsafe { camel_provider_get(PROTOCOL.as_ptr(), &mut error) };
    assert!(
        !provider.is_null(),
        "camel_provider_module_init registered no provider for {PROTOCOL:?}"
    );
    assert!(error.is_null(), "camel_provider_get set an error");
    // SAFETY: checked non-NULL, and 'static as above.
    unsafe { &*provider }
}

/// The protocol is the name in the `.urls` file, the name in a `.source`
/// file's `[Mail Account] BackendName`, and the scheme Camel keys its provider
/// table by. Three places that have to agree, and this is the one they agree
/// on.
#[test]
fn the_entry_point_registers_a_provider_for_the_jmap_protocol() {
    let provider = registered();
    // SAFETY: a registered provider's protocol is the 'static string we gave
    // it.
    assert_eq!(unsafe { CStr::from_ptr(provider.protocol) }, PROTOCOL);
    assert!(!provider.name.is_null(), "the provider installed no name");
    assert!(
        !provider.description.is_null(),
        "the provider installed no description"
    );
}

/// `evolution-mail` only offers a provider in its account dialogs when the
/// domain is exactly `mail`. Any other string — or NULL — is a provider that
/// is registered, loadable, and invisible.
#[test]
fn the_provider_is_in_the_mail_domain_evolution_lists() {
    let provider = registered();
    assert!(!provider.domain.is_null(), "the provider has no domain");
    // SAFETY: 'static string, checked non-NULL.
    assert_eq!(unsafe { CStr::from_ptr(provider.domain) }, c"mail");
}

/// The flags decide what Evolution will let a JMAP account be. Missing
/// `IS_SOURCE` is an account that cannot receive mail; missing `IS_STORAGE` is
/// one whose folders never appear in the tree.
#[test]
fn the_provider_is_a_remote_mail_source_with_folders() {
    let provider = registered();
    for (flag, name) in [
        (CAMEL_PROVIDER_IS_REMOTE, "IS_REMOTE"),
        (CAMEL_PROVIDER_IS_SOURCE, "IS_SOURCE"),
        (CAMEL_PROVIDER_IS_STORAGE, "IS_STORAGE"),
        (CAMEL_PROVIDER_SUPPORTS_SSL, "SUPPORTS_SSL"),
    ] {
        assert_eq!(
            provider.flags & flag as CamelProviderFlags,
            flag as CamelProviderFlags,
            "the provider does not claim CAMEL_PROVIDER_{name}"
        );
    }
}

/// JMAP has no local flavour: an account is a session URL on some host, and
/// there is nothing to configure without one. `NEED_HOST` is what makes the
/// account dialog refuse an empty server rather than build a service that
/// fails on first use.
#[test]
fn the_provider_needs_a_host() {
    let provider = registered();
    assert_eq!(
        provider.url_flags & CAMEL_URL_NEED_HOST as CamelProviderURLFlags,
        CAMEL_URL_NEED_HOST as CamelProviderURLFlags
    );
}

/// The store slot is the whole point of the provider; the transport slot is
/// deliberately empty until `EmailSubmission/set` has a `CamelJmapTransport`
/// behind it. Empty means `G_TYPE_INVALID` — a zeroed slot Camel reads as "no
/// transport", not as a type it should try to instantiate.
#[test]
fn the_provider_names_a_store_and_no_transport_yet() {
    let provider = registered();
    assert_eq!(
        provider.object_types.len(),
        CAMEL_NUM_PROVIDER_TYPES as usize
    );

    let store = provider.object_types[CAMEL_PROVIDER_STORE as usize];
    assert_ne!(store, G_TYPE_INVALID, "the provider names no store type");
    assert_eq!(
        store,
        store_type(),
        "the provider names some other type than the store this module registers"
    );

    assert_eq!(
        provider.object_types[CAMEL_PROVIDER_TRANSPORT as usize], G_TYPE_INVALID,
        "a store-only provider must leave the transport slot invalid; sending \
         mail is a later increment and a half-registered transport type is a \
         crash at send time, not a missing feature"
    );
}

fn store_type() -> GType {
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapStore as ObjectSubclass>::NAME.as_ptr()) }
}

/// `CamelOfflineStore` rather than plain `CamelStore`, because the summary
/// cache has to keep working with the network down — that is the entire reason
/// the offline subclass exists, and picking the wrong parent is not something
/// a later increment can fix without moving every vfunc.
#[test]
fn the_store_is_an_offline_camel_store() {
    registered();
    let gtype = store_type();
    assert_ne!(gtype, 0, "the module registered no store type");
    // SAFETY: all three are registered types.
    unsafe {
        assert_ne!(
            g_type_is_a(gtype, camel_store_get_type()),
            0,
            "Camel instantiates object_types[STORE] as a CamelStore"
        );
        assert_ne!(
            g_type_is_a(gtype, camel_offline_store_get_type()),
            0,
            "the store must be a CamelOfflineStore for the offline cache"
        );
    }
}

/// Registering the type is not the same as its class surviving `class_init`.
/// GObject allocates the class from the `class_size` this crate reported, then
/// copies the parent class into its leading bytes — so a class struct that
/// does not really lead with `CamelOfflineStoreClass` is a heap overflow here
/// and nowhere earlier.
#[test]
fn the_stores_class_initialises() {
    registered();
    let gtype = store_type();
    assert_ne!(gtype, 0);
    // SAFETY: a registered type; the reference is given back below.
    unsafe {
        let class = g_type_class_ref(gtype);
        assert!(!class.is_null(), "the store class would not initialise");
        g_type_class_unref(class);
    }
}

/// Camel dlopens a provider module once per process, but nothing stops
/// `camel_provider_module_init` from being reached twice — a second
/// `camel_provider_init()` after a `camel_provider_load`, or a test suite.
/// The provider is a pointer Camel keeps, so a second call that built a second
/// struct would leave Camel's table pointing at one of them and every
/// `camel_provider_get` caller holding the other.
#[test]
fn loading_the_module_twice_registers_the_same_provider() {
    let first = ptr::from_ref(registered());
    let second = ptr::from_ref(registered());
    assert_eq!(
        first, second,
        "the entry point registered a second provider struct"
    );
}

/// Camel declares `camel_provider_module_init` and never defines it; the
/// definition is the module's side of that contract. Coercing both to the same
/// function pointer type is a compile-time check that the signature matches the
/// header — the same trick that keeps `e_module_load` honest, and the reason
/// the symbol is allowlisted in `eds-sys` at all.
#[test]
fn the_entry_point_has_the_signature_camel_declares() {
    let declared: unsafe extern "C" fn() = eds_sys::camel_provider_module_init;
    let ours: unsafe extern "C" fn() = camel_provider_module_init;
    assert_eq!(
        declared as usize, ours as usize,
        "the linker resolved Camel's declaration to something other than this \
         module's definition"
    );
}

/// The half of the load path a running Camel cannot be asked about.
///
/// Camel decides *whether* to dlopen `libcameljmap.so` by reading
/// `libcameljmap.urls` beside it and matching the protocol asked for against
/// the lines in that file. A file that says something else is a provider that
/// is never loaded, and the failure looks exactly like the module not being
/// installed. So the file's content is checked against the protocol the code
/// registers, rather than the two being written out twice and trusted.
///
/// One protocol per line, no comments: Camel takes each line verbatim as a
/// protocol name, so an SPDX header would register a provider called
/// `# SPDX-FileCopyrightText: ...`. The file is covered by `REUSE.toml`
/// instead.
#[test]
fn the_urls_file_claims_the_protocol_the_provider_registers() {
    let urls = include_str!("../libcameljmap.urls");
    let protocols: Vec<&str> = urls.lines().collect();
    assert_eq!(
        protocols,
        vec![PROTOCOL.to_str().expect("the protocol is not UTF-8")],
        "libcameljmap.urls does not list exactly the protocol the provider \
         registers"
    );
}
