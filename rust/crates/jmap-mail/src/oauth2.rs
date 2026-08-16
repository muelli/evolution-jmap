// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a mail account authenticates with OAuth 2.0, and the access token
//! if it does.
//!
//! ## The same rule, a different field
//!
//! [`jmap_backend_core::oauth2`] answers this for the address book and
//! calendar backends by reading `[Authentication] Method` off the account's
//! `ESource`. Camel keeps no `ESource` on a service — it keeps the same
//! account's authentication choice on `CamelNetworkSettings:auth-mechanism`
//! instead, because that is the interface every Camel provider's settings
//! implements. Evolution's own account editor writes both fields from one
//! combo box when the account is a JMAP one (`jmap-config`'s `Authentication`
//! combo), so [`method_is_oauth2`] is asked of this field too rather than
//! re-decided — an account the editor calls OAuth 2.0 must be one on both
//! sides of it, or the mail half of it would silently fall back to sending no
//! credentials at all.
//!
//! [`method_is_oauth2`]: jmap_backend_core::oauth2::method_is_oauth2
//!
//! ## Where the token comes from
//!
//! `camel_session_get_oauth2_access_token_sync` is Camel's counterpart of
//! `e_source_get_oauth2_access_token_sync`, and in the running application it
//! is not a second implementation of that rule: `EMailSession`'s override
//! (evolution-data-server 3.52.3, `libemail-engine/e-mail-session.c`,
//! `mail_session_get_oauth2_access_token_sync`) looks the service's `ESource`
//! up in the registry by uid and calls the very same `ESource` function the
//! other two backends call directly. So this is not a parallel path that
//! could disagree with theirs about whose token is good — it is a longer way
//! to the same answer, taken because a `CamelService` has no `ESource` of its
//! own to ask.
//!
//! ## Which `CamelAuthenticationResult` a failure gets
//!
//! Deliberately [`crate::connect::StoreError::OAuth2`]'s own concern, not
//! this module's — see that variant's doc comment for why a failed token
//! fetch is reported as `CAMEL_AUTHENTICATION_ERROR` rather than `REJECTED`,
//! the opposite of the choice `jmap_backend_core::connect::ConnectError`
//! makes for the same failure on the EDS side.

use eds_sys::{
    CamelService, CamelSession, CamelSettings, camel_network_settings_dup_auth_mechanism,
    camel_session_get_oauth2_access_token_sync,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, g_error_free};

use crate::connect::StoreError;
use crate::server::{network, take_string};

/// Whether `settings`' `auth-mechanism` names OAuth 2.0 — see the module docs
/// for whose rule this reuses and why it is asked of this field.
///
/// # Safety
///
/// `settings` must be NULL or a valid `CamelSettings`. It is only read from,
/// and nothing outlives the call.
pub unsafe fn uses_oauth2(settings: *mut CamelSettings) -> bool {
    // SAFETY: the contract above is `network`'s own.
    let Some(network) = (unsafe { network(settings) }) else {
        return false;
    };
    // SAFETY: `network` implements the interface, which is `network`'s own
    // guarantee; the `dup_` accessor returns a g_malloc'd copy this call
    // frees via `take_string` rather than a pointer into storage another
    // thread may replace.
    let method = unsafe { take_string(camel_network_settings_dup_auth_mechanism(network)) };
    jmap_backend_core::oauth2::method_is_oauth2(method.as_deref())
}

/// The OAuth 2.0 access token to send this account's requests as.
///
/// This is where the refresh happens, exactly as it does on
/// [`e_source_get_oauth2_access_token_sync`]: whatever runs underneath looks
/// the account's refresh token up and exchanges it for an access token inside
/// this call, so what comes back is good now, and a failure is either
/// "nobody has consented to this account yet" or "the exchange did not
/// work" — [`StoreError::OAuth2`] either way.
///
/// [`e_source_get_oauth2_access_token_sync`]: eds_sys::e_source_get_oauth2_access_token_sync
///
/// # Safety
///
/// `session` must be a valid `CamelSession`, `service` a valid `CamelService`
/// registered on it, and `cancellable` NULL or a valid `GCancellable` — which
/// is what `attempt` has by the time it calls this.
pub unsafe fn access_token(
    session: *mut CamelSession,
    service: *mut CamelService,
    cancellable: *mut GCancellable,
) -> Result<String, StoreError> {
    let mut token = std::ptr::null_mut();
    let mut expires_in = 0;
    let mut error: *mut GError = std::ptr::null_mut();

    // SAFETY: a valid session and service by this function's contract, a
    // NULL-or-valid cancellable, and three writable out-parameters. The token
    // comes back as a GLib allocation this call owns, and the GError likewise.
    let ok = unsafe {
        camel_session_get_oauth2_access_token_sync(
            session,
            service,
            &mut token,
            &mut expires_in,
            cancellable,
            &mut error,
        )
    };

    if ok == GFALSE || token.is_null() {
        // SAFETY: `error` is NULL or a GError this call owns; reading its
        // message borrows a string the struct owns, and freeing it afterwards
        // is what the out-parameter contract asks for.
        let message = unsafe {
            let message = if error.is_null() {
                None
            } else {
                jmap_backend_core::marshal::read_string((*error).message)
            };
            if !error.is_null() {
                g_error_free(error);
            }
            message
        };
        return Err(StoreError::OAuth2(message.unwrap_or_else(|| {
            "no OAuth 2.0 access token could be obtained".to_owned()
        })));
    }

    // SAFETY: `token` is non-NULL by the check above, a g_malloc'd string this
    // call owns; `take_string` copies it and frees the original.
    Ok(unsafe { take_string(token) }.unwrap_or_default())
}
