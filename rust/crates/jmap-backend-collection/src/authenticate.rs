// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection between the account and the fan-out: the source of the
//! credentials of a collection backend, and the result that it gives to EDS.
//!
//! The fan-out does not occur in `populate`. The `populate` of
//! `ECollectionBackendClass` returns `void`, gets no arguments, and cannot show
//! a password prompt. Instead, EDS gives a backend this vfunc of its
//! grandparent class:
//!
//! ```c
//! ESourceAuthenticationResult (*authenticate_sync) (EBackend *backend,
//!                                                   const ENamedParameters *credentials,
//!                                                   gchar **out_certificate_pem,
//!                                                   GTlsCertificateFlags *out_certificate_errors,
//!                                                   GCancellable *cancellable,
//!                                                   GError **error);
//! ```
//!
//! A `populate` that needs the server calls
//! `e_backend_schedule_credentials_required()`. Then `evolution-source-registry`
//! gets the password of the account (libsecret, OAuth2, or a prompt through
//! Evolution). Then it calls `authenticate_sync` with an `ENamedParameters`.
//! The address book and calendar backends get the same type at `connect_sync`.
//! Thus the credentials come from EDS, one time, and not from this crate or a
//! configuration file. The fan-out occurs in the call that receives them.
//! `e_webdav_collection_backend_discover_sync()` of EDS has the same signature
//! for the same reason.
//!
//! [`authenticate_with`] is that vfunc without the instance. It reads the
//! account `ESource`, makes the [`Login`] that a fan-out needs, and sets the
//! result enum on each path. The fan-out is a closure because only it needs a
//! live `ECollectionBackend`: `e_collection_backend_new_child()` and
//! `e_collection_backend_list_*_sources()` are instance methods.
//!
//! ## The result controls what Evolution does next
//!
//! `ESourceAuthenticationResult` is not a status code. It tells Evolution what
//! to do next, and three of its values are dangerous:
//!
//! - `REJECTED` makes EDS discard the stored password and ask again. For a
//!   server that is down, or for a 403, a new password does not help, and EDS
//!   asks again on each attempt.
//! - `REQUIRED` causes the prompt. Any other result for an account without a
//!   password means that the user can never complete the account.
//! - `ERROR` stops this attempt. For an account where the user disabled all
//!   parts, it shows an error dialog that the user did not cause.
//!
//! [`ConnectError::auth_result`] in `jmap-backend-core` contains the rule that
//! only a 401 gives `REJECTED`. This module calls that rule and does not copy
//! it, because `connect_sync` of the book and calendar backends uses it too.
//!
//! ## The order of the reads
//!
//! [`parts_of`] comes first, then [`server_of`]. An account with all parts
//! disabled has nothing to discover and needs no server. If the host is read
//! first, an incomplete account becomes an error when the user disables its
//! last part. [`crate::collection_source`] documents this order, and this
//! module enforces it.
//!
//! ## The two out-parameters that this module does not set
//!
//! A backend uses `out_certificate_pem` and `out_certificate_errors` to give
//! EDS a server certificate that it could not verify. Evolution can then offer
//! to trust it. This backend does not set them: `ureq` and the system trust
//! store do TLS. This code cannot see the certificate, so it must not ask a
//! user to accept it. Thus a JMAP account with a self-signed certificate fails
//! with an error, not with a "trust this certificate?" dialog.

use std::fmt;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ERROR, ENamedParameters, ESource, ESourceAuthenticationResult,
    e_source_get_uid,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::connect::{
    ACCEPTED_AUTH_RESULT, AuthScheme, ConnectError, resolve_credentials,
};
use jmap_backend_core::error::{invalid_arg_gerror, set_raw_gerror};
use jmap_backend_core::marshal::{password as stored_password, read_string};
use jmap_backend_core::oauth2::source_uses_oauth2;
use jmap_backend_core::source::SourceError;
use jmap_client::{Credentials, Error};
use jmap_collection_sync::Parts;

use crate::collection_source::{Server, parts_of, server_of};

/// All that a fan-out needs, from one read of the account and one set of
/// credentials from EDS.
///
/// The three fields answer the three questions of [`Fanout::discover`]: which
/// server, which user, and which parts. They are one struct because they must
/// come from one read. The discovery origin is also the host in each child that
/// the backend creates, and two reads of one `ESource` can disagree.
///
/// [`Fanout::discover`]: jmap_collection_sync::Fanout::discover
#[derive(Debug, Clone)]
pub struct Login {
    /// The location of the server, in the two forms that the backend needs.
    /// See [`Server`].
    pub server: Server,
    /// The parts (mail, contacts, calendars) that the account enables. Never
    /// all disabled: an account with no enabled part does not reach a fan-out.
    pub parts: Parts,
    /// The credentials to authenticate with. [`Credentials::None`] is
    /// intentional: an account that names no user is anonymous.
    pub credentials: Credentials,
}

