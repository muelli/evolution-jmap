// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one symbol that Camel resolves out of `libcameljmap.so`, and the
//! provider it registers.
//!
//! Camel does not find a mail provider the way EDS finds a backend. There is
//! no `EModule`, no `GTypeModule`, and no scan for subclasses of anything. On
//! `camel_provider_init()`, Camel reads the `.urls` file next to each `.so`
//! in its provider directory and remembers which protocols that file claims.
//! Only when one of those protocols is asked for does Camel dlopen the
//! module and call `camel_provider_module_init`. In that call, the module's
//! job is to hand back a `CamelProvider` (a plain struct, not an object)
//! through `camel_provider_register`.
//!
//! The tests here drive the second half of that path directly: call the
//! entry point, then ask Camel for the protocol. This is exactly what
//! `camel_provider_get` does for a module it loaded itself. The first half,
//! that the `.urls` file next to the installed module names the same
//! protocol, is not something a running Camel can be asked about without an
//! install tree. So
//! [`the_urls_file_claims_the_protocol_the_provider_registers`] reads the
//! file in the source tree instead, and CTest's `install-camel-provider`
//! check confirms the same file reaches the directory Camel scans.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_NUM_PROVIDER_TYPES, CAMEL_PROVIDER_IS_EXTERNAL, CAMEL_PROVIDER_IS_REMOTE,
    CAMEL_PROVIDER_IS_SOURCE, CAMEL_PROVIDER_IS_STORAGE, CAMEL_PROVIDER_STORE,
    CAMEL_PROVIDER_SUPPORTS_SSL, CAMEL_PROVIDER_TRANSPORT, CAMEL_URL_ALLOW_AUTH,
    CAMEL_URL_ALLOW_PASSWORD, CAMEL_URL_ALLOW_PATH, CAMEL_URL_ALLOW_PORT, CAMEL_URL_ALLOW_USER,
    CAMEL_URL_NEED_HOST, CamelProvider, CamelProviderFlags, CamelProviderURLFlags,
    CamelTransportClass, GError, camel_offline_store_get_type, camel_provider_get,
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
use jmap_mail::transport::JmapTransport;

/// Calls the entry point the way Camel calls it, then reads the provider
/// back out of Camel's registry.
///
/// Calls `camel_provider_init()` first, because that is the state a
/// dlopened `libcameljmap.so` finds itself in: Camel calls the entry point
/// from inside its own provider loading. Registering works without this
/// call too. The call matches the module's real starting state; it is not
/// a dependency the module needs.
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
    // location this code owns. The returned pointer belongs to Camel and
    // lives as long as the process, because the provider is never
    // unregistered.
    let provider = unsafe { camel_provider_get(PROTOCOL.as_ptr(), &mut error) };
    assert!(
        !provider.is_null(),
        "camel_provider_module_init registered no provider for {PROTOCOL:?}"
    );
    assert!(error.is_null(), "camel_provider_get set an error");
    // SAFETY: checked non-NULL, and 'static as above.
    unsafe { &*provider }
}

/// The protocol name must agree in three places: the `.urls` file, a
/// `.source` file's `[Mail Account] BackendName`, and the scheme Camel keys
/// its provider table by. This test checks the value they agree on.
#[test]
fn the_entry_point_registers_a_provider_for_the_jmap_protocol() {
    let provider = registered();
    // SAFETY: a registered provider's protocol is the 'static string this
    // test gave it.
    assert_eq!(unsafe { CStr::from_ptr(provider.protocol) }, PROTOCOL);
    assert!(!provider.name.is_null(), "the provider installed no name");
    assert!(
        !provider.description.is_null(),
        "the provider installed no description"
    );
}

/// `evolution-mail` only offers a provider in its account dialogs when the
/// domain is exactly `mail`. Any other string, or NULL, still registers and
/// loads the provider, but hides it from the account dialogs.
#[test]
fn the_provider_is_in_the_mail_domain_evolution_lists() {
    let provider = registered();
    assert!(!provider.domain.is_null(), "the provider has no domain");
    // SAFETY: 'static string, checked non-NULL.
    assert_eq!(unsafe { CStr::from_ptr(provider.domain) }, c"mail");
}

