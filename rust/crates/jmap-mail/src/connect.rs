// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection of a JMAP store, without GObject code.
//!
//! [`crate::server`] reads the account from the settings of the store. This
//! module makes a live [`MailSync`] from that account and classifies its
//! failures. It is the mail equivalent of the [`connect`] module of
//! `jmap-backend-core`, not a caller of it. The reason is that a Camel service
//! must give two answers in the terms of Camel:
//!
//! - **The authentication result.** `CamelAuthenticationResult` has three
//!   values and `ESourceAuthenticationResult` has four. The missing value is
//!   `REQUIRED`. A store cannot say "ask the user before I try", so it tries.
//!   An account with a user and no password connects without credentials. The
//!   server answers 401, and [`CAMEL_AUTHENTICATION_REJECTED`] makes that 401
//!   into a prompt. The EDS side refuses before it connects. Each side is
//!   correct for the framework that it answers to.
//! - **The error domain.** Camel does not read `E_CLIENT_ERROR`. It reads
//!   `CAMEL_SERVICE_ERROR`, and specially [`CAMEL_SERVICE_ERROR_UNAVAILABLE`].
//!   That code is the mail equivalent of `E_CLIENT_ERROR_REPOSITORY_OFFLINE`.
//!   It makes a store use its summary cache, where other codes show the account
//!   as broken. `G_IO_ERROR_CANCELLED` is the exception, because it belongs to
//!   GLib and not to Camel or EDS. Each caller in Camel tests for it first.
//!
//! [`connect`]: jmap_backend_core::connect
//!
//! The two sides must give the same answer to one question: which failure
//! means a wrong password. Thus this module calls
//! [`jmap_backend_core::connect::is_wrong_password`] and does not copy it. A
//! store that treats a 403 as a wrong password causes a prompt loop that no
//! password can stop.
//!
//! This module does not read the password, as [`crate::server`] does not.
//! Camel gets the password through the `CamelSession` at connect time and
//! gives it to the service. A JMAP account must never take a credential from
//! a settings object, because Evolution writes those objects to a config file.

use std::fmt;

use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_FOLDER_ERROR_INSUFFICIENT_PERMISSION, CAMEL_FOLDER_ERROR_INVALID,
    CAMEL_FOLDER_ERROR_INVALID_UID, CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
    CAMEL_SERVICE_ERROR_INVALID, CAMEL_SERVICE_ERROR_NOT_CONNECTED,
    CAMEL_SERVICE_ERROR_UNAVAILABLE, CAMEL_SERVICE_ERROR_URL_INVALID, CAMEL_STORE_ERROR_NO_FOLDER,
    CamelAuthenticationResult, CamelServiceError, camel_folder_error_quark,
    camel_service_error_quark, camel_store_error_quark,
};
use glib_sys::{GError, GQuark, g_error_new_literal};
use jmap_backend_core::connect::is_wrong_password;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::source::{self, SourceError};
use jmap_client::{Credentials, Error};
use jmap_mail_sync::{FolderRole, MailSync, SyncError};
use jmap_proto::session::CAPABILITY_MAIL;

use crate::server::ServerConfig;

/// What a store reports when the connection came up.
pub const ACCEPTED_AUTHENTICATION: CamelAuthenticationResult = CAMEL_AUTHENTICATION_ACCEPTED;

