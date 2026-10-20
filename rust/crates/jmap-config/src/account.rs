// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account: the collection `ESource` that a setup commits.
//!
//! This is the inverse of [the reader of the collection backend][collection_source],
//! which reads the same three groups:
//!
//! - `[Collection]`: which parts the account offers,
//! - `[Authentication]`: where its server is and which user logs in,
//! - `[Security]`: whether the connection uses TLS.
//!
//! All that M6 does with an account, it does with what this module writes.
//!
//! ## Here `e_source_get_extension` is necessary for what it does
//!
//! The reader never calls it: it *creates* an extension that it cannot find,
//! and the reader gets the user's own account file. This module is the one
//! place where the purpose is to create the groups. `[Collection]` makes the
//! file an account and not a lone address book. Without it, the file is not a
//! broken account: `e_source_registry_server_ref_backend_factory` never gives
//! it to a collection factory.
//!
//! ## Each field is written, also the absent ones
//!
//! [`apply`] writes all eleven properties each time. It writes NULL for a string
//! that the account does not have, and 0 for a port that it does not name.
//! [The child writer][child_source] is different: it gets a *fresh* child, and
//! can leave a property unchanged if it has no value for it.
//!
//! A setup commits onto a source that already has values. An account in edit
//! has the old server, and `EMailConfigServiceBackend` commits into a scratch
//! copy of it. Thus "the user cleared the user name" must get to the file as
//! an empty `User=`. If the write was skipped, the old name would stay. The
//! account that the user made anonymous would then ask libsecret for a
//! password under a deleted name. Thus [`apply`] is idempotent in the strong
//! sense: after it, the source contains this [`Account`] and nothing of its
//! previous values.
//!
//! The one field that cannot be cleared is `[Authentication] Method`:
//! `ESourceAuthentication:method` has no unset state. A fresh extension
//! already reads `"none"`, and both NULL and `""` set it to that string (as
//! tested with the installed EDS). Thus an [`Account`] whose
//! [`auth_method`](Connection::auth_method) is `None` reads back as
//! `Some("none")`. The *meaning* is correct: the credentials provider of EDS
//! uses this value to find the password impl of the account, and "none" means
//! "ask for a password the usual way". But the value is not identical, and
//! `tests/account.rs` pins this.
//!
//! ## `credential-name`/`credential-store-id` is derived, not a field of `Account`
//!
//! `ESourceAuthentication:credential-name` gives the stored token of an OAuth
//! 2.0 account a key: the uid of the account. This key is stable when the
//! server moves. Two accounts on the same host never share it, which the
//! previous host-derived key could do (eds#663).
//!
//! It is not a field of [`Account`], because a dialog cannot usefully show it
//! and a user cannot usefully edit it. It is exactly the
//! [`auth_method`](Connection::auth_method) of the account as a yes/no. Thus
//! [`apply`] derives it each time and does not trust a copy.
//!
//! It is written only for OAuth 2.0, and cleared (NULL) for all other methods.
//! The credentials engine of EDS uses a non-empty `credential-name` as the
//! parameter name that it gives to `authenticate`. A password account with
//! one would thus look up its password under the wrong key. `child_added` and
//! `mail_child` of `jmap-backend-collection` bind it from the account onto each
//! child, as they bind `host`/`user`/`method`. Thus all services of the account
//! share one token cache entry.
//!
//! eds#663 (MR 247) later moved the credentials engine to a property with a
//! different name, `credential-store-id` (`Since: 3.64`, not released at the
//! time of writing). The value and the derivation are the same. The setter
//! that the installed EDS has decides which property this module writes.
//! `#[cfg(eds_credential_store_id)]` selects it, and the `build.rs` of
//! `eds-sys` sets that cfg from the headers. Builds before 3.64 continue to
//! write `credential-name`.
//!
//! ## `[Security]` is written as the method, and read as the boolean
//!
//! `ESourceSecurity` holds a string ("tls" or "none") and derives
//! `ESourceSecurity:secure` from it. Each JMAP backend reads the boolean. The
//! write uses `e_source_security_set_method()` for the same reason as in
//! [`child_source`]: the keyfile contains the string, thus the string must
//! have the correct spelling. A test that reads the boolean back finds an
//! incorrect spelling.
//!
//! ## What this module does not write
//!
//! - **`[Data Source] DisplayName`**: the name that the user gives the account.
//!   The assistant page of Evolution sets it. A write from here would rename
//!   the account on each commit, back to the name from the server settings
//!   page.
//! - **`Enabled`**: a new `ESource` is enabled. The flag of an existing source
//!   is the user's answer to "show this account", not the answer of the setup.
//! - **the mail sources**: see [`crate`]. The `[Mail Account]`,
//!   `[Mail Identity]` and `[Mail Transport]` of a JMAP account are three more
//!   sources, not three more groups in this source.
//! - **[`Account::oauth2_registered`]**:
//!   [`crate::oauth2_setup::discover_and_register`] writes the extension of
//!   [`crate::oauth2`], not this function. [`read`] reads it because
//!   [`crate::complete::check`] needs it, but [`apply`] does not change it, as
//!   it does not change the mail sources.
//!
//! The three links above go to `jmap-backend-collection`. That crate is a
//! dev-dependency here: the tests link it, the library must not. Thus they are
//! paths into the generated documentation, not intra-doc links.
//! `jmap-backend-collection` links to `SourceConfig` of `jmap-backend-core` in
//! the same way.
//!
//! [collection_source]: ../../jmap_backend_collection/collection_source/index.html
//! [child_source]: ../../jmap_backend_collection/child_source/index.html
//! [`child_source`]: ../../jmap_backend_collection/child_source/index.html
//! [factory]: ../../jmap_backend_collection/factory/constant.FACTORY_NAME.html

