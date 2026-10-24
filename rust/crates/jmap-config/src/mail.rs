// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three mail sources of an account: `[Mail Account]`, `[Mail Identity]`,
//! `[Mail Transport]`.
//!
//! [`crate::account`] writes the account source. This module writes the three
//! sources below it. They are separate *sources*, not three more groups in the
//! account's file. They are also not children of the collection *backend*.
//! [The collection backend's `prepare_mail`][prepare_mail] gives the full reason:
//! `collection_backend_load_resources()` deletes the cache file of each child
//! whose `dup_resource_id` returns NULL, and every reference implementation
//! returns NULL for the mail extensions. Thus the mail sources are in the
//! registry's own source directory, with the account as parent, and the setup
//! writes them.
//!
//! ## Two writers, one file
//!
//! `prepare_mail` also writes all of this, and the duplication is intentional.
//! The two writers run in different processes with access to different data. The
//! vfunc has the factory but *not* the user's answers: EDS gives it only the
//! three sources, so it writes no address. This module runs in Evolution, which
//! has the answers but no collection factory instance to call the vfunc on.
//!
//! Thus neither writer can replace the other, and in Evolution 3.52 only this one
//! runs: `e_collection_backend_factory_prepare_mail` has no caller in
//! evolution-data-server 3.52.3 or evolution 3.52.3. For this reason
//! `tests/mail.rs` compares the two writers. Nothing else detects when the
//! uncalled one becomes stale, and a later Evolution that calls that hook uses it.
//!
//! The two writers do not write the same data, and the difference is the server.
//! The vfunc writes the two service *names* but cannot write a host, because it
//! gets the three sources without their account. Thus the tests compare only the
//! data that both write. All data below the service name comes from this writer.
//!
//! ## What is written, and what writes it instead
//!
//! - **`Parent`**, on all three, makes them this account's mail:
//!   `e_collection_backend_list_mail_sources()` finds them through the account's
//!   children, and `collection_backend_bind_child_enabled()` binds the `enabled`
//!   of each to the account's `mail-enabled` in the same pass. This module writes
//!   it and does not leave it to Evolution's assistant (which possibly also
//!   writes it). If a writer makes a complete account only for one caller, its
//!   output depends on a thing that its tests cannot see.
//! - **The service name** on the account and the transport. This is Camel's
//!   protocol and the first line of `libcameljmap.urls`. It is `jmap` on both,
//!   because JMAP submits over the session it reads through, and
//!   [`jmap-mail`'s provider] registers one protocol with a store type *and* a
//!   transport type.
//! - **The two links**: the account's `identity-uid` and the identity's
//!   `[Mail Submission] transport-uid`. They make three sources one account.
//! - **`[Mail Identity] Address`**, from the same string as
//!   `[Collection] Identity`. EDS keeps the address in two locations, and the
//!   setup (not EDS) makes sure that they agree. An identity with a different
//!   address sends mail from an address that the account does not claim.
//! - **The server that the two services connect to**: `[Authentication]` and
//!   `[Security]` on the account and on the transport. `apply_server` below
//!   writes them and tells why they are not a Camel group. Host, port, user,
//!   authentication mechanism and encryption are not in the provider's own
//!   `[JMAP Backend]` group: `ESourceCamel` binds these five
//!   `CamelNetworkSettings` properties to the two extensions above. Thus the
//!   store of a mail source that names a provider but no host gets a settings
//!   object with an empty host. Only the *spelling* of the security method is
//!   Camel's: see [`MAIL_SECURITY_METHOD_TLS`], which is not the string that the
//!   collection contains.
//!
//! Not written here:
//!
//! - **`Enabled`**: the collection backend binds it to the account's
//!   `mail-enabled` on each load, so the registry overwrites a value written
//!   here. For the same reason, this module writes the three sources also when
//!   [`Parts::mail`](jmap_collection_sync::Parts::mail) is off: "receive mail for
//!   this account" is a switch, and a switch needs a thing to control.
//! - **`[Mail Identity] Name`**: the user's display name. The identity page of
//!   the assistant writes it, and an [`Account`] does not contain it. If it is
//!   blank, Evolution sends `From: <address>`, which conforms to the RFC. The
//!   setup must not invent a value.
//! - **The `[JMAP Backend]` group itself.** Its `ESourceCamel` subtype comes
//!   from the `CamelJmapSettings` GType of `jmap-mail`, and this crate links no
//!   Camel, but this module does not need the subtype.
//!   `e_source_camel_configure_service` creates the extension on demand in the
//!   process that opens the account. The five properties that this setup has
//!   values for are bound to `[Authentication]` and `[Security]`, not stored in
//!   the group. The remaining contents are the inherited defaults of
//!   `CamelStoreSettings` and `CamelOfflineSettings` (filters, offline limits).
//!   The user can change them later, and an account setup must not invent them.
//!   `tests/mail.rs` still generates the subtype and reads the settings object
//!   through it, because only that object answers the question that the store
//!   asks the account.
//!
//! [prepare_mail]: ../../jmap_backend_collection/prepare_mail/index.html
//! [`jmap-mail`'s provider]: ../../jmap_mail/provider/index.html

