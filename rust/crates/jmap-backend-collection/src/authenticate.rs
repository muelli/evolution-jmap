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

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ERROR, ENamedParameters, ESource, ESourceAuthenticationResult,
};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, ConnectError, credentials as login_as};
use jmap_backend_core::error::{invalid_arg_gerror, set_raw_gerror};
use jmap_backend_core::marshal::password as stored_password;
use jmap_backend_core::oauth2::{access_token, source_uses_oauth2};
use jmap_client::Credentials;
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
pub unsafe fn authenticate_with<F>(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    fan_out: F,
) -> ESourceAuthenticationResult
where
    F: FnOnce(Login) -> Result<(), jmap_client::Error>,
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

    // Parts before server: see the module comment on the order.
    // SAFETY: a valid source, checked non-NULL above.
    let parts = unsafe { parts_of(source) };
    if !parts.any() {
        return ACCEPTED_AUTH_RESULT;
    }

    // SAFETY: as above.
    let server = match unsafe { server_of(source) } {
        Ok(server) => server,
        Err(failure) => {
            // A missing host, a host that is not a host, or plain HTTP to a
            // remote server: none of them is fixed by a password, so none of
            // them is ever REQUIRED or REJECTED.
            // SAFETY: as above.
            unsafe { set_raw_gerror(error, failure.to_gerror()) };
            return E_SOURCE_AUTHENTICATION_ERROR;
        }
    };

    // SAFETY: `credentials` is NULL — which is what EDS passes before it has
    // asked libsecret for anything — or a valid `ENamedParameters` that
    // outlives the call.
    let password = unsafe { stored_password(credentials) };
    // Which authentication scheme this account uses is decided the same way
    // `connect_with` decides it for the address book and calendar backends —
    // see `jmap_backend_core::oauth2`'s module docs for whose rule this is.
    // An OAuth 2.0 account can name no `[Authentication] User` at all, so
    // reading a missing user as "anonymous" here, the way a plain password
    // account's absent user means, would silently skip OAuth 2.0 and fan the
    // account out with no credentials whatsoever.
    // SAFETY: `source` is a valid ESource, checked non-NULL above; `cancellable`
    // satisfies `access_token`'s contract by this function's own.
    let resolved = unsafe {
        if source_uses_oauth2(source) {
            access_token(source, cancellable).map(Credentials::bearer)
        } else {
            login_as(server.connection.user.as_deref(), password.as_deref())
        }
    };
    let credentials = match resolved {
        Ok(credentials) => credentials,
        Err(failure) => {
            // The prompt, and the only path that produces one: the account
            // names a user and EDS has no password for it yet, or the account
            // is OAuth 2.0 and no access token could be had.
            // SAFETY: as above.
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

    match fan_out(Login {
        server,
        parts,
        credentials,
    }) {
        Ok(()) => ACCEPTED_AUTH_RESULT,
        Err(failure) => {
            let failure = ConnectError::from(failure);
            // SAFETY: as above.
            unsafe { set_raw_gerror(error, failure.to_gerror()) };
            failure.auth_result()
        }
    }
}

/// The `GError` for a backend that was handed no account at all.
fn no_account_gerror() -> *mut GError {
    invalid_arg_gerror("the JMAP collection backend has no account to authenticate")
}