/// `EBackendClass::authenticate_sync` for a JMAP collection, without the
/// instance. It reads the account, makes the credentials, and runs `fan_out`
/// with the result. If an earlier step fails, it returns that result and
/// `fan_out` does not run.
///
/// `fan_out` needs the `ECollectionBackend`, so the caller supplies it. It does
/// `Fanout::discover` against [`Login::server`], one
/// `e_collection_backend_new_child` plus [`child_source::apply`] for each child,
/// and [`removal::remove_obsolete`] over the existing children. Its `Ok` gives
/// `ACCEPTED`. Its error gets the same classification as an error of
/// `connect_sync`.
///
/// `push_credentials` does the `e_collection_backend_authenticate_children()`
/// call of EWS. It is a closure because it also needs the live
/// `ECollectionBackend`. It runs one time, and only after `fan_out` returns
/// `Ok`. An account with no enabled part does not reach it. An account where
/// `fan_out` failed does not reach it either, because there is no new login to
/// give to the children. With it, the children get the credentials that this
/// call resolved, as in evolution-ews. Without it, each address book and
/// calendar child gets its credentials again when it next needs them (the
/// three-branch resolution of `connect_with`).
///
/// This function sets an error on each path that is not `ACCEPTED`, and on no
/// accepting path. This is the GLib convention for this enum, because
/// `ACCEPTED` is its only success. EDS reads the out-parameter for all results,
/// so a stale `GError` makes a correct account look broken.
///
/// [`child_source::apply`]: crate::child_source::apply
/// [`removal::remove_obsolete`]: crate::removal::remove_obsolete
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` (the account from which EDS
/// constructed the backend). `credentials` must be NULL or a valid
/// `ENamedParameters`. `cancellable` must be NULL or a valid `GCancellable`.
/// `error` must be NULL or a writable pointer to a NULL `GError`. An EDS vfunc
/// receives these values.
pub unsafe fn authenticate_with<F, P>(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    fan_out: F,
    push_credentials: P,
) -> ESourceAuthenticationResult
where
    F: FnOnce(Login) -> Result<(), jmap_client::Error>,
    P: FnOnce(*const ENamedParameters),
{
    // A backend without an account cannot be configured, so a prompt does not
    // help. EDS constructs the backend from a source, so this is not expected.
    // But a NULL dereference here stops `evolution-source-registry` and all
    // accounts in the session.
    if source.is_null() {
        // SAFETY: the contract of this function makes `error` valid.
        unsafe { set_raw_gerror(error, no_account_gerror()) };
        return E_SOURCE_AUTHENTICATION_ERROR;
    }

    let account_id = unsafe { read_string(e_source_get_uid(source)) };
    let account_id_str = account_id.as_deref();

    // Read the parts before the server. See the module documentation.
    // SAFETY: `source` is valid, and the check above makes sure it is not NULL.
    let parts = unsafe { parts_of(source) };
    if !parts.any() {
        tracing::debug!(
            account_id = account_id_str,
            "no collection parts enabled; accepting authentication without network request"
        );
        return ACCEPTED_AUTH_RESULT;
    }

    // SAFETY: `credentials` is NULL (EDS passes NULL before it asks libsecret)
    // or a valid `ENamedParameters` that is alive longer than this call.
    let password = unsafe { stored_password(credentials) };
    // Keep this value: a failure of `fan_out` also needs it. It separates a 401
    // on a new OAuth 2.0 token from a 401 on a password. See `finish_fan_out`.
    // SAFETY: `source` is valid, and the check above makes sure it is not NULL.
    let uses_oauth2 = unsafe { source_uses_oauth2(source) };
    tracing::debug!(
        account_id = account_id_str,
        uses_oauth2,
        "authenticating collection backend"
    );
    // SAFETY: `source` is valid and not NULL (see above). The contract of this
    // function makes `cancellable` satisfy the contract of `login_of`.
    let login = match unsafe { login_of(source, parts, password.as_deref(), cancellable) } {
        Ok(login) => login,
        Err(failure) => {
            tracing::debug!(
                account_id = account_id_str,
                uses_oauth2,
                ?failure,
                "collection backend credential resolution failed"
            );
            // SAFETY: the contract of this function makes `error` valid.
            unsafe { set_raw_gerror(error, failure.to_gerror()) };
            return failure.auth_result();
        }
    };

    // Keep this guard only for the duration of the fan-out. A flag that stays
    // after the call belongs to the account and not to this operation. Then,
    // after the user stops one authentication, all later requests on this thread
    // fail. See `jmap_backend_core::cancel::observe`.
    // SAFETY: `cancellable` is NULL or a valid `GCancellable`. EDS keeps it
    // alive for the duration of the vfunc, which is longer than this scope.
    let _cancel = unsafe { observe(cancellable) };

    match finish_fan_out(uses_oauth2, fan_out(login)) {
        Ok(()) => {
            tracing::debug!(
                account_id = account_id_str,
                uses_oauth2,
                "collection backend authentication accepted"
            );
            push_credentials(credentials);
            ACCEPTED_AUTH_RESULT
        }
        Err(failure) => {
            tracing::debug!(
                account_id = account_id_str,
                uses_oauth2,
                ?failure,
                "collection backend authentication failed"
            );
            // SAFETY: as above.
            unsafe { set_raw_gerror(error, failure.to_gerror()) };
            failure.auth_result()
        }
    }
}

/// The result of `authenticate_with`, from the result of `fan_out`.
///
/// This is a separate function so that the reclassification needs no live
/// `ESource`. [`jmap_backend_core::connect`] keeps `finish_connect` separate
/// from `connect_with` for the same reason. This function does the same for
/// the collection backend, over the `jmap_client::Error` of `fan_out`. See
/// [`ConnectError::reclassify_oauth2_rejection`] for why a 401 on a new OAuth
/// 2.0 bearer token does not mean a wrong password, as for Basic or an API
/// token.
fn finish_fan_out(uses_oauth2: bool, outcome: Result<(), Error>) -> Result<(), ConnectError> {
    outcome.map_err(ConnectError::from).map_err(|failure| {
        if uses_oauth2 {
            failure.reclassify_oauth2_rejection()
        } else {
            failure
        }
    })
}

/// One read of the account and one set of credentials, as the [`Login`] that
/// each operation against the server of this account needs.
///
/// This is separate from [`authenticate_with`] because `create_resource_sync`
/// also needs the same server, account and credentials. One rule selects the
/// credentials: the `[Authentication] Method` of the account selects OAuth 2.0
/// or an API token, otherwise the stored password. Two copies of the rule can
/// disagree without an error. For example, the fan-out uses OAuth 2.0 and the
/// account then creates address books anonymously.
///
/// The caller gives `parts`, so that [`authenticate_with`] keeps its documented
/// order: it answers an account with all parts disabled before it reads the
/// host. Also, a caller that does not use the parts as a gate still puts the
/// real parts of the account in the `Login`. (A create is about a collection on
/// the server, whatever parts the account enables.)
///
/// `password` is the value that EDS got from libsecret, in the form that
/// [`jmap_backend_core::marshal::password`] and
/// [`crate::create_resource::stored_password_of`] give. `None` means that
/// there is no password yet. For an account that names a user, this gives
/// [`ConnectError::CredentialsRequired`] and thus a prompt.
///
/// # Safety
///
/// `source` must be a valid `ESource` (the account from which EDS constructed
/// the backend). `cancellable` must be NULL or a valid `GCancellable`. An EDS
/// vfunc receives these values.
pub unsafe fn login_of(
    source: *mut ESource,
    parts: Parts,
    password: Option<&str>,
    cancellable: *mut GCancellable,
) -> Result<Login, LoginError> {
    // SAFETY: the contract of this function makes `source` valid.
    let server = unsafe { server_of(source) }?;

    // SAFETY: as above.
    let scheme = unsafe { AuthScheme::of(source) };
    let uses_oauth2 = scheme == AuthScheme::OAuth2;
    let uses_api_token = scheme == AuthScheme::ApiToken;
    let has_password = password.is_some();
    tracing::debug!(
        uses_oauth2,
        uses_api_token,
        has_password,
        "resolving collection backend login credentials"
    );

    // `resolve_credentials` is the same choice that `connect_with` makes for the
    // address book and calendar backends. See the module documentation of
    // `jmap_backend_core::oauth2` for the source of this rule. An OAuth 2.0
    // account can name no `[Authentication] User`. If a missing user meant
    // "anonymous" here, as for a password account, the backend would skip
    // OAuth 2.0 and connect with no credentials.
    // SAFETY: the contract of this function makes `source` valid, and makes
    // `cancellable` satisfy the contract of `access_token`.
    let credentials = unsafe {
        resolve_credentials(
            source,
            scheme,
            server.connection.user.as_deref(),
            || password.map(str::to_owned),
            cancellable,
        )
    }?;

    Ok(Login {
        server,
        parts,
        credentials,
    })
}

/// The reason why an account did not become a [`Login`].
///
/// There are two variants because Evolution must treat the two failures
/// differently. A broken account is never a password problem, so it must never
/// cause a prompt. A missing credential is only a password problem.
#[derive(Debug)]
pub enum LoginError {
    /// The account does not name a server that this backend can contact. See
    /// [`SourceError`].
    Account(SourceError),
    /// The account names a server, but there is no credential for it.
    Credentials(ConnectError),
}

impl From<SourceError> for LoginError {
    fn from(failure: SourceError) -> Self {
        Self::Account(failure)
    }
}

impl From<ConnectError> for LoginError {
    fn from(failure: ConnectError) -> Self {
        Self::Credentials(failure)
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(failure) => failure.fmt(f),
            Self::Credentials(failure) => failure.fmt(f),
        }
    }
}

impl std::error::Error for LoginError {}

impl LoginError {
    /// The `GError` that an operation reports for this error.
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Account(failure) => failure.to_gerror(),
            Self::Credentials(failure) => failure.to_gerror(),
        }
    }

    /// The return value of `authenticate_sync` for this error.
    ///
    /// [`LoginError::Account`] is always `ERROR`. A password does not fix a
    /// missing host, an invalid host, or plain HTTP to a remote server. Thus
    /// none of them is `REQUIRED` or `REJECTED`. The credentials variant uses
    /// the rule of [`ConnectError::auth_result`], which causes the prompt.
    pub fn auth_result(&self) -> ESourceAuthenticationResult {
        match self {
            Self::Account(_) => E_SOURCE_AUTHENTICATION_ERROR,
            Self::Credentials(failure) => failure.auth_result(),
        }
    }
}

/// The `GError` for a backend that got no account.
fn no_account_gerror() -> *mut GError {
    invalid_arg_gerror("the JMAP collection backend has no account to authenticate")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate of `authenticate_with` on the result of `fan_out`. Only a 401
    /// on an OAuth 2.0 login gets reclassification, and success does not
    /// change. Compare `jmap_mail::service::finish_authenticate_tests` and the
    /// `finish_connect` test of `jmap_backend_core::connect`.
    #[test]
    fn finish_fan_out_reclassifies_only_when_the_login_was_oauth2() {
        let unauthorized = || {
            Err(Error::Http {
                status: 401,
                problem: None,
            })
        };

        assert!(matches!(
            finish_fan_out(true, unauthorized()),
            Err(ConnectError::OAuth2(_))
        ));
        assert!(matches!(
            finish_fan_out(false, unauthorized()),
            Err(ConnectError::Client(_))
        ));
        assert!(finish_fan_out(true, Ok(())).is_ok());
        assert!(finish_fan_out(false, Ok(())).is_ok());
    }

    /// The reclassification is narrow: a failure other than 401 on an OAuth
    /// 2.0 login stays `Client`. Compare
    /// `only_a_401_is_reclassified_to_oauth2_required` in
    /// `jmap_backend_core::connect`.
    #[test]
    fn only_the_401_shape_is_reclassified() {
        for failure in [
            Error::Http {
                status: 403,
                problem: None,
            },
            Error::Transport("down".to_owned()),
        ] {
            assert!(matches!(
                finish_fan_out(true, Err(failure)),
                Err(ConnectError::Client(_))
            ));
        }
    }

    /// The `Display` of `LoginError` writes the inner error for both variants.
    /// Without this test, a `Display` that writes nothing compiles, and a
    /// broken account shows an empty error message.
    #[test]
    fn a_login_error_displays_the_inner_error() {
        let account = LoginError::Account(SourceError::MissingHost);
        assert_eq!(account.to_string(), SourceError::MissingHost.to_string());

        let credentials = LoginError::Credentials(ConnectError::CredentialsRequired);
        assert_eq!(
            credentials.to_string(),
            ConnectError::CredentialsRequired.to_string()
        );
    }
}
