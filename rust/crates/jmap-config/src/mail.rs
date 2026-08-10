// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three sources an account's mail is: `[Mail Account]`,
//! `[Mail Identity]`, `[Mail Transport]`.
//!
//! [`crate::account`] writes the account itself; this writes the three sources
//! that hang off it. They are separate *sources* rather than three more groups
//! in the account's file, and they are not children of the collection *backend*
//! either — [the collection backend's `prepare_mail`][prepare_mail] sets out
//! why at length: `collection_backend_load_resources()` deletes the cache file
//! of any child whose `dup_resource_id` answers NULL, and every reference
//! implementation answers NULL for exactly the mail extensions. So the mail
//! sources live in the registry's own source directory, parented to the
//! account, and are the setup's to write. That is this module.
//!
//! ## Two writers, one file
//!
//! Everything here is also written by `prepare_mail`, and the duplication is
//! real rather than accidental. The two run in different processes with
//! different things in reach: the vfunc has the factory and *not* the user's
//! answers — EDS hands it the three sources and nothing else, which is why it
//! writes no address — while this runs in Evolution, where the answers are, and
//! where no collection factory instance exists to call the vfunc on.
//!
//! Neither can therefore stand in for the other, and in Evolution 3.52 it is
//! this one that runs at all: `e_collection_backend_factory_prepare_mail` has
//! no caller anywhere in evolution-data-server 3.52.3 or evolution 3.52.3. That
//! is precisely why `tests/mail.rs` holds the two against each other — nothing
//! else would notice the uncalled one going stale, and it is the implementation
//! a later Evolution reaching that hook would get.
//!
//! The overlap is not equality, and the difference is the server: the vfunc
//! writes the two service *names* and can write no host, because it is handed
//! the three sources without the account they belong to. So the two are held
//! against each other on what both say, and everything below the service name is
//! this writer's alone.
//!
//! ## What is written, and what writes it instead
//!
//! - **`Parent`**, on all three, is what makes them this account's mail:
//!   `e_collection_backend_list_mail_sources()` finds them by walking the
//!   account's children, and `collection_backend_bind_child_enabled()` binds
//!   each one's `enabled` to the account's `mail-enabled` on the same walk. It
//!   is written here rather than left to Evolution's assistant — which is
//!   believed to set it too — because a writer that produced a complete account
//!   only when called from one particular caller is a writer whose output
//!   depends on something none of its tests can see.
//! - **The service name** on the account and the transport, which is Camel's
//!   protocol and the first line of `libcameljmap.urls`: `jmap` on both,
//!   because JMAP submits over the session it reads through and
//!   [`jmap-mail`'s provider] registers one protocol with a store type *and* a
//!   transport type in it.
//! - **The two links** — the account's `identity-uid` and the identity's
//!   `[Mail Submission] transport-uid` — which are what make three sources one
//!   account.
//! - **`[Mail Identity] Address`**, from the same string as
//!   `[Collection] Identity`. EDS keeps the address in two places; that they
//!   agree is not EDS's business but the setup's, and an identity that
//!   disagreed with its account would send mail from an address the account
//!   does not claim.
//! - **The server the two services reach** — `[Authentication]` and
//!   `[Security]` on the account and on the transport, which is
//!   `apply_server` below and where the answer to "why is this not a Camel
//!   group?"
//!   is written down. Host, port, user and encryption do not live in the
//!   provider's own `[JMAP Backend]` group at all: `ESourceCamel` binds those
//!   five `CamelNetworkSettings` properties to the two extensions above, so a
//!   mail source that names a provider and no host is one whose store is handed
//!   a settings object with an empty host in it. The one thing that is genuinely
//!   Camel's is the *spelling* of the security method — see
//!   [`MAIL_SECURITY_METHOD_TLS`], which is not the string the collection
//!   carries.
//!
//! Not written here:
//!
//! - **`Enabled`** — bound to the account's `mail-enabled` by the collection
//!   backend on every load, so a value written here is one the registry
//!   overwrites. Which is also why the three sources are written whether or not
//!   [`Parts::mail`](jmap_collection_sync::Parts::mail) is on: "receive mail
//!   for this account" is a switch, and a switch needs something to switch.
//! - **`[Mail Identity] Name`** — the user's display name, which is the
//!   assistant's identity page's to write and which an [`Account`] does not
//!   carry. Blank means Evolution sends `From: <address>`, which is
//!   RFC-conformant and is not something to invent an answer for.
//! - **The `[JMAP Backend]` group itself.** The `ESourceCamel` subtype it
//!   belongs to is generated from `jmap-mail`'s `CamelJmapSettings` GType, and
//!   this crate links no Camel — but nothing here needs it to. The extension is
//!   created on demand by `e_source_camel_configure_service` in whichever
//!   process opens the account, the five properties this setup has answers for
//!   are bound to `[Authentication]` and `[Security]` rather than stored in it,
//!   and what remains in the group is `CamelStoreSettings`' and
//!   `CamelOfflineSettings`' inherited defaults — filters, offline limits — which
//!   are the user's to change later and not an account setup's to invent.
//!   `tests/mail.rs` still generates the subtype and reads the settings object
//!   back through it, because that object is the only place the account can be
//!   asked the question the store will ask it.
//!
//! [prepare_mail]: ../../jmap_backend_collection/prepare_mail/index.html
//! [`jmap-mail`'s provider]: ../../jmap_mail/provider/index.html

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_IDENTITY, E_SOURCE_EXTENSION_MAIL_SUBMISSION,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, E_SOURCE_EXTENSION_SECURITY, ESource, ESourceAuthentication,
    ESourceBackend, ESourceMailAccount, ESourceMailIdentity, ESourceMailSubmission,
    ESourceSecurity, e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_backend_set_backend_name, e_source_get_extension,
    e_source_get_uid, e_source_mail_account_get_type, e_source_mail_account_set_identity_uid,
    e_source_mail_identity_get_type, e_source_mail_identity_set_address,
    e_source_mail_submission_get_type, e_source_mail_submission_set_transport_uid,
    e_source_mail_transport_get_type, e_source_security_get_type, e_source_security_set_method,
    e_source_set_parent,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_collection_sync::child_source::Connection;

