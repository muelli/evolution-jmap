// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail half of a JMAP account: what this backend writes on the three
//! sources that it does *not* create.
//!
//! ## Why the mail sources are not children of this collection
//!
//! For all other parts, [`crate::populate`] names a resource,
//! `e_collection_backend_new_child` creates the `ESource`, and
//! [`crate::resource_id`] reads the name back. Mail is different, because of
//! `collection_backend_load_resources()`. On every start, it reads each
//! `.source` file in the cache directory of the backend and asks
//! `dup_resource_id` for its id. It **deletes** the file when the answer is
//! `NULL`.
//!
//! Thus `dup_resource_id` must claim a mail source in that directory, and every
//! reference implementation refuses to claim one. The `dup_resource_id` of
//! `module-google-backend.c` chains up only for `[Calendar]`, `[Memo List]`,
//! `[Task List]` and `[Address Book]`, and answers `NULL` for all others.
//! evolution-ews answers with its own folder id, which the mail sources do not
//! have.
//!
//! This is consistent, because the mail sources are in a different location:
//! the source directory of the registry, with `Parent` set to the uid of the
//! account. The setup UI writes them when it creates the account. They are
//! children of the *account*, but not cached resources of the *backend*.
//! `e_collection_backend_list_mail_sources()` still finds them, and
//! `collection_backend_bind_child_enabled()` still binds their `enabled` to the
//! `mail-enabled` of the account.
//!
//! ## What remains for a collection backend
//!
//! Only this vfunc. A vendor backend fills the three sources through
//! `e_collection_backend_factory_prepare_mail`. The inherited implementation
//! does the wiring that is the same for all vendors. The `identity-uid` of the
//! mail account refers to the identity. The `[Mail Submission] transport-uid`
//! of the identity refers to the transport. Each of the three gets the
//! extension that identifies its type.
//!
//! The vendor-specific part is *which service* the account and the transport
//! are. `module-google-backend.c` writes `imapx`/`smtp` and the host, port and
//! security into an `ESourceCamel`. evolution-ews writes only the name `ews` on
//! both, because the collection gives the server of an EWS account.
//!
//! JMAP has the second shape: one protocol, and no host to invent. The name is
//! `jmap` on both sources, because [`jmap-mail`'s provider] registers one
//! `CamelProvider` with a store type *and* a transport type. JMAP submits
//! through the same session that it reads through (RFC 8621 §7). Thus there is
//! no second service, unlike `smtp` next to `imapx`.
//!
//! [`jmap-mail`'s provider]: ../../jmap_mail/provider/index.html
//!
//! ## What this module deliberately does not write
//!
//! **The host, the port and the security method.** For Google, they are vendor
//! constants. Here they are properties of the account of the user. They are
//! already in the `[Authentication]` and `[Security]` of the collection
//! source. [`crate::collection_source`] reads them there, and every child from
//! [`crate::child_source`] copies them. This vfunc does not get the account:
//! `prepare_mail` gets the three mail sources and the *factory*, not the
//! collection. A guess would be worse than no value.
//!
//! **`[Mail Identity] Address`.** A JMAP session states the email address of
//! the identity (`Identity/get` has it). But this vfunc runs before any
//! connection, and a wrong address is a wrong `From:` on sent mail. The
//! `identity` module of `jmap-mail-sync` gives that address when a session
//! exists.
//!
//! ## Nothing calls it in Evolution 3.52
//!
//! This limits the value of the tests.
//! `e_collection_backend_factory_prepare_mail` has no caller in
//! evolution-data-server 3.52.3 or evolution 3.52.3. It is public API that
//! vendor backends implement, for the account-setup path to call.
//! evolution-ews implements it, and so does this crate. `tests/prepare_mail.rs`
//! calls it as a caller would. Thus it tests the vfunc, not that a caller
//! exists.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_TRANSPORT, ECollectionBackendFactory,
    ECollectionBackendFactoryClass, ESource, ESourceBackend, e_collection_backend_factory_get_type,
    e_source_backend_set_backend_name, e_source_get_extension,
};
use jmap_backend_core::subclass;
use jmap_backend_core::trampoline::{guard, log_critical};

/// The `ESourceBackend:backend-name` on the mail account and the mail
/// transport: the Camel protocol, and the first line of `libcameljmap.urls`.
///
/// The spelling is the same as [`crate::factory::FACTORY_NAME`] and as the book
/// and calendar backends: four namespaces, one account, one word. Here it is
/// the protocol that `camel_provider_get` gets. Of the four, only this one is
/// not an EDS factory name.
pub const MAIL_BACKEND_NAME: &CStr = c"jmap";

/// The two of the three sources that name a service, each with the extension
/// that holds the name.
///
/// The identity is deliberately not here: it is a person, not a service.
/// `collection_backend_child_is_mail()` would read an identity with
/// `[Mail Account]` as a second receiving account of this user.
const SERVICES: [&CStr; 2] = [
    E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT,
];

/// The `prepare_mail` vfunc: chains up, then names the provider on the two
/// services.
///
/// # Safety
///
/// The three sources must be NULL or valid `ESource`s that outlive the call,
/// and `factory` must be a valid `ECollectionBackendFactory` of this subclass.
/// EDS's own `e_collection_backend_factory_prepare_mail` checks these
/// arguments before it dispatches here.
pub unsafe fn prepare_mail(
    factory: *mut ECollectionBackendFactory,
    mail_account_source: *mut ESource,
    mail_identity_source: *mut ESource,
    mail_transport_source: *mut ESource,
) {
    // Unlike `crate::child_source`, this function does not call
    // `e_source_mail_*_get_type()`. `e_source_get_extension` returns NULL for
    // an unregistered type, without a message (its `g_critical` is behind
    // `#ifdef DEBUG`). The chain-up of the parent would then join nothing.
    // That cannot occur here: `e_source_class_init` ends with a
    // `g_type_ensure` of every built-in extension, all four mail ones included.
    // Every source here is a live `ESource`, so that class_init has run.
    // `tests/prepare_mail.rs` fails if EDS stops this.

    // Use the class of the parent type, not `g_type_class_peek_parent` of the
    // class of the factory. The reason is in the finalize trampoline of
    // `jmap-backend-core`: for a further subclass, that would point back to
    // this function.
    // SAFETY: an instance of this subclass exists, so its class does, so the
    // class it derives from is initialised and alive for the process.
    let parent = unsafe {
        subclass::parent_class::<ECollectionBackendFactoryClass>(
            e_collection_backend_factory_get_type(),
        )
    };
    match parent.and_then(|class| class.prepare_mail) {
        // SAFETY: the implementation of the parent, called with the arguments
        // of this vfunc on an instance of a type derived from it.
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
            // Not reachable through the EDS wrapper, which refuses a NULL
            // source before it dispatches. A direct caller of the vfunc gets
            // one unwritten source, not a crash in the registry.
            log_critical(&format!(
                "prepare_mail: no source to write {extension:?} on; it will \
                 name no provider"
            ));
            continue;
        }

        // SAFETY: a valid source by the contract of this function, and a
        // header constant naming an extension deriving from `ESourceBackend`,
        // whose type `e_source_class_init` registers (see above); the
        // extension is created on demand and owned by the source.
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

/// The slot that [`crate::factory`] installs, with the panic guard that EDS
/// needs before it.
///
/// A panic here would unwind into `evolution-source-registry`, the one process
/// that owns every data source in the session. Thus the panic becomes a
/// critical and a half-prepared account.
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