use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceBackend, ESourceCollection, ESourceSecurity,
    e_source_authentication_dup_host, e_source_authentication_dup_method,
    e_source_authentication_dup_user, e_source_authentication_get_port,
    e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_backend_set_backend_name,
    e_source_collection_dup_identity, e_source_collection_get_calendar_enabled,
    e_source_collection_get_contacts_enabled, e_source_collection_get_mail_enabled,
    e_source_collection_get_type, e_source_collection_set_calendar_enabled,
    e_source_collection_set_contacts_enabled, e_source_collection_set_identity,
    e_source_collection_set_mail_enabled, e_source_get_extension, e_source_get_uid,
    e_source_security_get_secure, e_source_security_get_type, e_source_security_set_method,
};
// The token-cache-key setter that the installed EDS has. See the module
// comment on `credential-name`/`credential-store-id`.
#[cfg(not(eds_credential_store_id))]
use eds_sys::e_source_authentication_set_credential_name as set_credential_id;
#[cfg(eds_credential_store_id)]
use eds_sys::e_source_authentication_set_credential_store_id as set_credential_id;
use glib_sys::{GFALSE, GTRUE, gboolean};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::{extension_if_present, take_string};
use jmap_backend_core::oauth2::{OAUTH2_AUTH_METHOD, OAUTH2_METHOD};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

/// The name with which the registry finds the collection factory of this
/// account.
///
/// `[Collection] BackendName` is not a description. The registry keeps each
/// collection factory under `"<factory_name>:Collection"` and looks up the key
/// made from this string. A value that does not match the factory name causes
/// no error: the account shows in the sidebar with no children and no log
/// entry. `tests/account.rs` compares it with
/// [the `FACTORY_NAME` of the collection factory](../../jmap_backend_collection/factory/constant.FACTORY_NAME.html),
/// which is the name that the module registers.
pub const BACKEND_NAME: &str = "jmap";