use std::ffi::CStr;

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

/// The Camel protocol that the mail account and the mail transport name.
///
/// [The collection backend's vfunc][prepare_mail] writes the same string, and it
/// is the one line in `libcameljmap.urls`. Both use one name because there is one
/// provider: RFC 8621 §7 submits over the session that reads the mail. Thus there
/// is no second service, as `smtp` is beside `imapx`.
///
/// [prepare_mail]: ../../jmap_backend_collection/prepare_mail/constant.MAIL_BACKEND_NAME.html
pub const MAIL_BACKEND_NAME: &CStr = jmap_backend_core::BACKEND_NAME;

/// The `[Security] Method` of a mail source when the connection is encrypted.
///
/// It is deliberately not `"tls"`, which [`crate::account`] writes on the
/// collection. Two different readers read this one key, as a function of the
/// source type. On the collection, `ESourceSecurity` compares it with `"none"`
/// and gives the JMAP backends a boolean. On a mail source, an `ESourceCamel`
/// extension also binds it to the `security-method` of `CamelNetworkSettings`
/// through `e_binding_transform_enum_nick_to_value`. That function reads the
/// string as a **`CamelNetworkSecurityMethod` enum nick**.
///
/// A string that is not a nick fails silently, and the result is not obvious.
/// The transform returns `FALSE` and the binding sets nothing. Thus the settings
/// object keeps the default of the property, which in EDS 3.52 is
/// `STARTTLS_ON_STANDARD_PORT`. An account written as `"tls"` thus *does*
/// connect over TLS now, through a default that nobody selected. `jmap-mail`
/// only compares with `NONE`, so it sees encryption. But Evolution's account
/// editor shows an encryption setting that the user did not select. If a Camel
/// release changes that default to `NONE`, the same keyfile refuses to connect.
/// The nick makes the account tell its true meaning to both readers.
///
/// Of Camel's three nicks, this one describes HTTPS: TLS from the first byte,
/// with no in-band upgrade. Evolution's own server settings page writes the same
/// spelling back through the same binding. Thus an account that is committed
/// here and only opened in the editor does not change.
pub const MAIL_SECURITY_METHOD_TLS: &CStr = c"ssl-on-alternate-port";

/// The `[Security] Method` of a mail source when the connection is not encrypted.
///
/// Both readers use this spelling: `ESourceSecurity` documents `"none"` as "no
/// security", and the first value of `CamelNetworkSecurityMethod` has this nick.
pub const MAIL_SECURITY_METHOD_NONE: &CStr = c"none";