/// These flags decide what Evolution lets a JMAP account do. Without
/// `IS_SOURCE`, the account cannot receive mail. Without `IS_STORAGE`, its
/// folders never appear in the tree. Without `IS_EXTERNAL`, the store wrongly
/// claims to come from the mail component's own New Mail Account wizard.
/// Every JMAP mail store in fact comes from a collection account's `ESource`
/// extensions.
#[test]
fn the_provider_is_a_remote_mail_source_with_folders() {
    let provider = registered();
    for (flag, name) in [
        (CAMEL_PROVIDER_IS_REMOTE, "IS_REMOTE"),
        (CAMEL_PROVIDER_IS_EXTERNAL, "IS_EXTERNAL"),
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

/// JMAP has no local flavour: an account is a session URL on some host, with
/// nothing to configure without one. `NEED_HOST` makes the account dialog
/// refuse an empty server, instead of building a service that fails on first
/// use. The `ALLOW_*` flags are the URL parts a JMAP account can carry: a
/// path (the session URL), and the credential parts. This backend reads
/// credentials from `ESourceAuthentication` and libsecret, not the URL, but
/// the flag still must permit them.
#[test]
fn the_provider_allows_the_url_parts_jmap_uses() {
    let provider = registered();
    for (flag, name) in [
        (CAMEL_URL_NEED_HOST, "NEED_HOST"),
        (CAMEL_URL_ALLOW_PORT, "ALLOW_PORT"),
        (CAMEL_URL_ALLOW_PATH, "ALLOW_PATH"),
        (CAMEL_URL_ALLOW_USER, "ALLOW_USER"),
        (CAMEL_URL_ALLOW_AUTH, "ALLOW_AUTH"),
        (CAMEL_URL_ALLOW_PASSWORD, "ALLOW_PASSWORD"),
    ] {
        assert_eq!(
            provider.url_flags & flag as CamelProviderURLFlags,
            flag as CamelProviderURLFlags,
            "the provider does not claim CAMEL_URL_{name}"
        );
    }
}

/// Both slots make a JMAP account a complete account. The store holds the
/// mail, and the transport sends it. Camel instantiates each from the type
/// named here. An empty slot is `G_TYPE_INVALID`, which Camel reads as
/// "this provider cannot do that". A transport slot left invalid gives
/// Evolution no way to send mail. A transport type whose `send_to_sync` is
/// NULL causes a GLib critical the first time the user presses Send.
#[test]
fn the_provider_names_a_store_and_a_transport() {
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

    let transport = provider.object_types[CAMEL_PROVIDER_TRANSPORT as usize];
    assert_ne!(
        transport, G_TYPE_INVALID,
        "the provider names no transport type"
    );
    assert_eq!(
        transport,
        transport_type(),
        "the provider names some other type than the transport this module registers"
    );

    // Camel dispatches sending through the class. A transport type without
    // the vfunc is worse than no transport type at all, so this precondition
    // must hold before the slot is filled.
    //
    // SAFETY: a live GType of a class that has one. The code returns the
    // reference afterward.
    unsafe {
        let class = g_type_class_ref(transport).cast::<CamelTransportClass>();
        assert!(
            (*class).send_to_sync.is_some(),
            "the transport the provider names cannot send"
        );
        g_type_class_unref(class.cast());
    }
}

fn transport_type() -> GType {
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapTransport as ObjectSubclass>::NAME.as_ptr()) }
}

fn store_type() -> GType {
    // SAFETY: NAME is a 'static NUL-terminated string.
    unsafe { g_type_from_name(<JmapStore as ObjectSubclass>::NAME.as_ptr()) }
}

/// `CamelOfflineStore` rather than plain `CamelStore`, because the summary
/// cache must keep working while the network is down. That is the entire
/// reason the offline subclass exists. Picking the wrong parent here is not
/// something a later change can fix without moving every vfunc.
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

/// Registering the type does not prove that its class survives `class_init`.
/// GObject allocates the class from the `class_size` this crate reported,
/// then copies the parent class into its leading bytes. If the class struct
/// does not truly lead with `CamelOfflineStoreClass`, this copy causes a
/// heap overflow here, not at an earlier point.
#[test]
fn the_stores_class_initialises() {
    registered();
    let gtype = store_type();
    assert_ne!(gtype, 0);
    // SAFETY: a registered type. The code returns the reference below.
    unsafe {
        let class = g_type_class_ref(gtype);
        assert!(!class.is_null(), "the store class would not initialise");
        g_type_class_unref(class);
    }
}

/// Camel dlopens a provider module once per process, but nothing stops
/// `camel_provider_module_init` from running twice: a second
/// `camel_provider_init()` after a `camel_provider_load`, or a test suite,
/// can both reach it again. Camel keeps a pointer to the provider. If a
/// second call built a second struct, Camel's table would point at one of
/// them, and some `camel_provider_get` callers would hold the other.
#[test]
fn loading_the_module_twice_registers_the_same_provider() {
    let first = ptr::from_ref(registered());
    let second = ptr::from_ref(registered());
    assert_eq!(
        first, second,
        "the entry point registered a second provider struct"
    );
}

/// Camel declares `camel_provider_module_init` but never defines it; the
/// definition here is the module's side of that contract. Coercing both to
/// the same function pointer type checks at compile time that the signature
/// matches the header. This is the same trick that keeps `e_module_load`
/// honest, and the reason the symbol is allowlisted in `eds-sys` at all.
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

/// The half of the load path that a running Camel cannot be asked about.
///
/// Camel decides whether to dlopen `libcameljmap.so` by reading
/// `libcameljmap.urls` beside it, and matching the requested protocol
/// against the lines in that file. A file naming a different protocol means
/// the provider is never loaded. That failure looks exactly like the module
/// not being installed. So this test checks the file's content against the
/// protocol the code registers, instead of trusting both values written out
/// twice.
///
/// The file holds one protocol per line, with no comments: Camel takes each
/// line verbatim as a protocol name. An SPDX header would register a
/// provider named `# SPDX-FileCopyrightText: ...`. `REUSE.toml` covers the
/// file's copyright notice instead.
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
