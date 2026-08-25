// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The piece between the account and the fan-out: where a collection backend
//! gets its credentials, and what it answers EDS with.
//!
//! This is the question that blocked `populate`, and the answer is that
//! `populate` is not where the fan-out happens. `ECollectionBackendClass`'s
//! `populate` returns `void`, is handed nothing, and has nowhere to put a
//! password prompt. What EDS gives a backend instead is its grandparent's vfunc:
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
//! and the loop around it is: a `populate` that needs the server calls
//! `e_backend_schedule_credentials_required()`, `evolution-source-registry`
//! resolves the account's password (libsecret, or OAuth2, or a prompt through
//! Evolution), and then calls back into `authenticate_sync` with an
//! `ENamedParameters` — the same shape the address book and calendar backends
//! are handed at `connect_sync`. So the credentials never come from this
//! crate's own code, and never from a config file; they come from EDS, once, and
//! the fan-out happens inside the call that receives them. EDS's own
//! `e_webdav_collection_backend_discover_sync()` has exactly this signature and
//! exists for exactly this reason.
//!
//! [`authenticate_with`] is that vfunc minus the instance: everything from the
//! account `ESource` down to the [`Login`] a fan-out needs, with the one enum
//! EDS reads written on every path. The fan-out itself is a closure because it
//! is the only part that needs a live `ECollectionBackend` —
//! `e_collection_backend_new_child()` and `e_collection_backend_list_*_sources()`
//! are instance methods — and the decisions here are not about children at all.
//!
//! ## The result is the whole user experience
//!
//! `ESourceAuthenticationResult` is not a status code, it is what Evolution
//! *does* next, and three of its values are traps:
//!
//! - `REJECTED` makes EDS discard the stored password and ask again. Answering
//!   it for a server that is down, or for a 403, asks someone to fix something a
//!   password cannot fix — and asks again every time.
//! - `REQUIRED` is what turns into the prompt. Answering anything else for an
//!   account that has no password yet is an account that can never be completed.
//! - `ERROR` is the end of the road for this attempt. Answering it for an
//!   account whose parts the user simply switched off puts a dialog in front of
//!   someone who asked for nothing.
//!
//! The 401-and-only-401 rule is [`ConnectError::auth_result`]'s, in
//! `jmap-backend-core`, and it is reached from here rather than restated:
//! `connect_sync` on the book and calendar backends answers the same question
//! with the same enum, and a rule like that written twice is a rule corrected
//! once.
//!
//! ## What is read, and in which order
//!
//! [`parts_of`] first, and only then [`server_of`]. An account with every part
//! switched off has nothing to discover and needs no server, so asking for the
//! host first would report a half-written account as broken the moment its owner
//! unticked the last part. That ordering is [`crate::collection_source`]'s
//! documented one; this is where it is enforced.
//!
//! ## The two out-parameters this does not touch
//!
//! `out_certificate_pem` and `out_certificate_errors` are how a backend hands
//! EDS a server certificate it could not verify, so that Evolution can offer to
//! trust it. This backend does not fill them in: TLS is `ureq`'s and the system
//! trust store's, and a certificate this code cannot see is one it must not
//! invite anyone to accept. The consequence is honest and small — a JMAP account
//! with a self-signed certificate fails with an error instead of a "trust this
//! certificate?" dialog.

use std::fmt;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ERROR, ENamedParameters, ESource, ESourceAuthenticationResult,
    e_source_get_uid,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::api_token::source_uses_api_token;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::connect::{
    ACCEPTED_AUTH_RESULT, ConnectError, bearer_credentials, credentials as login_as,
};
use jmap_backend_core::error::{invalid_arg_gerror, set_raw_gerror};
use jmap_backend_core::marshal::{password as stored_password, read_string};
use jmap_backend_core::oauth2::{access_token, source_uses_oauth2};
use jmap_backend_core::source::SourceError;
use jmap_client::{Credentials, Error};
use jmap_collection_sync::Parts;

