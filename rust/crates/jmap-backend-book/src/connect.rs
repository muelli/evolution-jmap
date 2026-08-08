// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the connection `connect_sync` needs, with no GObject in sight.
//!
//! `connect_sync` is the one vfunc that has to answer two questions at once:
//! did this work, and — if not — should Evolution ask the user for a password
//! again? The second answer is `out_auth_result`, and getting it wrong is the
//! difference between a prompt that fixes the account and an account that is
//! permanently broken with no way to say so. [`ConnectError::auth_result`] is
//! that decision, kept next to the failures it classifies.

use std::fmt;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_REQUIRED, E_CLIENT_ERROR_INVALID_ARG,
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_ERROR,
    E_SOURCE_AUTHENTICATION_REJECTED, E_SOURCE_AUTHENTICATION_REQUIRED, EClientError,
    ENamedParameters, ESource, ESourceAuthenticationResult, e_client_error_create,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::cancel::CancelBridge;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::marshal::password as stored_password;
use jmap_backend_core::source::SourceConfig;
use jmap_book_sync::BookSync;
use jmap_client::transport::CancelFlag;
use jmap_client::{Client, Credentials, Error};
use jmap_proto::session::CAPABILITY_CONTACTS;

/// What `connect_sync` writes into `out_auth_result` when it succeeds.
pub const ACCEPTED_AUTH_RESULT: ESourceAuthenticationResult = E_SOURCE_AUTHENTICATION_ACCEPTED;

/// A connection that could not be opened.
#[derive(Debug)]
pub enum ConnectError {
    /// The account names a user but EDS has no password for it yet.
    CredentialsRequired,
    /// The server refused, failed, or is unreachable.
    Client(Error),
    /// The account names an address book the server does not have.
    NoSuchAddressBook(String),
    /// The account names no address book and the server flags none default.
    NoDefaultAddressBook,
}

impl From<Error> for ConnectError {
    fn from(error: Error) -> Self {
        Self::Client(error)
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CredentialsRequired => f.write_str("the account has no password yet"),
            Self::Client(error) => error.fmt(f),
            Self::NoSuchAddressBook(id) => write!(
                f,
                "the account names address book \"{id}\", which the server does not have"
            ),
            Self::NoDefaultAddressBook => {
                f.write_str("the server offers no default address book for this account")
            }
        }
    }
}

impl std::error::Error for ConnectError {}

impl ConnectError {
    /// What `connect_sync` writes into `out_auth_result`.
    ///
    /// Only `REJECTED` makes Evolution discard the stored password and ask
    /// again, so it is reserved for the one case where asking again can help:
    /// the server said the credentials were wrong. A 403 is *not* that case —
    /// the credentials were accepted and the account is not allowed — and a
    /// server that is down is not either.
    pub fn auth_result(&self) -> ESourceAuthenticationResult {
        match self {
            Self::CredentialsRequired => E_SOURCE_AUTHENTICATION_REQUIRED,
            Self::Client(Error::Http { status: 401, .. }) => E_SOURCE_AUTHENTICATION_REJECTED,
            _ => E_SOURCE_AUTHENTICATION_ERROR,
        }
    }

    /// Allocates a `GError` describing this failure. Ownership passes to the
    /// caller, as with [`jmap_backend_core::error::to_gerror`].
    pub fn to_gerror(&self) -> *mut GError {
        if let Self::Client(error) = self {
            return jmap_backend_core::error::to_gerror(error);
        }
        let message = cstring_lossy(&self.to_string());
        // SAFETY: the code is one of the enum's own values and the message is
        // copied by the call.
        unsafe { e_client_error_create(self.client_error_code(), message.as_ptr()) }
    }

    fn client_error_code(&self) -> EClientError {
        match self {
            Self::CredentialsRequired => E_CLIENT_ERROR_AUTHENTICATION_REQUIRED,
            // Both address book failures are fixed by editing the account (or
            // by creating a book on the server), never by retrying, so they
            // are reported the same way a malformed host is.
            _ => E_CLIENT_ERROR_INVALID_ARG,
        }
    }
}

