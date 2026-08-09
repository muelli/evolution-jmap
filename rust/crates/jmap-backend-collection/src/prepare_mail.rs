// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail half of a JMAP account: what this backend says about the three
//! sources it does *not* create.
//!
//! ## Why the mail sources are not children of this collection
//!
//! Every other part of M6 works the same way — [`crate::populate`] names a
//! resource, `e_collection_backend_new_child` mints the `ESource`,
//! [`crate::resource_id`] reads the name back. Mail does not, and the reason is
//! in `collection_backend_load_resources()`: on every start it reads each
//! `.source` file in the backend's cache directory, asks `dup_resource_id` what
//! it is, and **deletes** the file when the answer is `NULL`. So a mail source
//! kept in that directory would have to be claimed by `dup_resource_id` — and
//! every reference implementation refuses to claim one.
//! `module-google-backend.c`'s `dup_resource_id` chains up only for
//! `[Calendar]`, `[Memo List]`, `[Task List]` and `[Address Book]` and answers
//! `NULL` for anything else; evolution-ews answers with its own folder id, which
//! the mail sources do not carry.
//!
//! That is consistent because the mail sources live somewhere else: in the
//! registry's own source directory, with `Parent` set to the account's uid,
//! written by the setup UI when the account is created (M7 here). They are
//! children of the *account* without being cached resources of the *backend* —
//! `e_collection_backend_list_mail_sources()` still finds them, and
//! `collection_backend_bind_child_enabled()` still binds their `enabled` to the
//! account's `mail-enabled`.
//!
//! ## So what is left for a collection backend to say
//!
//! Exactly this vfunc. `e_collection_backend_factory_prepare_mail` is the hook a
//! vendor backend fills those three sources in through, and the inherited
//! implementation does only the wiring that is the same for every vendor: the
//! mail account's `identity-uid` points at the identity, the identity's
//! `[Mail Submission] transport-uid` points at the transport, and each of the
//! three carries the extension that makes it recognisable as what it is. What is
//! vendor-specific is *which service* the account and the transport are —
//! `module-google-backend.c` writes `imapx`/`smtp` plus host, port and security
//! into an `ESourceCamel`; evolution-ews writes the single name `ews` on both
//! and nothing else, because an EWS account's server comes from the collection.
//!
//! JMAP is the second shape: one protocol, no host of our own to invent. The
//! name is `jmap` on both sources because [`jmap-mail`'s provider] registers one
//! `CamelProvider` with a store type *and* a transport type in it — JMAP submits
//! over the session it reads through (RFC 8621 §7), so there is no second
//! service beside it the way `smtp` sits beside `imapx`.
//!
//! [`jmap-mail`'s provider]: ../../jmap_mail/provider/index.html
//!
//! ## What this deliberately does not write
//!
//! **The host, the port and the security method.** Google can write them because
//! they are constants of the vendor; here they are properties of the user's
//! account, and the place they already live is the collection source's own
//! `[Authentication]` and `[Security]` — which is what
//! [`crate::collection_source`] reads and what every child written by
//! [`crate::child_source`] repeats. Writing them here as well would need the
//! account, which this vfunc is not given: `prepare_mail` is handed the three
//! mail sources and the *factory*, not the collection. Guessing them from
//! nothing would be worse than leaving them for the code that has them.
//!
//! **`[Mail Identity] Address`.** The identity's email address is the one thing
//! here a JMAP session actually states — `Identity/get` has it — but this vfunc
//! runs before anything has connected, and an identity written with a wrong
//! address is a wrong `From:` on sent mail. `jmap-mail-sync`'s `identity` module
//! is where that answer comes from, once there is a session to ask.
//!
//! ## Nothing calls it in Evolution 3.52
//!
//! Worth writing down, because it bounds what the tests are worth:
//! `e_collection_backend_factory_prepare_mail` has no caller anywhere in
//! evolution-data-server 3.52.3 or evolution 3.52.3 — it is public API that
//! vendor backends implement and that the account-setup path is expected to
//! call. evolution-ews implements it anyway, and so does this. `tests/prepare_mail.rs`
//! calls it the way a caller would, which checks the vfunc rather than the fact
//! that somebody reaches it.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_TRANSPORT, ECollectionBackendFactory,
    ECollectionBackendFactoryClass, ESource, ESourceBackend, e_collection_backend_factory_get_type,
    e_source_backend_set_backend_name, e_source_get_extension,
};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::trampoline::{guard, log_critical};

/// The `ESourceBackend:backend-name` written on the mail account and the mail
/// transport: Camel's protocol, and the first line of `libcameljmap.urls`.
///
/// The same spelling as [`crate::factory::FACTORY_NAME`] and as the book and
/// calendar backends' — four namespaces, one account, one word. Here it means
/// the protocol `camel_provider_get` is asked for, which is the only one of the
/// four that is not an EDS factory name.
pub const MAIL_BACKEND_NAME: &CStr = c"jmap";