use crate::collection_source::{Server, parts_of, server_of};

/// Everything a fan-out needs, out of one read of the account and one set of
/// credentials from EDS.
///
/// The three fields answer the three questions [`Fanout::discover`] asks —
/// which server, as whom, and for which parts — and they are one struct because
/// they have to be one *read*: the origin this backend discovers from is also
/// the host written into every child it creates, and two reads of one `ESource`
/// are two chances to disagree about which server the account is.
///
/// [`Fanout::discover`]: jmap_collection_sync::Fanout::discover
#[derive(Debug, Clone)]
pub struct Login {
    /// Where the server is, in both the shapes it is needed in — see
    /// [`Server`].
    pub server: Server,
    /// Which of mail, contacts and calendars the account still asks for.
    /// Never all-off: an account with nothing switched on never reaches a
    /// fan-out.
    pub parts: Parts,
    /// What to authenticate as. [`Credentials::None`] is a deliberate answer,
    /// not a missing one — an account that names no user is anonymous on
    /// purpose.
    pub credentials: Credentials,
}

/// `EBackendClass::authenticate_sync` for a JMAP collection, minus the
/// instance: the account is read, the credentials are assembled, and `fan_out`
/// is run against the result — or one of the answers before it is returned
/// instead, and `fan_out` never runs at all.
///
/// `fan_out` is what needs the `ECollectionBackend` and so is not here:
/// `Fanout::discover` against [`Login::server`], an
/// `e_collection_backend_new_child` plus [`child_source::apply`] per child, and
/// [`removal::remove_obsolete`] over the children the collection already has.
/// Its `Ok` is `ACCEPTED` and its error is classified the same way
/// `connect_sync`'s is.
///
/// `push_credentials` is EWS's `e_collection_backend_authenticate_children()`
/// call, for the same reason `fan_out` is a closure: it needs the live
/// `ECollectionBackend`, which is not here either. It runs exactly once, only
/// once `fan_out` has returned `Ok` — an account with nothing switched on never
/// reaches it, and neither does one `fan_out` failed for, since there is
/// nothing freshly authenticated to hand children that never got a look at this
/// login at all. Without it, each address-book/calendar child independently
/// re-fetches its own credentials the next time it needs them (`connect_with`'s
/// three-branch resolution, unchanged) rather than being handed what this
/// authenticate just resolved — see `docs/EWS-PARITY.md` Surface 5.
///
/// An error is set on every non-`ACCEPTED` path and on none of the accepting
/// ones, which is GLib's convention read against this enum: `ACCEPTED` is the
/// only success it has. EDS reads the out-parameter whatever the result was, so
/// a stale `GError` is how an account that is fine gets reported as broken.
///
/// [`child_source::apply`]: crate::child_source::apply
/// [`removal::remove_obsolete`]: crate::removal::remove_obsolete
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` — the account EDS constructed the
/// backend from — `credentials` NULL or a valid `ENamedParameters`,
/// `cancellable` NULL or a valid `GCancellable`, and `error` NULL or a writable
/// pointer to a NULL `GError`. That is what an EDS vfunc receives.
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
    // An account-less backend cannot be configured, so no prompt helps. It
    // should not happen — EDS constructs the backend *from* a source — but a
    // NULL dereference here takes `evolution-source-registry` down, and with it
    // every account in the session.
    if source.is_null() {
        // SAFETY: the out-parameter satisfies the contract by this function's.
        unsafe { set_raw_gerror(error, no_account_gerror()) };
        return E_SOURCE_AUTHENTICATION_ERROR;
    }

    let account_id = unsafe { read_string(e_source_get_uid(source)) };
    let account_id_str = account_id.as_deref();

    // Parts before server: see the module comment on the order.
    // SAFETY: a valid source, checked non-NULL above.
    let parts = unsafe { parts_of(source) };
    if !parts.any() {
        tracing::debug!(
            account_id = account_id_str,
            "no collection parts enabled; accepting authentication without network request"
        );
        return ACCEPTED_AUTH_RESULT;
    }

    // SAFETY: `credentials` is NULL — which is what EDS passes before it has
    // asked libsecret for anything — or a valid `ENamedParameters` that
    // outlives the call.
    let password = unsafe { stored_password(credentials) };
    // Read once and kept, not just asked inside `login_of`: `fan_out`'s own
    // failure needs it too, to tell a 401 on the token this login just
    // fetched apart from one on an ordinary password — see `finish_fan_out`.
    // SAFETY: a valid source, checked non-NULL above.
    let uses_oauth2 = unsafe { source_uses_oauth2(source) };
    tracing::debug!(
        account_id = account_id_str,
        uses_oauth2,
        "authenticating collection backend"
    );
    // SAFETY: a valid source, checked non-NULL above, and a cancellable that
    // satisfies `login_of`'s contract by this function's own.
    let login = match unsafe { login_of(source, parts, password.as_deref(), cancellable) } {
        Ok(login) => login,
        Err(failure) => {
            tracing::debug!(
                account_id = account_id_str,
                uses_oauth2,
                ?failure,
                "collection backend credential resolution failed"
            );
            // SAFETY: the out-parameter satisfies the contract by this
            // function's.
            unsafe { set_raw_gerror(error, failure.to_gerror()) };
            return failure.auth_result();
        }
    };

    // Held for the length of the fan-out and no longer. A flag that outlived
    // the call would belong to the *account* rather than to this operation, and
    // an authenticate the user stopped would leave every later request on this
    // thread refusing. See `jmap_backend_core::cancel::observe`.
    // SAFETY: `cancellable` is NULL or a valid `GCancellable` that EDS keeps
    // alive for the duration of the vfunc, which outlives this scope.
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

/// What `authenticate_with` reports, given what `fan_out` itself answered.
///
/// Split out so the reclassification decision needs no live `ESource`, the
/// same way [`jmap_backend_core::connect`]'s own `finish_connect` is split
/// out of `connect_with` for the address book and calendar backends'
/// `connect_sync` — this is the collection backend's sibling of that, over
/// `fan_out`'s `jmap_client::Error` rather than a `Result<T, ConnectError>`
/// an `open` already returns. See
/// [`ConnectError::reclassify_oauth2_rejection`] for why a 401 on a bearer
/// token this login just fetched is not "the password was wrong" the way it
/// is for Basic or an API token.
fn finish_fan_out(uses_oauth2: bool, outcome: Result<(), Error>) -> Result<(), ConnectError> {
    outcome.map_err(ConnectError::from).map_err(|failure| {
        if uses_oauth2 {
            failure.reclassify_oauth2_rejection()
        } else {
            failure
        }
    })
}

/// One read of the account and one set of credentials, as the [`Login`] every
/// operation against this account's server needs.
///
/// Split out of [`authenticate_with`] because `authenticate_sync` is no longer
/// the only vfunc that needs one: `create_resource_sync` has to reach the same
/// server as the same account with the same credentials, and the rule that
/// decides *which* credentials — OAuth 2.0 by the account's
/// `[Authentication] Method`, otherwise the stored password — is one rule.
/// Written twice it would be two, and the way it fails is silent: an account
/// whose OAuth 2.0-ness one path reads and the other does not is one that fans
/// out fine and then creates address books anonymously.
///
/// `parts` is passed in rather than read here so that [`authenticate_with`]
/// keeps its documented order — an account with every part switched off is
/// answered before its host is even looked at — and so that a caller for whom
/// the parts do not gate anything (a create is about a collection that exists on
/// the server whatever the account has switched on) still reports the account's
/// real parts in the `Login`.
///
/// `password` is what EDS resolved out of libsecret, in the form
/// [`jmap_backend_core::marshal::password`] and
/// [`crate::create_resource::stored_password_of`] both produce. `None` is "there
/// is none yet", which for an account that names a user is
/// [`ConnectError::CredentialsRequired`] and so a prompt.
///
/// # Safety
///
/// `source` must be a valid `ESource` — the account EDS constructed the backend
/// from — and `cancellable` NULL or a valid `GCancellable`. That is what an EDS
/// vfunc receives.
pub unsafe fn login_of(
    source: *mut ESource,
    parts: Parts,
    password: Option<&str>,
    cancellable: *mut GCancellable,
) -> Result<Login, LoginError> {
    // SAFETY: a valid source by this function's contract.
    let server = unsafe { server_of(source) }?;

    let uses_oauth2 = unsafe { source_uses_oauth2(source) };
    let uses_api_token = unsafe { source_uses_api_token(source) };
    let has_password = password.is_some();
    tracing::debug!(
        uses_oauth2,
        uses_api_token,
        has_password,
        "resolving collection backend login credentials"
    );

    // Which authentication scheme this account uses is decided the same way
    // `connect_with` decides it for the address book and calendar backends —
    // see `jmap_backend_core::oauth2`'s module docs for whose rule this is.
    // An OAuth 2.0 account can name no `[Authentication] User` at all, so
    // reading a missing user as "anonymous" here, the way a plain password
    // account's absent user means, would silently skip OAuth 2.0 and connect
    // with no credentials whatsoever.
    // SAFETY: a valid source by this function's contract; `cancellable`
    // satisfies `access_token`'s contract by this function's own.
    let credentials = unsafe {
        if uses_oauth2 {
            access_token(source, cancellable).map(Credentials::bearer)
        } else if uses_api_token {
            bearer_credentials(password)
        } else {
            login_as(server.connection.user.as_deref(), password)
        }
    }?;

    Ok(Login {
        server,
        parts,
        credentials,
    })
}

/// Why an account could not be turned into a [`Login`].
///
/// Two variants because the two halves fail differently and Evolution has to
/// treat them differently: a broken *account* is never a password problem, so it
/// must never become a prompt, while a missing credential is nothing but.
#[derive(Debug)]
pub enum LoginError {
    /// The account does not name a server this backend may contact — see
    /// [`SourceError`].
    Account(SourceError),
    /// The account names a server and there is no credential to reach it with.
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
    /// The `GError` an operation reports this through.
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Account(failure) => failure.to_gerror(),
            Self::Credentials(failure) => failure.to_gerror(),
        }
    }

    /// What `authenticate_sync` writes into its return value.
    ///
    /// [`LoginError::Account`] is always `ERROR`: a missing host, a host that is
    /// not a host, or plain HTTP to a remote server: none of them is fixed by a
    /// password, so none of them is ever `REQUIRED` or `REJECTED`. The
    /// credentials half keeps [`ConnectError::auth_result`]'s own rule, which is
    /// the one that produces the prompt.
    pub fn auth_result(&self) -> ESourceAuthenticationResult {
        match self {
            Self::Account(_) => E_SOURCE_AUTHENTICATION_ERROR,
            Self::Credentials(failure) => failure.auth_result(),
        }
    }
}

/// The `GError` for a backend that was handed no account at all.
fn no_account_gerror() -> *mut GError {
    invalid_arg_gerror("the JMAP collection backend has no account to authenticate")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate `authenticate_with` applies around `fan_out`'s outcome: only
    /// an OAuth 2.0 login's 401 is reclassified, and success passes through
    /// unchanged either way — the collection backend's sibling of
    /// `jmap_mail::service::finish_authenticate_tests` and
    /// `jmap_backend_core::connect`'s own `finish_connect` test.
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

    /// The reclassification is narrow: a non-401 failure on an OAuth 2.0
    /// login is left as `Client`, the same as `jmap_backend_core::connect`'s
    /// `only_a_401_is_reclassified_to_oauth2_required`.
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
}