/// Connects to the server `config` names and resolves which JMAP address book
/// the source stands for.
///
/// `password` is what EDS got out of libsecret, which is `None` on the first
/// attempt: a source that names a user then fails with
/// [`ConnectError::CredentialsRequired`] rather than trying the server
/// anonymously, so the prompt happens before anything is sent. A source that
/// names no user is anonymous on purpose — that is `jmap-mockd` and a
/// development Stalwart, and a real server answers it with the 401 that turns
/// into a prompt anyway.
pub fn open_book(
    config: &SourceConfig,
    password: Option<&str>,
    cancel: CancelFlag,
) -> Result<BookSync, ConnectError> {
    let credentials = match (config.user.as_deref(), password) {
        (Some(user), Some(password)) => Credentials::basic(user, password),
        (Some(_), None) => return Err(ConnectError::CredentialsRequired),
        (None, _) => Credentials::none(),
    };

    let client = Client::builder()
        .cancel_flag(cancel)
        .connect(&config.origin, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CONTACTS)?;
    let books = client.address_books(&account_id)?;

    let address_book_id = match config.address_book_id.as_deref() {
        // Checked against the server rather than trusted: a typo in a
        // hand-written `.source` would otherwise present as an address book
        // that is merely empty, which is indistinguishable from a server that
        // lost the contacts.
        Some(wanted) => books
            .iter()
            .filter_map(|book| book.id.as_ref())
            .find(|id| id.as_ref() == wanted)
            .cloned()
            .ok_or_else(|| ConnectError::NoSuchAddressBook(wanted.to_owned()))?,
        // Never "the first one": which book a source means is not something
        // to guess at, because the guess is where contacts get written.
        None => books
            .iter()
            .filter(|book| book.is_default == Some(true))
            .find_map(|book| book.id.clone())
            .ok_or(ConnectError::NoDefaultAddressBook)?,
    };

    Ok(BookSync::new(client, account_id, address_book_id))
}

/// The whole of `connect_sync` except the instance: from the `ESource` EDS
/// hands the backend to a connection, with `out_auth_result` and `error`
/// written the way the vfunc has to write them.
///
/// This is the layer the subclass calls, and it is here rather than in
/// [`crate::ops`] because it is the only vfunc body whose input is an
/// `ESource` — which a test can build with `e_source_new_with_uid`, where an
/// `EBookMetaBackend` would need a registry.
///
/// `out_auth_result` is written on **every** path, success included: EDS reads
/// it whenever the vfunc returns, and a stale value from a previous attempt is
/// how an account ends up either never prompting or prompting forever.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource`, `credentials` NULL or a valid
/// `ENamedParameters`, `cancellable` NULL or a valid `GCancellable`, and the
/// two out-parameters NULL or writable — which is what an EDS vfunc receives.
pub unsafe fn connect(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    out_auth_result: *mut ESourceAuthenticationResult,
    error: *mut *mut GError,
) -> Option<BookSync> {
    // A backend without a source cannot be configured, so no prompt helps. It
    // should not happen — EDS constructs the backend *from* a source — but a
    // NULL dereference in `evolution-addressbook-factory` takes every other
    // account down with it.
    if source.is_null() {
        // SAFETY: the out-parameters satisfy the contract by this function's.
        unsafe {
            write_auth_result(out_auth_result, E_SOURCE_AUTHENTICATION_ERROR);
            set_raw_gerror(error, no_source_gerror());
        }
        return None;
    }

    // SAFETY: `source` is a valid ESource, checked non-NULL above.
    let config = match unsafe { SourceConfig::from_source(source) } {
        Ok(config) => config,
        Err(failure) => {
            // A misconfigured account: re-prompting for a password cannot fix
            // a missing host or a plaintext origin, so this is never REJECTED.
            // SAFETY: as above.
            unsafe {
                write_auth_result(out_auth_result, E_SOURCE_AUTHENTICATION_ERROR);
                set_raw_gerror(error, failure.to_gerror());
            }
            return None;
        }
    };

    // SAFETY: `credentials` is NULL or a valid ENamedParameters, which
    // outlives the call.
    let password = unsafe { stored_password(credentials) };
    // SAFETY: `cancellable` is NULL or a valid GCancellable that EDS keeps
    // alive for the duration of the vfunc, which outlives the bridge.
    let bridge = unsafe { CancelBridge::new(cancellable) };

    match open_book(&config, password.as_deref(), bridge.flag().clone()) {
        Ok(sync) => {
            // SAFETY: as above.
            unsafe { write_auth_result(out_auth_result, ACCEPTED_AUTH_RESULT) };
            Some(sync)
        }
        Err(failure) => {
            // SAFETY: as above.
            unsafe {
                write_auth_result(out_auth_result, failure.auth_result());
                set_raw_gerror(error, failure.to_gerror());
            }
            None
        }
    }
}

fn no_source_gerror() -> *mut GError {
    let message = cstring_lossy("the address book backend has no account to connect to");
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call.
    unsafe { e_client_error_create(E_CLIENT_ERROR_INVALID_ARG, message.as_ptr()) }
}

/// Writes an `out_auth_result` the caller may not have asked for.
///
/// # Safety
///
/// `dest` must be NULL or point at a writable `ESourceAuthenticationResult`.
pub unsafe fn write_auth_result(
    dest: *mut ESourceAuthenticationResult,
    value: ESourceAuthenticationResult,
) {
    if !dest.is_null() {
        // SAFETY: `dest` is writable by the contract above.
        unsafe { *dest = value };
    }
}