/// An account, in the shape that the setup has before it is a source.
///
/// The fields use the types of the reader, not new types. [`Connection`] is
/// what [`server_of` of the collection backend](../../jmap_backend_collection/collection_source/fn.server_of.html)
/// returns and what each child of the account gets. [`Parts`] controls each
/// listing and each removal. Thus there is one description of an account:
/// this module writes it, and the reader reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `[Collection] Identity`: who the account is. For JMAP, this is the
    /// email address that the user typed on the identity page.
    ///
    /// This is not the same field as `[Authentication] User`, and it is
    /// deliberately not derived from it. The mail address of a user and the
    /// login name are frequently the same string, but not always.
    pub identity: String,
    /// Where the server is, and as which user the account authenticates.
    pub connection: Connection,
    /// Which of mail, contacts and calendars the account offers: the three
    /// check boxes, and the children that the collection backend makes.
    pub parts: Parts,
    /// Whether `[JMAP OAuth2]` already names a registered client id.
    ///
    /// For a setup, this field is read-only: [`apply`] never writes it. Only
    /// [`crate::oauth2_setup::discover_and_register`] writes it, onto its own
    /// extension, independently of the entries that the widgets of this crate
    /// bind. With this field, [`crate::complete::check`] can tell "OAuth 2.0
    /// selected, client registered" from "OAuth 2.0 selected, no client yet".
    /// It needs only a plain [`Account`], not an `ESource`.
    pub oauth2_registered: bool,
}