use crate::account::{Account, as_ptr};

/// The Camel protocol the mail account and the mail transport name — the same
/// string [the collection backend's vfunc][prepare_mail] writes, and the one
/// line in `libcameljmap.urls`.
///
/// One name for both, because there is one provider: RFC 8621 §7 submits over
/// the same session the mail is read through, so there is no second service
/// beside it the way `smtp` sits beside `imapx`.
///
/// [prepare_mail]: ../../jmap_backend_collection/prepare_mail/constant.MAIL_BACKEND_NAME.html
pub const MAIL_BACKEND_NAME: &CStr = c"jmap";

/// The `[Security] Method` a mail source carries when the connection is
/// encrypted — and deliberately not `"tls"`, which is what [`crate::account`]
/// writes on the collection.
///
/// One key, read by two different pieces of code depending on which kind of
/// source it is on. On the collection, `ESourceSecurity` compares it against
/// `"none"` and hands the JMAP backends a boolean. On a mail source an
/// `ESourceCamel` extension additionally binds it to `CamelNetworkSettings`'s
/// `security-method` through `e_binding_transform_enum_nick_to_value`, which
/// looks the string up as a **`CamelNetworkSecurityMethod` enum nick**.
///
/// The failure on a string that is not one is quiet and worth stating exactly,
/// because it is not what it first looks like: the transform returns `FALSE` and
/// the binding sets nothing, so the settings object keeps the property's own
/// default — which in EDS 3.52 is `STARTTLS_ON_STANDARD_PORT`. An account
/// written as `"tls"` therefore *does* connect over TLS today, by way of a
/// default nobody chose: `jmap-mail` only distinguishes `NONE` from not, so it
/// sees encryption, while Evolution's account editor shows the user an
/// encryption setting they did not pick, and a Camel release that moved that
/// default to `NONE` would turn the same keyfile into a refusal to connect.
/// Writing the nick is what makes the account say what it means to both readers.
///
/// Of Camel's three nicks this is the one that describes HTTPS — TLS from the
/// first byte, no in-band upgrade — and it is the spelling Evolution's own
/// server settings page writes back through the same binding, so an account
/// committed here and then merely opened in the editor does not change shape.
pub const MAIL_SECURITY_METHOD_TLS: &CStr = c"ssl-on-alternate-port";

