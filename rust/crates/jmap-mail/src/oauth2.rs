// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tells if a mail account uses OAuth 2.0, and gets its access token.
//!
//! ## The method field
//!
//! [`jmap_backend_core::oauth2`] reads `[Authentication] Method` from the
//! `ESource` of an address book or calendar. A Camel service has no `ESource`.
//! `ESourceCamel` binds the same method to `CamelNetworkSettings:auth-mechanism`,
//! so this module asks [`method_is_oauth2`] about that field. Thus the mail
//! side and the EDS side always agree on which accounts use OAuth 2.0.
//!
//! [`method_is_oauth2`]: jmap_backend_core::oauth2::method_is_oauth2
//!
//! ## The token source
//!
//! `camel_session_get_oauth2_access_token_sync` is the Camel equivalent of
//! `e_source_get_oauth2_access_token_sync`. In Evolution, the `EMailSession`
//! override (`libemail-engine/e-mail-session.c`,
//! `mail_session_get_oauth2_access_token_sync`) finds the `ESource` of the
//! service by uid and calls that `ESource` function. Thus the mail side gets
//! the same token as the other backends.
//!
//! ## The `CamelAuthenticationResult` of a failure
//!
//! [`crate::connect::StoreError::OAuth2`] decides this, not this module. Its
//! doc comment tells why a failed token fetch is `CAMEL_AUTHENTICATION_ERROR`
//! and not `REJECTED`.

use eds_sys::{
    CamelService, CamelSession, CamelSettings, camel_network_settings_dup_auth_mechanism,
    camel_session_get_oauth2_access_token_sync,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, g_error_free};

use crate::connect::StoreError;
use crate::server::{network, take_string};

/// Tells if the `auth-mechanism` of `settings` names OAuth 2.0. The module
/// docs tell why this field is used.
///
/// # Safety
///
/// `settings` must be NULL or a valid `CamelSettings`. This function only reads
/// it, and nothing outlives the call.
pub unsafe fn uses_oauth2(settings: *mut CamelSettings) -> bool {
    // SAFETY: the contract above is the contract of `network`.
    let Some(network) = (unsafe { network(settings) }) else {
        return false;
    };
    // SAFETY: `network` guarantees that its result implements the interface.
    // The `dup_` accessor returns a g_malloc'd copy, not a pointer into storage
    // that another thread can replace. `take_string` frees the copy.
    let method = unsafe { take_string(camel_network_settings_dup_auth_mechanism(network)) };
    jmap_backend_core::oauth2::method_is_oauth2(method.as_deref())
}

/// The OAuth 2.0 access token to send this account's requests as.
///
/// The refresh occurs in this call, as in
/// [`e_source_get_oauth2_access_token_sync`]. Thus a returned token is valid
/// now. A failure (no consent yet, or a failed exchange) is
/// [`StoreError::OAuth2`].
///
/// [`e_source_get_oauth2_access_token_sync`]: eds_sys::e_source_get_oauth2_access_token_sync
///
/// # Safety
///
/// `session` must be a valid `CamelSession`, `service` a valid `CamelService`
/// registered on it, and `cancellable` NULL or a valid `GCancellable`. `attempt`
/// has all three when it calls this function.
pub unsafe fn access_token(
    session: *mut CamelSession,
    service: *mut CamelService,
    cancellable: *mut GCancellable,
) -> Result<String, StoreError> {
    let mut token = std::ptr::null_mut();
    let mut expires_in = 0;
    let mut error: *mut GError = std::ptr::null_mut();

    // SAFETY: the contract of this function gives a valid session, a valid
    // service and a NULL-or-valid cancellable. The three out-parameters are
    // writable. This call owns the returned token and the GError (GLib
    // allocations).
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
        // SAFETY: `error` is NULL or a GError that this call owns.
        // `trace_failure` only reads it. The out-parameter contract tells the
        // caller to free it after.
        let message = unsafe {
            let message = trace_failure(error);
            if !error.is_null() {
                g_error_free(error);
            }
            message
        };
        return Err(StoreError::OAuth2(message.unwrap_or_else(|| {
            "no OAuth 2.0 access token could be obtained".to_owned()
        })));
    }

    // SAFETY: the check above makes `token` non-NULL. It is a g_malloc'd string
    // that this call owns. `take_string` copies it and frees the original.
    Ok(unsafe { take_string(token) }.unwrap_or_default())
}