/// A JMAP mail account that could not be opened.
#[derive(Debug)]
pub enum StoreError {
    /// The settings do not give a server that the store can contact.
    ///
    /// This is a separate variant, not a client error, because only an edit of
    /// the account can fix it, never a retry. As `CAMEL_SERVICE_ERROR_UNAVAILABLE`,
    /// Evolution tries to connect the store again without end.
    Config(SourceError),
    /// The server answered with a session that shows that it did not see the
    /// credentials of this connection. The session has an empty user name and
    /// no accessible accounts. [`Client::is_anonymous`] reads this.
    ///
    /// This is not a server with no data. It is a server that did not
    /// authenticate this request at all. A conformant server answers 401 in
    /// this case, and [`is_wrong_password`] makes that 401 into
    /// [`CAMEL_AUTHENTICATION_REJECTED`] below. Real Stalwart does not send the
    /// 401 (RFC 8620 §2/§8.1; Finding 12 in `STALWART-RFC-FINDINGS.md`). It
    /// answers 200 OK with the anonymous session that it gives to a request with
    /// no `Authorization` header. `password_credentials` and
    /// `bearer_credentials` send exactly that request for an account with a user
    /// and no stored password. They expect the 401 that the module doc says
    /// becomes a prompt. Without this variant, the 200 gives no primary account
    /// and a generic `ERROR` with no prompt. Thus the first connection of a new
    /// account fails and does not ask for a password. [`open_mail`] checks for
    /// this before it finds the account. Thus a server with no mail capability
    /// (still an authenticated session) does not get this error.
    ///
    /// [`Client::is_anonymous`]: jmap_client::Client::is_anonymous
    Unauthenticated,
    /// The server refused, failed, or is unreachable.
    Client(Error),
    /// The account uses OAuth 2.0 and the store could not get an access token.
    /// Possibly no user gave consent yet, or the refresh failed. The string is
    /// the EDS message for the failure.
    ///
    /// The result is `CAMEL_AUTHENTICATION_ERROR`. This is the opposite of
    /// the choice of `jmap_backend_core::connect::ConnectError::OAuth2` for the
    /// same failure on the EDS side. That side has a fourth
    /// `ESourceAuthenticationResult` value, `REQUIRED`, which opens the consent
    /// window and keeps the stored refresh token. `CamelAuthenticationResult`
    /// has only three values. Here, `REJECTED` means "ask again" *and* "the
    /// credentials are wrong". [`Self::authentication_result`] accepts the same
    /// double meaning for a usual wrong password. With `REJECTED`, a temporary
    /// failure opens the interactive consent dialog each time it occurs. An
    /// example is a network problem during a token fetch, not a revoked grant.
    /// An expired refresh token opens the same dialog. `ERROR` is the safe
    /// choice that this file makes for each other failure that is not a 401.
    /// This failure is one of those, not a new type of prompt.
    OAuth2(String),
    /// The store got a request that needs a server, and the store has no
    /// connection.
    ///
    /// This is not a failure of the account. Camel uses a store that it
    /// *thinks* is connected, and that state can become old. Examples are a
    /// `disconnect_sync` on a different thread, or a reconnect that did not
    /// occur yet. The code is `CAMEL_SERVICE_ERROR_NOT_CONNECTED`. With this
    /// code, Camel connects and asks again, and does not show the account as
    /// broken.
    Disconnected,
    /// Camel asked to open a folder that the account does not have.
    ///
    /// The domain is `CAMEL_STORE_ERROR`, not `CAMEL_SERVICE_ERROR`. This is
    /// the one location where this type does not use the service domain. The
    /// connection and the account are correct. A service error shows a working
    /// account as broken because one folder is missing.
    NoFolder(String),
    /// Camel asked for a folder by purpose (the inbox, the trash, the junk),
    /// and no mailbox of the account has that role.
    ///
    /// The account is valid, not broken: RFC 8621 §2 lets `role` be null on
    /// each mailbox. But the request has no answer. A mailbox *named* "Inbox"
    /// or "Trash" is not a fallback, because then the provider guesses where
    /// mail arrives and where deleted mail goes. The report is the same as for
    /// [`Self::NoFolder`], because this is that case: Camel asked for a folder
    /// that the account does not have.
    NoRole(FolderRole),
    /// Camel asked by uid for a message that the account does not have.
    ///
    /// The domain is the folder domain, not the store domain. The store and
    /// the folder are correct. A uid comes from the last listing, and a
    /// different client can delete the message after it. That is usual, not a
    /// fault. The code is `CAMEL_FOLDER_ERROR_INVALID_UID`. Evolution reads it
    /// as "that message is gone", not as a reason to set the account offline.
    NoMessage(String),
    /// An append tried to put a message into a mailbox with
    /// `myRights.mayAddItems` `false` in its last listing (RFC 8621 §2). An
    /// example is a read-only shared mailbox. The string is the display name of
    /// the mailbox.
    ///
    /// The code is `CAMEL_FOLDER_ERROR_INSUFFICIENT_PERMISSION`. The
    /// `append_message_sync` of `camel-nntp-folder.c` uses this code for the
    /// same refusal. The account and the connection are correct. The store does
    /// not ask the server, because the last listing already gives the answer.
    NoPermission(String),
    /// A send uses a from address for which the account has no identity.
    ///
    /// The code is `CAMEL_SERVICE_ERROR_INVALID`, as for the refusals of
    /// [`crate::envelope`], and intentionally not `UNAVAILABLE`. The account
    /// and the connection are correct, and the send fails the same way on a
    /// working server. Only a send from an address of the account fixes it,
    /// never a retry.
    NoIdentity(String),
    /// A send needs a mailbox for the outgoing message, and the account has no
    /// such mailbox.
    ///
    /// The code is `CAMEL_SERVICE_ERROR_INVALID`, as for [`Self::NoIdentity`]
    /// and for the same reasons. The account and the connection are correct,
    /// and a retry does not help. This is intentionally *not* the store code
    /// `NO_FOLDER` of [`Self::NoRole`], although both are about a role that no
    /// mailbox has. A `CamelTransport` gives this error, and a transport is not
    /// a store. Also, Camel did not ask for a folder that can be missing.
    NoOutgoingFolder,
    /// A call to `get_quota_info_sync` found no `Quota` object of the account
    /// that applies to Mail.
    ///
    /// This is not a failure of the connection or the account. A server
    /// without the `urn:ietf:params:jmap:quota` capability, or with no quota
    /// for Mail, correctly answers "no quota information available". The
    /// `get_quota_info_sync` of `camel-imapx-folder.c` reports the same case
    /// in the same way. The code is `G_IO_ERROR_NOT_SUPPORTED`, the domain and
    /// code of that file. Thus the folder-properties dialog, which reads this
    /// vfunc, shows a JMAP account as it shows an IMAP account without quotas.
    NoQuota,
    /// A call to `get_filename` found no local message cache for a path in
    /// this folder, or got a uid for which this cache never makes a file. The
    /// cache is missing when the folder could not make the cache directory of
    /// the account at build time (see `crate::cache`).
    ///
    /// This is not a failure of the connection or the account. Each caller of
    /// `camel_folder_get_filename` reads a NULL answer as "no local copy" and
    /// reads the message in a different way. "View Source" in
    /// `e-mail-formatter-source.c` is an example. The code is
    /// `G_IO_ERROR_NOT_SUPPORTED`, the code of [`Self::NoQuota`] for the same
    /// type of "no data".
    NoLocalFile,
}

