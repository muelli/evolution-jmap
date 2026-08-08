// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the connection a JMAP store holds, with no GObject in sight.
//!
//! [`crate::server`] read the account off the store's settings; this turns that
//! into a live [`MailSync`], and classifies the ways it can fail. It is the
//! mail-side counterpart of `jmap-backend-core`'s [`connect`] module rather
//! than a caller of it, because both of the answers a Camel service has to give
//! are spelled in Camel's own vocabulary:
//!
//! - **The authentication verdict.** `CamelAuthenticationResult` has three
//!   values where `ESourceAuthenticationResult` has four, and the missing one
//!   is `REQUIRED`. There is no way for a store to say "ask the user before I
//!   try", so it does not try to: an account that names a user and has no
//!   password yet connects without credentials, the server answers 401, and
//!   that 401 is what [`CAMEL_AUTHENTICATION_REJECTED`] turns into a prompt.
//!   The EDS side refuses in advance instead, and both are right for the
//!   machinery they answer to.
//! - **The error domain.** Camel does not read `E_CLIENT_ERROR`; what it
//!   branches on is `CAMEL_SERVICE_ERROR`, and in particular
//!   [`CAMEL_SERVICE_ERROR_UNAVAILABLE`], which is the mail-side equivalent of
//!   `E_CLIENT_ERROR_REPOSITORY_OFFLINE` — the difference between a store that
//!   falls back on its summary cache and one that reports the account as
//!   broken. `G_IO_ERROR_CANCELLED` is the exception, because it belongs to
//!   GLib and not to either stack: every caller in Camel tests for it before
//!   deciding anything went wrong at all.
//!
//! [`connect`]: jmap_backend_core::connect
//!
//! What the two sides must *not* answer differently is which failure means the
//! password was wrong, so that question is
//! [`jmap_backend_core::connect::is_wrong_password`] and is asked, not
//! reimplemented. A store that treated a 403 as a bad password would put an
//! account into a prompt loop no password can end.
//!
//! The password itself is not read here, exactly as it is not read in
//! [`crate::server`]: Camel fetches it through the `CamelSession` at connect
//! time and hands it to the service, and a JMAP account must never take a
//! credential from a settings object Evolution serialises into a config file.

use std::fmt;

use eds_sys::{
    CAMEL_AUTHENTICATION_ACCEPTED, CAMEL_AUTHENTICATION_ERROR, CAMEL_AUTHENTICATION_REJECTED,
    CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE, CAMEL_SERVICE_ERROR_INVALID,
    CAMEL_SERVICE_ERROR_NOT_CONNECTED, CAMEL_SERVICE_ERROR_UNAVAILABLE,
    CAMEL_SERVICE_ERROR_URL_INVALID, CamelAuthenticationResult, CamelServiceError,
    camel_service_error_quark,
};
use glib_sys::{GError, g_error_new_literal};
use jmap_backend_core::connect::is_wrong_password;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::source::SourceError;
use jmap_client::transport::CancelFlag;
use jmap_client::{Client, Credentials, Error};
use jmap_mail_sync::{MailSync, SyncError};
use jmap_proto::session::CAPABILITY_MAIL;

use crate::server::ServerConfig;

/// What a store reports when the connection came up.
pub const ACCEPTED_AUTHENTICATION: CamelAuthenticationResult = CAMEL_AUTHENTICATION_ACCEPTED;

/// A JMAP mail account that could not be opened.
#[derive(Debug)]
pub enum StoreError {
    /// The settings do not describe a server that may be contacted at all.
    ///
    /// Its own variant rather than a client error, because it is fixed by
    /// editing the account and never by retrying: reported as
    /// `CAMEL_SERVICE_ERROR_UNAVAILABLE` it would be a store Evolution keeps
    /// hopefully reconnecting to forever.
    Config(SourceError),
    /// The server refused, failed, or is unreachable.
    Client(Error),
    /// The store was asked to do something that needs a server, and it has no
    /// connection.
    ///
    /// Not a failure of the account: Camel drives a store it *believes* is
    /// connected, and the belief goes stale — a `disconnect_sync` on another
    /// thread, a reconnect that has not happened yet. Reported as
    /// `CAMEL_SERVICE_ERROR_NOT_CONNECTED`, which is what makes Camel connect
    /// and ask again rather than show the account as broken.
    Disconnected,
}