/// Classifies a failed access-token fetch, traces it, and returns the message
/// for [`StoreError::OAuth2`].
///
/// This is a separate function so that a test can supply a hand-built `GError`.
/// It uses [`classify_failure`], thus the mail side and the EDS side trace the
/// same reasons (for example, a stale D-Bus proxy). The returned message is the
/// message of the `GError`.
///
/// [`classify_failure`]: jmap_backend_core::oauth2::classify_failure
///
/// # Safety
///
/// `error` must be NULL or a valid `GError` that this call does not own. This
/// function only reads it and does not free it.
unsafe fn trace_failure(error: *const GError) -> Option<String> {
    // SAFETY: the contract above is the contract of `classify_failure`.
    let (reason, error_domain, error_domain_name, error_code, message) =
        unsafe { jmap_backend_core::oauth2::classify_failure(error) };
    tracing::debug!(
        reason = reason.as_str(),
        escalates_to_consent = reason.escalates_to_consent(),
        error_domain = error_domain.as_deref(),
        error_domain_name = error_domain_name.as_deref(),
        error_code,
        error_message = message.as_deref(),
        "failed to obtain OAuth 2.0 access token for a mail account"
    );
    message
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use gio_sys::{G_DBUS_ERROR_SERVICE_UNKNOWN, g_dbus_error_quark, g_io_error_quark};
    use glib_sys::g_error_new_literal;
    use jmap_tracing_capture::capture;

    use super::*;

    /// Makes a real `GError`, not a hand-built struct. `classify_failure` reads
    /// the `domain`, `code` and `message` fields directly.
    fn error(domain: glib_sys::GQuark, code: i32, text: &str) -> *mut GError {
        let message = CString::new(text).unwrap();
        // SAFETY: a valid domain and a NUL-terminated message. Every caller
        // below frees the result.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    fn run_captured(
        error: *const GError,
    ) -> (Option<String>, Vec<(tracing::Level, String, String)>) {
        // SAFETY: every caller passes NULL or a live GError it owns and frees.
        capture(|| unsafe { trace_failure(error) })
    }

    /// A dead D-Bus peer behind the token fetch traces as `service_gone` with
    /// `escalates_to_consent=false`, as on the EDS side.
    ///
    /// The reason is `service_gone`, not `secret_store_failure`. Both are
    /// members of [`SilentRefreshFailureReason`] and neither escalates to
    /// consent.
    ///
    /// [`SilentRefreshFailureReason`]: jmap_backend_core::oauth2::SilentRefreshFailureReason
    #[test]
    fn a_dead_dbus_peer_is_traced_as_service_gone() {
        // SAFETY: a registered GLib error quark, no arguments.
        let error = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SERVICE_UNKNOWN,
            "The name :1.4 was not provided by any .service files",
        );
        let (message, captured) = run_captured(error);
        // SAFETY: a GError this test owns and has finished reading.
        unsafe { g_error_free(error) };

        assert_eq!(
            message.as_deref(),
            Some("The name :1.4 was not provided by any .service files")
        );
        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "reason" && value == "service_gone"),
            "expected reason=service_gone, got {captured:?}"
        );
        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "escalates_to_consent" && value == "false"),
            "expected escalates_to_consent=false, got {captured:?}"
        );
    }

    /// A missing consent escalates, and the trace tells so. The returned
    /// message is the message of the `GError`.
    #[test]
    fn a_missing_consent_is_traced_as_escalating() {
        // SAFETY: a registered GLib error quark, no arguments.
        let error = error(
            unsafe { g_io_error_quark() },
            gio_sys::G_IO_ERROR_NOT_SUPPORTED,
            "no registered OAuth2 service",
        );
        let (message, captured) = run_captured(error);
        // SAFETY: a GError this test owns and has finished reading.
        unsafe { g_error_free(error) };

        assert_eq!(message.as_deref(), Some("no registered OAuth2 service"));
        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "escalates_to_consent" && value == "true"),
            "expected escalates_to_consent=true, got {captured:?}"
        );
    }

    /// A NULL `GError*`. [`trace_failure`] must not dereference it. The
    /// fallback message of `access_token` covers this case.
    #[test]
    fn a_null_error_traces_without_a_message() {
        let (message, captured) = run_captured(std::ptr::null());
        assert_eq!(message, None);
        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "reason" && value == "null_error"),
            "expected reason=null_error, got {captured:?}"
        );
    }
}