impl From<SourceError> for StoreError {
    fn from(error: SourceError) -> Self {
        Self::Config(error)
    }
}

impl From<SyncError> for StoreError {
    /// A sync failure is a client failure. `SyncError` keeps
    /// [`jmap_client::Error`] unchanged across the crate boundary. Here it
    /// becomes a `CAMEL_SERVICE_ERROR`.
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Client(error) => Self::Client(error),
            // Not a client failure. The sync layer gives this its own variant
            // for the mapping below, so do not merge it into a client failure.
            SyncError::NoSuchMessage(uid) => Self::NoMessage(uid.as_str().to_owned()),
            // The same, one level up. At other locations `NoFolder` holds a
            // *path*, because Camel asks with a path. Here it holds the mailbox
            // id, because the caller gave the id, and the path can be different
            // now. In both cases the store tells Camel that a listed folder is
            // missing. A caller in C reads only that part.
            SyncError::NoSuchFolder(id) => Self::NoFolder(id.as_str().to_owned()),
            // An address that the account cannot send from is a message for the
            // user with that address, not a server failure. It stays unchanged
            // up to the `GError`.
            SyncError::NoIdentity(address) => Self::NoIdentity(address),
            // This account cannot send at all, not only from one address. The
            // variant holds no data in either crate, because the missing mailbox
            // does not exist and thus has no id or path.
            SyncError::NoOutgoingFolder => Self::NoOutgoingFolder,
        }
    }
}