/// The three scratch sources of a commit: receiving account, identity, transport.
///
/// They are raw pointers because the setup that commits them owns them. [`apply`]
/// keeps none of them after the call.
pub struct MailSources {
    /// The receiving service of Evolution, and the root of the folder tree.
    pub account: *mut ESource,
    /// The sender of the mail: a person, not a service.
    pub identity: *mut ESource,
    /// The service that Evolution sends through.
    pub transport: *mut ESource,
}

/// Writes the three mail sources of the account, so that they match only this
/// account.
///
/// As [`crate::account::apply`] does, this writes all fields each time, not only
/// the changed ones. A commit writes to sources that already contain values, and
/// an old address that stays is the `From:` of each message sent after it.
///
/// # Safety
///
/// `collection` and all three sources in `sources` must be valid `ESource`s: the
/// account source and the three scratch sources that the setup commits. This call
/// takes no reference to any of them, and nothing here lives longer than the call.
pub unsafe fn apply(collection: *mut ESource, sources: &MailSources, account: &Account) {
    // `e_source_get_extension` looks up a name among the registered children of
    // `E_TYPE_SOURCE_EXTENSION`. It cannot find an unreferenced type, so it
    // creates nothing and returns NULL. `e_source_class_init` also calls
    // `g_type_ensure` on all built-in extensions, these four too. Each call here
    // costs one lookup and makes this module independent of EDS's built-in list.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_identity_get_type();
        e_source_mail_submission_get_type();
        e_source_mail_transport_get_type();
        // The two extensions of [`apply_server`]. They are not mail extensions,
        // but `e_source_class_init` also ensures them.
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // The string lives longer than each call that borrows it, so the pointers
    // below stay valid. As at each typed string that goes into C, an interior NUL
    // truncates the string. C callers read it like that anyway, and a refusal
    // would keep the *previous* address on an edited identity.
    let address = cstring_lossy(&account.identity);

    // SAFETY: valid sources by this function's contract. The account source owns
    // the uid, and the uid lives longer than the three setters that copy it. Each
    // extension is created on demand, and the source that it is requested from
    // owns it.
    unsafe {
        // The parent makes the three the account's mail, not three top-level
        // sources that the collection does not know.
        let account_uid = e_source_get_uid(collection);
        for source in [sources.account, sources.identity, sources.transport] {
            e_source_set_parent(source, account_uid);
        }

        let mail_account: *mut ESourceMailAccount =
            e_source_get_extension(sources.account, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
                .cast();
        // `ESourceMailAccount` and `ESourceMailTransport` derive from
        // `ESourceBackend`, which contains the Camel protocol.
        e_source_backend_set_backend_name(
            mail_account.cast::<ESourceBackend>(),
            MAIL_BACKEND_NAME.as_ptr(),
        );
        e_source_mail_account_set_identity_uid(mail_account, e_source_get_uid(sources.identity));

        let identity: *mut ESourceMailIdentity =
            e_source_get_extension(sources.identity, E_SOURCE_EXTENSION_MAIL_IDENTITY.as_ptr())
                .cast();
        e_source_mail_identity_set_address(identity, address.as_ptr());

        // A group of the identity, not a separate source: the route of a
        // person's outgoing mail is a property of the person.
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

    // The server of each of the two services. The identity connects to no server.
    for source in [sources.account, sources.transport] {
        // SAFETY: a valid source by this function's contract.
        unsafe { apply_server(source, &account.connection) };
    }
}

/// Writes the server of a mail service on the source that configures that service.
///
/// It is public because it is all that a `commit_changes` can write. [`apply`]
/// commits the *whole account*: three sources at once, from the one location that
/// has access to all three. But the vfunc that Evolution dispatches gets one
/// backend with one scratch source, and the account. Thus [`crate::backend::commit`]
/// calls this one time, on the service source that its backend is the candidate
/// for. The other data that [`apply`] writes is Evolution's own (the parent, the
/// two uid links, the identity's address). Or it is already on the source when
/// the vfunc runs: `e_mail_config_assistant` writes the service name when it
/// creates the scratch source.
///
/// No group here is a *Camel* group. Thus this writes two usual `ESource`
/// extensions and does not depend on `jmap-mail`. Host, port, user,
/// authentication mechanism and security method are the five
/// `CamelNetworkSettings` properties that `ESourceCamel` binds to *other*
/// extensions: `[Authentication]` and `[Security]` on the same source. Thus the
/// generated `[JMAP Backend]` group never contains them. The provider's own group
/// contains the remaining properties: for this provider, the inherited properties
/// of `CamelStoreSettings` and `CamelOfflineSettings`. This setup has no values
/// for them.
///
/// The values are the collection's own, from the same [`Connection`]. This is
/// necessary, not only consistent. EDS compares the two `[Authentication] Host`
/// strings to decide if a child source shares the password of its collection
/// (`e_util_can_use_collection_as_credential_source`, called from
/// `e_source_credentials_provider_ref_credentials_source`). The comparison lets an
/// account put its outgoing service on a different server with its own password.
/// Here, a different host (also a missing host when the collection has one) causes
/// a second password prompt for the same server. It also causes a second
/// libsecret entry that can become out of sync.
///
/// `[Authentication] Method` is a copy of the collection's value, not blank. A
/// blank value was a real bug. On a collection, it names the EDS credentials
/// provider. On a mail source, `ESourceCamel` binds it to
/// `CamelNetworkSettings:auth-mechanism`. JMAP has no SASL mechanism, and
/// `jmap-mail` gives a NULL mechanism to `camel_session_authenticate_sync`. But
/// `jmap-mail` *also* uses that `auth-mechanism` field to select the credential
/// type. The combo of [`crate::backend`] writes one of these values:
///
/// - `"none"` (password → Basic),
/// - [`API_TOKEN_METHOD`](jmap_backend_core::api_token::API_TOKEN_METHOD)
///   (`"bearer"` → an `Authorization: Bearer` of the pasted token),
/// - the OAuth 2.0 method.
///
/// `uses_api_token`/`uses_oauth2` of `jmap-mail` read it to select the
/// authentication. A mail source with a blank value *always* uses Basic and
/// cannot keep the account's Bearer or OAuth selection. An API-token Fastmail
/// account had this bug: the collection connected over Bearer, but its mail child
/// asked for a password again and again. Thus this writes the collection's own
/// method. `"none"` still gets to Camel as the absent mechanism: it is the value
/// that `ESourceCamel` converts NULL back to.
///
/// # Safety
///
/// `source` must be a valid `ESource`: one of the two service sources that the
/// setup commits. Nothing here lives longer than the call.
pub unsafe fn apply_server(source: *mut ESource, connection: &Connection) {
    // These live longer than each call that borrows them, as in `apply` above.
    let host = cstring_lossy(&connection.host);
    let user = connection.user.as_deref().map(cstring_lossy);
    let auth_method = connection.auth_method.as_deref().map(cstring_lossy);
    let security_method = if connection.secure {
        MAIL_SECURITY_METHOD_TLS
    } else {
        MAIL_SECURITY_METHOD_NONE
    };

    // SAFETY: a valid source by this function's contract, and header constants
    // that name extensions whose types `apply` registered. Each extension is
    // created on demand and the source owns it. Each setter copies its string.
    unsafe {
        let auth: *mut ESourceAuthentication =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_host(auth, host.as_ptr());
        e_source_authentication_set_user(auth, as_ptr(&user));
        e_source_authentication_set_method(auth, as_ptr(&auth_method));
        // Zero is "not set" for `[Authentication] Port` and for an unconfigured
        // `CamelNetworkSettings`. Thus the two ends of the binding also agree
        // when there is no value.
        e_source_authentication_set_port(auth, connection.port.unwrap_or(0));

        let security: *mut ESourceSecurity =
            e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
        e_source_security_set_method(security, security_method.as_ptr());
    }
}
