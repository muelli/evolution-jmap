// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelProvider` this module hands to Camel.
//!
//! A provider is a plain C struct, not a GObject: Camel takes the pointer in
//! [`camel_provider_register`] and keeps it for the life of the process without
//! copying it, so whatever this module registers has to stay put and stay
//! valid. Everything the struct points at is therefore either a `'static` C
//! string literal or a `GType`, and the struct itself is leaked on purpose —
//! see [`register`].
//!
//! Registering it is also the only thing the struct does. It carries no
//! behaviour; it says which protocol Camel should route to this module, what
//! Evolution is allowed to offer a JMAP account as, and which types to
//! instantiate. The behaviour lives in those types.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_PROVIDER_IS_REMOTE, CAMEL_PROVIDER_IS_SOURCE, CAMEL_PROVIDER_IS_STORAGE,
    CAMEL_PROVIDER_STORE, CAMEL_PROVIDER_SUPPORTS_SSL, CAMEL_PROVIDER_TRANSPORT,
    CAMEL_URL_ALLOW_AUTH, CAMEL_URL_ALLOW_PASSWORD, CAMEL_URL_ALLOW_PATH, CAMEL_URL_ALLOW_PORT,
    CAMEL_URL_ALLOW_USER, CAMEL_URL_NEED_HOST, CamelProvider, CamelProviderFlags,
    CamelProviderURLFlags, camel_provider_register,
};
use gobject_sys::G_TYPE_INVALID;
use jmap_backend_core::i18n::DOMAIN;

use crate::store::store_type;
use crate::transport::transport_type;

/// The protocol Camel keys its provider table by.
///
/// It is also the first line of `libcameljmap.urls`, which is how Camel knows
/// to dlopen this module in the first place, and the `BackendName` an account's
/// `.source` file names. `tests/provider.rs` checks the file against this
/// constant so the two cannot drift.
pub const PROTOCOL: &CStr = c"jmap";

/// What Evolution may offer a JMAP account as.
///
/// `IS_SOURCE` and `IS_STORAGE` are the two that decide whether the account can
/// receive mail and whether its folders appear in the tree; without them the
/// provider loads and does nothing visible. `SUPPORTS_SSL` is not a claim that
/// the connection is encrypted — it is a claim that the *user may choose* an
/// encrypted one, which is what puts the security options in the account
/// dialog. Refusing plaintext to anywhere but localhost stays where it can
/// actually be enforced, in the client.
const FLAGS: CamelProviderFlags = (CAMEL_PROVIDER_IS_REMOTE
    | CAMEL_PROVIDER_IS_SOURCE
    | CAMEL_PROVIDER_IS_STORAGE
    | CAMEL_PROVIDER_SUPPORTS_SSL) as CamelProviderFlags;

/// Which parts of an account URL mean anything for JMAP.
///
/// `NEED_HOST` rather than `ALLOW_HOST`: there is no local flavour of JMAP, so
/// an account without a server is not a degraded account, it is not an account.
/// `ALLOW_PATH` because a JMAP session lives at a path — `/.well-known/jmap` by
/// convention, but only by convention. The credential parts are `ALLOW` and not
/// `NEED` because on this backend they come from `ESourceAuthentication` and
/// libsecret, not from anything typed into a URL.
const URL_FLAGS: CamelProviderURLFlags = (CAMEL_URL_NEED_HOST
    | CAMEL_URL_ALLOW_PORT
    | CAMEL_URL_ALLOW_PATH
    | CAMEL_URL_ALLOW_USER
    | CAMEL_URL_ALLOW_AUTH
    | CAMEL_URL_ALLOW_PASSWORD) as CamelProviderURLFlags;

/// A registered provider, as a pointer Camel and this module both hold.
///
/// The `OnceLock` needs `Sync` and a raw pointer is not; the pointer is
/// published once and only ever read afterwards, and what it points at is
/// never freed.
struct Registered(*mut CamelProvider);