impl From<Error> for StoreError {
    fn from(error: Error) -> Self {
        Self::Client(error)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::Unauthenticated => f.write_str(&translate(
                // TRANSLATORS: shown when the server did not recognise this
                // connection as authenticated at all, rather than rejecting a
                // specific password.
                c"the server did not accept the account's credentials",
            )),
            Self::Client(error) => error.fmt(f),
            Self::OAuth2(message) => f.write_str(message),
            Self::Disconnected => f.write_str(&translate(
                // TRANSLATORS: shown when Camel asks a JMAP store to do
                // something that needs a connection it does not currently have.
                c"not connected to the JMAP server",
            )),
            Self::NoFolder(path) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the folder path Camel asked for.
                c"no such folder: %1$s",
                &[path.as_str()],
            )),
            Self::NoRole(role) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is a JMAP mailbox role such as "inbox" or
                // "trash", not translated: it is a protocol keyword, not prose.
                c"no mailbox of this account has the %1$s role",
                &[role.as_jmap()],
            )),
            Self::NoMessage(uid) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the message id Camel asked for.
                c"no such message: %1$s",
                &[uid.as_str()],
            )),
            Self::NoPermission(name) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the folder's display name.
                c"you do not have permission to add messages to %1$s",
                &[name.as_str()],
            )),
            Self::NoIdentity(address) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is the email address the message was to be
                // sent from.
                c"this account cannot send mail as %1$s",
                &[address.as_str()],
            )),
            Self::NoOutgoingFolder => f.write_str(&translate(
                c"this account has no Drafts or Sent folder to send a message from",
            )),
            Self::NoQuota => f.write_str("no quota information available for this account"),
            Self::NoLocalFile => f.write_str("this message has no local file"),
        }
    }
}

impl std::error::Error for StoreError {}

impl StoreError {
    /// The answer of the `authenticate_sync` vfunc.
    ///
    /// Only `REJECTED` makes Camel discard the stored password and ask again.
    /// Thus it is only for the one failure that a new password can fix. The EDS
    /// backends use the same rule, through the same function.
    pub fn authentication_result(&self) -> CamelAuthenticationResult {
        match self {
            Self::Client(error) if is_wrong_password(error) => CAMEL_AUTHENTICATION_REJECTED,
            // The same result as a real 401, for the Finding 12 replacement of
            // a 401. See the doc comment of the variant.
            Self::Unauthenticated => CAMEL_AUTHENTICATION_REJECTED,
            // `Self::OAuth2` comes here intentionally. Its doc comment tells
            // why this is the safer one of the two possible values.
            _ => CAMEL_AUTHENTICATION_ERROR,
        }
    }