impl From<SourceError> for StoreError {
    fn from(error: SourceError) -> Self {
        Self::Config(error)
    }
}

impl From<SyncError> for StoreError {
    /// A sync failure is a client failure: `SyncError` exists to keep
    /// [`jmap_client::Error`] intact across the crate boundary, and this is the
    /// end of that journey — the point where it becomes a `CAMEL_SERVICE_ERROR`.
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Client(error) => Self::Client(error),
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
            Self::Client(error) => error.fmt(f),
            Self::Disconnected => f.write_str("not connected to the JMAP server"),
        }
    }
}

impl std::error::Error for StoreError {}

impl StoreError {
    /// What the `authenticate_sync` vfunc answers with.
    ///
    /// Only `REJECTED` makes Camel forget the stored password and ask again, so
    /// it is reserved for the one failure asking again can fix — which is the
    /// same rule the EDS backends follow, asked of the same function.
    pub fn authentication_result(&self) -> CamelAuthenticationResult {
        match self {
            Self::Client(error) if is_wrong_password(error) => CAMEL_AUTHENTICATION_REJECTED,
            _ => CAMEL_AUTHENTICATION_ERROR,
        }
    }

    /// Allocates a `GError` describing this failure. Ownership passes to the
    /// caller, who must `g_error_free` it or hand it to a C caller that will.
    pub fn to_gerror(&self) -> *mut GError {
        let message = cstring_lossy(&self.to_string());

        // The user pressed Stop. Not Camel's domain and not ours: this is the
        // one code every layer above agrees on, and reporting a service error
        // instead would turn a cancelled folder refresh into an alert.
        if matches!(self, Self::Client(Error::Cancelled)) {
            // SAFETY: a live quark and a NUL-terminated message the call
            // copies.
            return unsafe {
                g_error_new_literal(
                    gio_sys::g_io_error_quark(),
                    gio_sys::G_IO_ERROR_CANCELLED,
                    message.as_ptr(),
                )
            };
        }

        // SAFETY: as above; the code is one of the enum's own values.
        unsafe {
            g_error_new_literal(
                camel_service_error_quark(),
                self.service_error_code() as i32,
                message.as_ptr(),
            )
        }
    }

    fn service_error_code(&self) -> CamelServiceError {
        match self {
            Self::Config(_) => CAMEL_SERVICE_ERROR_URL_INVALID,
            Self::Disconnected => CAMEL_SERVICE_ERROR_NOT_CONNECTED,
            // The server could not be reached. This is the code Camel reads to
            // decide the store goes offline rather than the account being
            // wrong, so it is the one that must not be generic.
            Self::Client(Error::Transport(_)) => CAMEL_SERVICE_ERROR_UNAVAILABLE,
            Self::Client(error) if is_wrong_password(error) => {
                CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE
            }
            // A 403, a method error, a malformed response: the server answered,
            // so it is reachable and the credentials were taken. The message
            // carries the detail.
            _ => CAMEL_SERVICE_ERROR_INVALID,
        }
    }
}

/// Connects to the server `config` names and resolves the account its mail
/// lives in.
///
/// `password` is whatever Camel got out of the session, which is `None` for an
/// account nobody has been prompted for yet — and, deliberately, for an account
/// that names no user at all. The two cases produce the same request, because
/// there is nothing to send in either: a user name with an empty password is
/// not a weaker credential, it is a wrong one, and a server that counts failed
/// attempts would count it.
pub fn open_mail(
    config: &ServerConfig,
    password: Option<&str>,
    cancel: CancelFlag,
) -> Result<MailSync, StoreError> {
    let credentials = match (config.user.as_deref(), password) {
        (Some(user), Some(password)) => Credentials::basic(user, password),
        _ => Credentials::none(),
    };

    let client = Client::builder()
        .cancel_flag(cancel)
        .connect(&config.origin, credentials)?;
    // Under `urn:ietf:params:jmap:mail`, the way the address book backend
    // resolves its own account under `:contacts`. An account that offers the
    // one and not the other is not a mail account, and a store that ignored
    // the capability would present it as one with no folders in it.
    let account_id = client.primary_account(CAPABILITY_MAIL)?;

    Ok(MailSync::new(client, account_id))
}