// SAFETY: the pointer is set once, under the OnceLock, and the struct it
// points at is leaked — nothing can free it or move it, and nothing writes to
// it after registration.
unsafe impl Send for Registered {}
unsafe impl Sync for Registered {}

static PROVIDER: OnceLock<Registered> = OnceLock::new();

/// Builds the provider and hands it to Camel, once per process.
///
/// Returns the registered struct, which is `'static` because it is leaked: a
/// `Box` that is never reclaimed. That is deliberate and not a leak in the
/// sense that matters — Camel stores the bare pointer in a table it never
/// clears, and a provider it can still hand out through `camel_provider_get`
/// after the module dropped the allocation would be a use-after-free in some
/// other process's mail client.
///
/// Idempotent because the entry point is not guaranteed to be reached only
/// once. A second call that built a second struct would leave Camel's table
/// pointing at one of them while earlier callers held the other — two
/// providers for one protocol, disagreeing about nothing today and about the
/// transport slot tomorrow.
pub fn register() -> &'static CamelProvider {
    let registered = PROVIDER.get_or_init(|| {
        // Resolved before the struct is built rather than inside it: both types
        // have to exist before Camel can be told to instantiate them.
        let store = store_type();
        // The mail the account sends, as against the mail it holds. Two
        // services with no pointer between them, which is Camel's shape rather
        // than JMAP's — see `crate::transport`. Naming the type here is only
        // safe because its class installs `send_to_sync`: Camel dispatches
        // sending through the class, and a transport slot naming a type that
        // cannot send is a GLib critical the first time the user presses Send.
        let transport = transport_type();

        let mut object_types = [G_TYPE_INVALID; 2];
        object_types[CAMEL_PROVIDER_STORE as usize] = store;
        object_types[CAMEL_PROVIDER_TRANSPORT as usize] = transport;

        let provider = Box::new(CamelProvider {
            protocol: PROTOCOL.as_ptr(),
            name: c"JMAP".as_ptr(),
            description: c"For reading and storing mail on JMAP servers.".as_ptr(),
            // Exactly "mail": evolution-mail filters the provider list by this
            // string, so anything else is a provider that loads and is never
            // offered.
            domain: c"mail".as_ptr(),
            flags: FLAGS,
            url_flags: URL_FLAGS,
            // The legacy account-dialog machinery. Evolution 3.52 configures a
            // JMAP account through its ESource extensions, not through these,
            // and a NULL here is "no extra widgets" rather than a default.
            extra_conf: ptr::null_mut(),
            port_entries: ptr::null_mut(),
            auto_detect: None,
            object_types,
            // The SASL mechanisms the provider advertises. Empty rather than
            // NULL-as-a-mistake: authentication is Bearer or Basic over HTTPS,
            // negotiated by the client, and Camel's list is about its own SASL
            // implementations.
            authtypes: ptr::null_mut(),
            // Only consulted by the CamelURL-keyed service cache, which
            // nothing reaches on a registry-configured account; NULL makes
            // Camel use its own defaults.
            url_hash: None,
            url_equal: None,
            // Not NULL, which in this struct means "a provider in the EDS
            // source tree, translated with EDS's catalogue". These strings are
            // ours, so the domain is ours — the same constant the module's
            // entry point binds, because Camel looking a name up in a domain
            // nothing bound would search the host process's idea of where
            // catalogues live. There is no catalogue installed under this
            // domain yet, and gettext falls back to the untranslated string
            // when there is none, which is the honest outcome.
            translation_domain: DOMAIN.as_ptr(),
            priv_: ptr::null_mut(),
        });

        // SAFETY: the struct is leaked, so it outlives the call and every
        // later `camel_provider_get`; every pointer in it is 'static.
        let provider = Box::into_raw(provider);
        unsafe { camel_provider_register(provider) };
        Registered(provider)
    });

    // SAFETY: the pointer came from `Box::into_raw` above, was never freed,
    // and is not written to after registration.
    unsafe { &*registered.0 }
}