    /// Changes a server 401 on an OAuth 2.0 bearer token into the `OAuth2`
    /// failure that a failed *fetch* of that token gets.
    ///
    /// Call this only when the last credentials were an OAuth 2.0 access token.
    /// `service::attempt` is the one caller. It knows this from the account
    /// field that [`oauth2::uses_oauth2`] reads. This function keeps a 401 on a
    /// Basic or API-token attempt. There, [`is_wrong_password`] correctly means
    /// a wrong password, and `REJECTED` (ask again) is the correct answer.
    ///
    /// For OAuth 2.0, `REJECTED` is not correct. The doc of `access_token`
    /// tells why a failure to *get* a token is `OAuth2`/`ERROR`, not
    /// `REJECTED`. `CamelAuthenticationResult` has no fourth value for "ask
    /// again and keep the stored data", as `ESourceAuthentication
    /// Result::REQUIRED` has. Thus with `REJECTED`, the credentials prompter
    /// discards the stored refresh token and opens a new consent screen. That
    /// is correct for a revoked grant. It is too disruptive for a token that
    /// the server refused one time, for example temporarily or just before EDS
    /// refreshes it. A 401 *after* the store got a token has the same two
    /// possible causes as a *failure to get one*. Thus it gets the same answer:
    /// report it, and do not discard the token.
    ///
    /// [`oauth2::uses_oauth2`]: crate::oauth2::uses_oauth2
    /// [`is_wrong_password`]: jmap_backend_core::connect::is_wrong_password
    pub fn reclassify_oauth2_rejection(self) -> Self {
        match self {
            Self::Client(error) if is_wrong_password(&error) => Self::OAuth2(error.to_string()),
            other => other,
        }
    }