/// The two of the three sources that name a service, each with the extension the
/// name is written on.
///
/// The identity is not here, and its absence is the point: it is a person rather
/// than a service, and `collection_backend_child_is_mail()` would read an
/// identity carrying `[Mail Account]` as a second receiving account of this
/// user's.
const SERVICES: [&CStr; 2] = [
    E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT,
];

/// The `prepare_mail` vfunc: chain up, then name the provider on the two
/// services.
///
/// # Safety
///
/// The three sources must be NULL or valid `ESource`s that outlive the call, and
/// `factory` must be a valid `ECollectionBackendFactory` of this subclass — the
/// arguments EDS's own `e_collection_backend_factory_prepare_mail` checks before
/// it dispatches here.
pub unsafe fn prepare_mail(
    factory: *mut ECollectionBackendFactory,
    mail_account_source: *mut ESource,
    mail_identity_source: *mut ESource,
    mail_transport_source: *mut ESource,
) {
    // No `e_source_mail_*_get_type()` calls first, unlike `crate::child_source`.
    // `e_source_get_extension` finds an extension by walking the registered
    // children of `E_TYPE_SOURCE_EXTENSION` and returns NULL for a type nothing
    // has referenced yet — silently, since its `g_critical` is behind
    // `#ifdef DEBUG` — which would leave the parent's chain-up joining nothing
    // to nothing. It cannot happen here: `e_source_class_init` ends with a
    // `g_type_ensure` of every built-in extension, all four mail ones included,
    // and every source reaching this function is a live `ESource`, so that
    // class_init has already run. `tests/prepare_mail.rs` is what would notice
    // if EDS stopped doing it.

    // The parent type's class rather than `g_type_class_peek_parent` of the
    // factory's, for the reason `jmap-backend-core`'s finalize trampoline gives:
    // a further subclass of ours would make that one point back at this
    // function.
    // SAFETY: an instance of this subclass exists, so its class does, so the
    // class it derives from is initialised and alive for the process.
    let parent = unsafe { g_type_class_peek(e_collection_backend_factory_get_type()) }
        .cast::<ECollectionBackendFactoryClass>();
    match unsafe { parent.as_ref() }.and_then(|class| class.prepare_mail) {
        // SAFETY: the parent's own implementation, called with this vfunc's
        // arguments on an instance of a type derived from it.
        Some(prepare) => unsafe {
            prepare(
                factory,
                mail_account_source,
                mail_identity_source,
                mail_transport_source,
            );
        },
        None => log_critical(
            "prepare_mail: there is no parent implementation to chain up to; \
             the account, identity and transport are left unjoined",
        ),
    }

    for (source, extension) in [mail_account_source, mail_transport_source]
        .into_iter()
        .zip(SERVICES)
    {
        if source.is_null() {
            // Unreachable through EDS's wrapper, which refuses a NULL source
            // before it dispatches. A caller that reached the vfunc directly
            // gets one unwritten source rather than a crash in the registry.
            log_critical(&format!(
                "prepare_mail: no source to write {extension:?} on; it will \
                 name no provider"
            ));
            continue;
        }

        // SAFETY: a valid source by this function's contract, and a header
        // constant naming an extension deriving from `ESourceBackend` whose type
        // is registered above; the extension is created on demand and owned by
        // the source.
        let backend: *mut ESourceBackend =
            unsafe { e_source_get_extension(source, extension.as_ptr()) }.cast();
        if backend.is_null() {
            log_critical(&format!(
                "prepare_mail: {extension:?} is not a registered ESource \
                 extension, so the source names no provider"
            ));
            continue;
        }

        // SAFETY: a live extension of the source, and the setter copies the
        // string.
        unsafe { e_source_backend_set_backend_name(backend, MAIL_BACKEND_NAME.as_ptr()) };
    }
}

/// The slot [`crate::factory`] installs, with the panic guard EDS needs in front
/// of it.
///
/// A panic here would unwind into `evolution-source-registry` — the one process
/// that owns every data source in the session — so it becomes a critical and a
/// half-prepared account instead.
///
/// # Safety
///
/// As [`prepare_mail`]; this is the signature EDS calls through.
pub(crate) unsafe extern "C" fn prepare_mail_trampoline(
    factory: *mut ECollectionBackendFactory,
    mail_account_source: *mut ESource,
    mail_identity_source: *mut ESource,
    mail_transport_source: *mut ESource,
) {
    guard("prepare_mail", (), || {
        // SAFETY: the caller is EDS, whose wrapper has checked all four.
        unsafe {
            prepare_mail(
                factory,
                mail_account_source,
                mail_identity_source,
                mail_transport_source,
            );
        }
    });
}