/// Writes `account` onto `source`. After the call, `source` contains exactly
/// that account.
///
/// # Safety
///
/// `source` must be a valid `ESource`: the collection source that the setup
/// commits into. This call takes no reference to it, and nothing here stays
/// alive after the call.
pub unsafe fn apply(source: *mut ESource, account: &Account) {
    // As at each lookup of an extension by name: `e_source_get_extension`
    // searches the registered children of `E_TYPE_SOURCE_EXTENSION`. It cannot
    // find a type that nothing referenced yet, and here it would then create
    // nothing and return NULL. A reference to the GType registers it.
    // SAFETY: no arguments, and the type system initializes itself.
    unsafe {
        e_source_collection_get_type();
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // The strings stay alive longer than each call that borrows them. This
    // keeps the pointers below valid. An inline `cstring_lossy(…).as_ptr()`
    // would point into a temporary that is dropped at the end of the statement.
    // As at each place where a typed string goes into C, an interior NUL
    // truncates the string, and the write is not refused. The kept part is what
    // the string means to each C caller downstream. A refused write would give
    // an account with no host, which makes all operations fail, not one field.
    let identity = cstring_lossy(&account.identity);
    let host = cstring_lossy(&account.connection.host);
    let user = account.connection.user.as_deref().map(cstring_lossy);
    let auth_method = account.connection.auth_method.as_deref().map(cstring_lossy);
    let security_method = if account.connection.secure {
        c"tls"
    } else {
        c"none"
    };
    let backend_name = cstring_lossy(BACKEND_NAME);

    // SAFETY: a valid source by this function's contract, and header constants
    // that name extensions whose types the code above registers. The source
    // creates each extension on demand and owns it. Each setter copies the
    // string that it gets.
    unsafe {
        let collection: *mut ESourceCollection =
            e_source_get_extension(source, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
        // `ESourceCollection` derives from `ESourceBackend`, which holds the
        // name that the registry uses as the key for the factory.
        e_source_backend_set_backend_name(
            collection.cast::<ESourceBackend>(),
            backend_name.as_ptr(),
        );
        e_source_collection_set_identity(collection, identity.as_ptr());
        e_source_collection_set_mail_enabled(collection, gboolean(account.parts.mail));
        e_source_collection_set_contacts_enabled(collection, gboolean(account.parts.contacts));
        e_source_collection_set_calendar_enabled(collection, gboolean(account.parts.calendars));

        let auth: *mut ESourceAuthentication =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_host(auth, host.as_ptr());
        // For these two, write NULL, do not skip the write. See the module
        // comment on a commit onto an account that already has values. The
        // string setters of EDS accept NULL: the getter then returns NULL. The
        // reader gets the same "no user" from an account that never had one.
        e_source_authentication_set_user(auth, as_ptr(&user));
        e_source_authentication_set_method(auth, as_ptr(&auth_method));
        // Zero means "not set" for `[Authentication] Port`. The reader changes
        // it back into `None`.
        e_source_authentication_set_port(auth, account.connection.port.unwrap_or(0));

        // `credential-name`/`credential-store-id` (eds#663): only the uid of
        // the account, and only for OAuth 2.0 (see the module comment). For
        // all other methods, write NULL, do not skip the write. The reason is
        // the same idempotency as for the fields above. An account that
        // changes back to a password must not keep the key of a previous
        // OAuth 2.0 commit. That key would change the parameter name that the
        // credentials engine gives to `authenticate` for a plain password.
        //
        // `OAUTH2_METHOD` ("OAuth2") is the generic alias. `OAUTH2_AUTH_METHOD`
        // is the method that real accounts have, because the setup combo of
        // `backend.rs` and the discovery of `config_lookup.rs` write it. A
        // check for only the alias gave no key to all real OAuth 2.0 accounts,
        // and eds#663 silently stayed unfixed. A check for the literal "JMAP"
        // would cause that bug again on an EDS where the method is
        // "OAuth2Dynamic". Thus this check, the check in `complete.rs`, and the
        // writers in `backend.rs` and `config_lookup.rs` all use the constant.
        let oauth2_auth_method = OAUTH2_AUTH_METHOD
            .to_str()
            .expect("OAUTH2_AUTH_METHOD is a fixed ASCII string");
        let connection_auth_method = account.connection.auth_method.as_deref();
        let credential_id = if connection_auth_method == Some(OAUTH2_METHOD)
            || connection_auth_method == Some(oauth2_auth_method)
        {
            e_source_get_uid(source)
        } else {
            ptr::null()
        };
        set_credential_id(auth, credential_id);

        let security: *mut ESourceSecurity =
            e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
        e_source_security_set_method(security, security_method.as_ptr());
    }
}

/// The account that `source` contains now: the inverse of [`apply`].
///
/// The widgets get their values from it, and [`check`](crate::complete::check)
/// examines it. `EMailConfigServiceBackend` gives its vfuncs a collection
/// source, and each question after `new_collection` is about the account in
/// that source.
///
/// This function is deliberately total. [`server_of`] returns a `Result`,
/// because a backend that opens a connection cannot use an account without a
/// host. This function runs on each keystroke of a dialog that the user still
/// fills in, where "no host yet" is the usual state and not a failure. Thus it
/// returns the account as it is, and [`check`](crate::complete::check) (which
/// exists for this purpose) says whether it can be committed.
///
/// # What an absent group means
///
/// Nothing here creates an extension: this function gets the user's own
/// account file, and `e_source_get_extension` would add the group. It tests for
/// each group. For an absent group, it gives the same answer as
/// [the reader of the collection backend][collection_source]:
///
/// - no `[Collection]` is [`Parts::ALL`], through [`Parts::from_collection`];
/// - no `[Security]` is TLS. `ESourceSecurity:secure` reads FALSE when there is
///   no group, and a dialog that committed *that* value would disable TLS on a
///   hand-written account;
/// - no `[Authentication]` is the empty host and no user, which is what an
///   empty group gives.
///
/// The host and the identity change form: the absent-or-non-empty `NULL` of the
/// keyfile becomes the empty string of an unfilled entry. For the host, this is
/// the translation of [`check`](crate::complete::check) in reverse.
///
/// # Where this is deliberately different from the reader of the registry
///
/// [`parts_of`] includes the `Enabled` flag of the source, because a disabled
/// account has no parts to populate. This function does not. `enabled` is not
/// a field of [`Account`], and [`apply`] writes all three switches each time.
/// Otherwise an account that the user hid would come back with three cleared
/// check boxes, and a commit would write them. Thus "hide this for now" would
/// lose which parts the account offered. `tests/account.rs` pins both answers.
///
/// # Safety
///
/// `source` must be a valid `ESource`: the collection source that the setup
/// edits. This function only reads it, and nothing stays alive after the call.
///
/// [collection_source]: ../../jmap_backend_collection/collection_source/index.html
/// [`parts_of`]: ../../jmap_backend_collection/collection_source/fn.parts_of.html
/// [`server_of`]: ../../jmap_backend_collection/collection_source/fn.server_of.html
pub unsafe fn read(source: *mut ESource) -> Account {
    // As in `apply`, and for the same reason: `e_source_has_extension` also
    // cannot find an extension GType that nothing referenced yet.
    // SAFETY: no arguments, and the type system initializes itself.
    unsafe {
        e_source_collection_get_type();
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // SAFETY: a valid source by this function's contract, and each header
    // constant names an extension of the type that the code reads.
    let collection =
        unsafe { extension_if_present::<ESourceCollection>(source, E_SOURCE_EXTENSION_COLLECTION) };
    // SAFETY: as above.
    let security =
        unsafe { extension_if_present::<ESourceSecurity>(source, E_SOURCE_EXTENSION_SECURITY) };
    // SAFETY: as above.
    let auth = unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    };

    // SAFETY: a live extension, by the contract of `extension_if_present`.
    // Each `dup` getter returns NULL or a copy that the caller owns. A config
    // lookup reads on a worker thread, thus a borrowed string can be freed
    // during the copy.
    let (identity, parts) = collection.map_or((None, None), |collection| unsafe {
        (
            take_string(e_source_collection_dup_identity(collection)),
            Some(Parts {
                mail: e_source_collection_get_mail_enabled(collection) != GFALSE,
                contacts: e_source_collection_get_contacts_enabled(collection) != GFALSE,
                calendars: e_source_collection_get_calendar_enabled(collection) != GFALSE,
            }),
        )
    });

    // SAFETY: as above.
    let secure =
        security.is_none_or(|security| unsafe { e_source_security_get_secure(security) != GFALSE });

    // SAFETY: as above.
    let (host, port, user, auth_method) = auth.map_or((None, 0, None, None), |auth| unsafe {
        (
            take_string(e_source_authentication_dup_host(auth)),
            e_source_authentication_get_port(auth),
            take_string(e_source_authentication_dup_user(auth)),
            take_string(e_source_authentication_dup_method(auth)),
        )
    });

    // SAFETY: a valid source by this function's contract. `oauth2::read`
    // creates nothing, and returns `Config::default()` (no client id) for a
    // source with no `[JMAP OAuth2]` extension yet.
    let oauth2_registered = unsafe { crate::oauth2::read(source) }.client_id.is_some();

    Account {
        identity: identity.unwrap_or_default(),
        connection: Connection {
            host: host.unwrap_or_default(),
            // Zero means "not set" in the keyfile, as `apply` writes it.
            port: (port != 0).then_some(port),
            user,
            auth_method,
            secure,
        },
        // `true`, not the flag of the source. See the note above on the one
        // deliberate difference from the reader of the registry.
        parts: Parts::from_collection(true, parts),
        oauth2_registered,
    }
}

/// The string in a `CString`, or NULL if there is no `CString`.
pub(crate) fn as_ptr(value: &Option<std::ffi::CString>) -> *const std::ffi::c_char {
    value.as_ref().map_or(ptr::null(), |value| value.as_ptr())
}

/// A Rust `bool` as the C `gboolean` that the setters of EDS take.
fn gboolean(value: bool) -> gboolean {
    if value { GTRUE } else { GFALSE }
}