    /// Allocates a `GError` for this failure. The caller owns it. Free it with
    /// `g_error_free`, or give it to a C caller that frees it.
    pub fn to_gerror(&self) -> *mut GError {
        let message = cstring_lossy(&self.to_string());
        let (domain, code) = self.gerror_code();

        // SAFETY: a live quark, a code from that domain's own enum, and a
        // NUL-terminated message the call copies.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    /// The domain and code a caller in C branches on.
    fn gerror_code(&self) -> (GQuark, i32) {
        match self {
            // The user pressed Stop. This domain belongs to GLib, not to Camel
            // or to this crate. Each layer above reads this code. A service error makes
            // a cancelled folder refresh into an alert.
            //
            // SAFETY: the quark functions take no arguments and register
            // themselves on first use.
            Self::Client(Error::Cancelled) => unsafe {
                (gio_sys::g_io_error_quark(), gio_sys::G_IO_ERROR_CANCELLED)
            },
            // SAFETY: as above.
            Self::NoQuota | Self::NoLocalFile => unsafe {
                (
                    gio_sys::g_io_error_quark(),
                    gio_sys::G_IO_ERROR_NOT_SUPPORTED,
                )
            },
            // SAFETY: as above.
            Self::NoFolder(_) | Self::NoRole(_) => unsafe {
                (
                    camel_store_error_quark(),
                    CAMEL_STORE_ERROR_NO_FOLDER as i32,
                )
            },
            // SAFETY: as above.
            Self::NoMessage(_) => unsafe {
                (
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INVALID_UID as i32,
                )
            },
            // SAFETY: as above.
            Self::NoPermission(_) => unsafe {
                (
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INSUFFICIENT_PERMISSION as i32,
                )
            },
            // A message larger than the `maxSizeUpload` of the account. The
            // account and the connection are correct, and only the message is
            // unusable. Thus the code is the code for a message that Camel
            // cannot write, not a service error. Evolution reads a service
            // error as an unusable account. The message text gives the limit.
            // The code must only show the correct cause.
            //
            // This is intentionally not `Error::RequestTooLarge`. That error
            // goes to the wildcard below and becomes a service error. It means
            // that the server gave an id too long for a request of the size
            // that the server set. That is an inconsistent account, not one
            // unusable message.
            //
            // SAFETY: as above.
            Self::Client(Error::TooLarge { .. }) => unsafe {
                (
                    camel_folder_error_quark(),
                    CAMEL_FOLDER_ERROR_INVALID as i32,
                )
            },
            // SAFETY: as above.
            _ => unsafe {
                (
                    camel_service_error_quark(),
                    self.service_error_code() as i32,
                )
            },
        }
    }

    fn service_error_code(&self) -> CamelServiceError {
        match self {
            Self::Config(_) => CAMEL_SERVICE_ERROR_URL_INVALID,
            Self::Disconnected => CAMEL_SERVICE_ERROR_NOT_CONNECTED,
            // The server is not reachable. Camel reads this code to set the
            // store offline instead of showing a wrong account. Thus this code
            // must not be generic.
            Self::Client(Error::Transport(_)) => CAMEL_SERVICE_ERROR_UNAVAILABLE,
            Self::Client(error) if is_wrong_password(error) => {
                CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE
            }
            // The same code as above, for the same reason. See the doc comment
            // of `Self::Unauthenticated`.
            Self::Unauthenticated => CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
            // Not a wrong password, but the same type of problem: the account
            // cannot prove its identity. This code is for that case. `INVALID`
            // only means "a part of this account is wrong".
            Self::OAuth2(_) => CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE,
            // A 403, a method error, a malformed response: the server answered.
            // Thus it is reachable and it accepted the credentials. The message
            // gives the details.
            _ => CAMEL_SERVICE_ERROR_INVALID,
        }
    }
}

/// The credentials from the user name of the account and the password that
/// Camel got from its session. `attempt` makes this password decision once
/// for each connect. [`crate::oauth2::access_token`] makes the OAuth 2.0
/// decision.
///
/// `password` is `None` for an account that had no prompt yet. Intentionally,
/// an account with no user also gets no credentials. The two cases send the
/// same request, because neither has data to send. A user name with an empty
/// password is not a weak credential, it is a wrong credential. A server that
/// counts failed attempts counts it.
pub fn password_credentials(user: Option<&str>, password: Option<&str>) -> Credentials {
    match (user, password) {
        (Some(user), Some(password)) => Credentials::basic(user, password),
        _ => Credentials::none(),
    }
}

/// The API-token version of [`password_credentials`], for
/// [`crate::api_token::uses_api_token`] accounts. The docs of that module tell
/// why `attempt` reads the `auth-mechanism` field for a third value. A token
/// has no user name. Thus an account that had no prompt yet sends the same
/// request with no credentials as `password_credentials`. The 401 of the
/// server then becomes the retry prompt.
pub fn bearer_credentials(password: Option<&str>) -> Credentials {
    match password {
        Some(password) => Credentials::bearer(password),
        None => Credentials::none(),
    }
}

/// Connects to the server of `config` and finds the account that holds its
/// mail.
///
/// `credentials` are already known. `attempt` decides between a password from
/// the Camel session and an OAuth 2.0 bearer token. It decides one time, so
/// that a store and a transport on one account use the same decision. See
/// [`crate::oauth2`].
pub fn open_mail(config: &ServerConfig, credentials: Credentials) -> Result<MailSync, StoreError> {
    // The client intentionally has no cancellation flag. A client lives as long
    // as the account, and nothing can clear a flag. Thus a flag from the
    // operation that opened the connection stays set forever after one Stop.
    // The scope that the vfunc installs (`jmap_backend_core::cancel::observe`)
    // cancels a JMAP operation. The client checks that scope first.
    // `false`: Camel keeps the server of this account in `CamelNetworkSettings`,
    // not in an `ESource` extension (the docs of `crate::server` tell why).
    // Thus there is no per-source rebase opt-in
    // (`jmap_backend_core::rebase::rebase_urls`) to read here.
    // `JMAP_LIVE_SERVER_REBASE_URLS` still applies through the OR with the
    // environment variable in `source::connect`.
    let client = source::connect(&config.target, false, credentials)?;
    // Check this before the account lookup. An anonymous session has no
    // capability. Thus a lookup first reports "no mail capability", not the
    // unauthenticated connection. See `StoreError::Unauthenticated`.
    if client.is_anonymous() {
        return Err(StoreError::Unauthenticated);
    }
    // Under `urn:ietf:params:jmap:mail`, as the address book backend finds its
    // account under `:contacts`. An account with `:contacts` and without
    // `:mail` is not a mail account. Without this check, the store shows it as
    // a mail account with no folders.
    let account_id = client.primary_account(CAPABILITY_MAIL)?;

    Ok(MailSync::new(client, account_id))
}
