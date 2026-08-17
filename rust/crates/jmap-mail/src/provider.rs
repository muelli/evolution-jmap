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
use jmap_backend_core::i18n::{DOMAIN, N_};

use crate::store::store_type;
use crate::transport::transport_type;

/// The protocol Camel keys its provider table by.
///
/// It is also the first line of `libcameljmap.urls`, which is how Camel knows
/// to dlopen this module in the first place, and the `BackendName` an account's
/// `.source` file names. `tests/provider.rs` checks the file against this
/// constant so the two cannot drift.
pub const PROTOCOL: &CStr = c"jmap";

/// What the account type is called in the list Evolution offers.
///
/// Marked with [`N_`] rather than looked up, because the lookup is not ours to
/// make: Camel calls `dgettext` on this string with the provider's
/// `translation_domain` — [`DOMAIN`], bound by this module's entry point —
/// every time it displays it. Translating it here instead would freeze it into
/// whatever locale was current when the module happened to be dlopened.
///
/// A protocol name is not something a language translates, and it is in the
/// catalogue anyway: a translator whose script is not Latin can only
/// transliterate it if it is there to transliterate, and a msgid nobody changes
/// costs a translator one glance.
// TRANSLATORS: the name of an account type, in the list of account types
// Evolution offers when adding an account. A protocol name — leave it as it is
// unless your language writes it in another script.
const NAME: &CStr = N_(c"JMAP");

/// The one-line description shown beneath [`NAME`] in that same list.
///
/// Translated by Camel, for the reason given there.
// TRANSLATORS: the one-line description of the "JMAP" account type, shown
// beneath its name in the list of account types.
const DESCRIPTION: &CStr = N_(c"For reading and storing mail on JMAP servers.");

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
            name: NAME.as_ptr(),
            description: DESCRIPTION.as_ptr(),
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
            // Camel's offer to guess an account's settings from an address.
            // Nothing to guess here: a JMAP account is discovered from its
            // session object, which is [`crate::server`]'s job and needs a
            // connection rather than a callback on a struct. Dropped from the
            // struct in EDS 3.58 along with the two below, so on a newer EDS
            // there is not even a slot to decline — see the note on
            // `url_equal`.
            #[cfg(camel_provider_url_helpers)]
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
            //
            // EDS 3.58 removed all three of these fields — that cache is gone,
            // and with it `CamelProviderAutoDetectFunc`. The fields are
            // `#[cfg]`-gated rather than the struct being built from zeroed
            // memory on purpose: naming every field keeps this a struct
            // literal, so a field some future EDS *adds* is a compile error
            // here instead of a silent NULL, which is the whole point of the
            // version matrix in `docs/eds-version-matrix.md`.
            #[cfg(camel_provider_url_helpers)]
            url_hash: None,
            #[cfg(camel_provider_url_helpers)]
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