/// And when it is not encrypted, which is the one spelling both sides share:
/// `ESourceSecurity` documents `"none"` as its convention for "no security" and
/// `CamelNetworkSecurityMethod`'s first value has the same nick.
pub const MAIL_SECURITY_METHOD_NONE: &CStr = c"none";

/// The three scratch sources a commit is handed, in the order they are talked
/// about everywhere else — receiving account, identity, transport.
///
/// Raw pointers rather than anything owned: they belong to the setup that is
/// committing them, and [`apply`] keeps none of them past the call.
pub struct MailSources {
    /// What Evolution receives through, and what the folder tree hangs off.
    pub account: *mut ESource,
    /// Who the mail is from — a person, not a service.
    pub identity: *mut ESource,
    /// What Evolution sends through.
    pub transport: *mut ESource,
}

/// Writes the account's three mail sources, which afterwards say exactly this
/// account.
///
/// Like [`crate::account::apply`], every field is written every time rather
/// than only when there is something new to say: a commit lands on sources that
/// already say something, and an address left behind because the writer had
/// nothing to add is the `From:` of every message sent afterwards.
///
/// # Safety
///
/// `collection` and all three sources in `sources` must be valid `ESource`s —
/// the account source and the three scratch sources the setup is committing.
/// This call takes no reference to any of them and nothing here outlives it.
pub unsafe fn apply(collection: *mut ESource, sources: &MailSources, account: &Account) {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a type
    // nothing has referenced yet is one it cannot find — and here it would
    // create nothing and return NULL. `e_source_class_init` happens to
    // `g_type_ensure` every built-in extension, these four included, so a live
    // `ESource` already implies them; referencing them anyway costs one
    // already-registered type lookup each and keeps this module's correctness
    // out of EDS's list of built-ins.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_identity_get_type();
        e_source_mail_submission_get_type();
        e_source_mail_transport_get_type();
        // The two [`apply_server`] writes on, which are not mail extensions and
        // so are not covered by the four above — though `e_source_class_init`
        // ensures these as well.
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // The strings outlive every call that borrows them, which is what keeps the
    // pointers below valid. Truncating at an interior NUL rather than refusing,
    // as everywhere a typed string crosses into C: what is kept is what the
    // address would have meant to every C caller downstream anyway, and
    // refusing the write would leave the *previous* address on an identity
    // being edited.
    let address = cstring_lossy(&account.identity);

    // SAFETY: valid sources by this function's contract; the uid is owned by
    // the account source and outlives the three setters that copy it, and each
    // extension is created on demand and owned by the source it is asked of.
    unsafe {
        // What makes the three the account's mail rather than three top-level
        // sources the collection knows nothing about.
        let account_uid = e_source_get_uid(collection);
        for source in [sources.account, sources.identity, sources.transport] {
            e_source_set_parent(source, account_uid);
        }

        let mail_account: *mut ESourceMailAccount =
            e_source_get_extension(sources.account, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
                .cast();
        // `ESourceMailAccount` derives from `ESourceBackend`, which is where
        // the Camel protocol lives; so does `ESourceMailTransport` below.
        e_source_backend_set_backend_name(
            mail_account.cast::<ESourceBackend>(),
            MAIL_BACKEND_NAME.as_ptr(),
        );
        e_source_mail_account_set_identity_uid(mail_account, e_source_get_uid(sources.identity));

        let identity: *mut ESourceMailIdentity =
            e_source_get_extension(sources.identity, E_SOURCE_EXTENSION_MAIL_IDENTITY.as_ptr())
                .cast();
        e_source_mail_identity_set_address(identity, address.as_ptr());

        // A group of the identity's rather than a source of its own: where a
        // person's mail leaves through is a property of the person.
        let submission: *mut ESourceMailSubmission = e_source_get_extension(
            sources.identity,
            E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr(),
        )
        .cast();
        e_source_mail_submission_set_transport_uid(submission, e_source_get_uid(sources.transport));

        let transport: *mut ESourceBackend = e_source_get_extension(
            sources.transport,
            E_SOURCE_EXTENSION_MAIL_TRANSPORT.as_ptr(),
        )
        .cast();
        e_source_backend_set_backend_name(transport, MAIL_BACKEND_NAME.as_ptr());
    }

    // And the server each of the two services reaches. Not the identity, which
    // reaches none.
    for source in [sources.account, sources.transport] {
        // SAFETY: a valid source by this function's contract.
        unsafe { apply_server(source, &account.connection) };
    }
}

/// Writes where a mail service reaches its server, on the source that service
/// is configured from.
///
/// Public because it is also the whole of what a `commit_changes` can write.
/// [`apply`] above is the shape a *commit of the account* has — three sources at
/// once, from the one place all three are in reach — but the vfunc Evolution
/// dispatches is handed one backend holding one scratch source, and the account
/// beside it. So [`crate::backend::commit`] calls this, once, on whichever of
/// the two service sources its own backend is the candidate for; everything else
/// [`apply`] writes is either Evolution's own (the parent, the two uid links,
/// the identity's address) or already on the source when the vfunc is reached
/// (the service name, which `e_mail_config_assistant` puts there when it mints
/// the scratch source).
///
/// Nothing here is a *Camel* group, which is the surprise and the reason this is
/// four ordinary `ESource` extensions rather than a dependency on `jmap-mail`:
/// host, port, user, authentication mechanism and security method are the five
/// `CamelNetworkSettings` properties that `ESourceCamel` binds to *other*
/// extensions — `[Authentication]` and `[Security]` on the same source — so they
/// are excluded from the generated `[JMAP Backend]` group by construction. The
/// provider's own group holds what is left, which for this provider is
/// `CamelStoreSettings`' and `CamelOfflineSettings`' inherited properties and
/// nothing this setup has an answer for.
///
/// The values are the collection's own, out of the same [`Connection`], and that
/// is load-bearing beyond consistency: EDS decides whether a child source shares
/// its collection's password by comparing the two `[Authentication] Host`
/// strings (`e_util_can_use_collection_as_credential_source`, reached from
/// `e_source_credentials_provider_ref_credentials_source`). The comparison exists
/// so that an account may put its outgoing service on another server with a
/// password of its own; here a host that disagreed — including one left
/// unwritten while the collection has one — would be a second password prompt
/// for the same server and a second libsecret entry to fall out of step.
///
/// `[Authentication] Method` is the one field written differently from the
/// collection's, and it is written as nothing. On a collection it names the EDS
/// credentials provider implementation; on a mail source `ESourceCamel` also
/// binds it to `CamelNetworkSettings:auth-mechanism`, where it names a SASL
/// mechanism. `jmap-mail` passes a NULL mechanism to
/// `camel_session_authenticate_sync` because JMAP authenticates over HTTP and
/// advertises none to choose between, so the honest value is the absent one —
/// which EDS spells `"none"` and `ESourceCamel` converts back to NULL on the way
/// to Camel. It is *written* rather than left alone for the same reason as every
/// other field here: a mechanism left over from a previous commit is one
/// Evolution's account editor would show as this account's authentication type.
///
/// # Safety
///
/// `source` must be a valid `ESource` — one of the two service sources the setup
/// is committing. Nothing here outlives the call.
pub unsafe fn apply_server(source: *mut ESource, connection: &Connection) {
    // Outliving every call that borrows them, as in `apply` above.
    let host = cstring_lossy(&connection.host);
    let user = connection.user.as_deref().map(cstring_lossy);
    let security_method = if connection.secure {
        MAIL_SECURITY_METHOD_TLS
    } else {
        MAIL_SECURITY_METHOD_NONE
    };

    // SAFETY: a valid source by this function's contract, and header constants
    // naming extensions whose types `apply` registered; each extension is
    // created on demand and owned by the source, and every setter copies the
    // string it is given.
    unsafe {
        let auth: *mut ESourceAuthentication =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_host(auth, host.as_ptr());
        e_source_authentication_set_user(auth, as_ptr(&user));
        e_source_authentication_set_method(auth, ptr::null());
        // Zero is how `[Authentication] Port` spells "not set", and it is what
        // an unconfigured `CamelNetworkSettings` reads back as as well, so the
        // two ends of the binding agree about the absence and not only about a
        // value.
        e_source_authentication_set_port(auth, connection.port.unwrap_or(0));

        let security: *mut ESourceSecurity =
            e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
        e_source_security_set_method(security, security_method.as_ptr());
    }
}
