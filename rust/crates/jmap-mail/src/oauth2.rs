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
        // SAFETY: `error` is NULL or a GError this call owns; `trace_failure`
        // only reads it, and freeing it afterwards is what the out-parameter
        // contract asks for.
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

    // SAFETY: `token` is non-NULL by the check above, a g_malloc'd string this
    // call owns; `take_string` copies it and frees the original.
    Ok(unsafe { take_string(token) }.unwrap_or_default())
}

/// Classifies a failed access-token fetch and traces it, returning the
/// message [`StoreError::OAuth2`] carries.
///
/// Split out of [`access_token`] so the classification can be driven by a
/// test with a hand-built `GError`, the same way
/// `jmap_backend_core::oauth2::access_token`'s own tracing is tested — that
/// EDS-side token fetch already traces `reason`/`escalates_to_consent` (see
/// its module docs and `docs/ROADMAP.md` item 20's "make every consent
/// escalation traceable" ask), but this call site, Camel's
/// `camel_session_get_oauth2_access_token_sync`, logged nothing at all before
/// this: a failure here — including the item-22 stale-D-Bus-proxy shape,
/// which this reuses [`jmap_backend_core::oauth2::classify_failure`] to
/// recognise identically — was invisible in the journal from the mail/Camel
/// side even though the equivalent EDS-side fetch already names it. Purely
/// additive: the returned message is unchanged from what this call site
/// already sent into [`StoreError::OAuth2`].
///
/// # Safety
///
/// `error` must be NULL or a valid `GError` this call does not own — read
/// only, never freed here.
unsafe fn trace_failure(error: *const GError) -> Option<String> {
    // SAFETY: the contract above is `classify_failure`'s own.
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
    use std::sync::{Arc, Mutex};

    use gio_sys::{G_DBUS_ERROR_SERVICE_UNKNOWN, g_dbus_error_quark, g_io_error_quark};
    use glib_sys::g_error_new_literal;

    use super::*;

    /// A real `GError`, not a hand-rolled struct: [`trace_failure`] reads
    /// `classify_failure`'s output, which reads the `domain`/`code`/`message`
    /// fields directly — see `jmap_backend_core::oauth2`'s own tests for why.
    fn error(domain: glib_sys::GQuark, code: i32, text: &str) -> *mut GError {
        let message = CString::new(text).unwrap();
        // SAFETY: a valid domain and a NUL-terminated message; every caller
        // below frees the result.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    /// Records every event's field name → value, duplicated from
    /// `jmap_mail::service`'s own harness for the same reason that one
    /// gives: this crate depends on `tracing`, not `tracing-subscriber`.
    struct CapturingSubscriber {
        captured: Arc<Mutex<Vec<(String, String)>>>,
    }

    struct Recorder<'a> {
        sink: &'a Mutex<Vec<(String, String)>>,
    }

    impl tracing::field::Visit for Recorder<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.sink
                .lock()
                .unwrap()
                .push((field.name().to_owned(), format!("{value:?}")));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.sink
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.sink
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_owned()));
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut Recorder {
                sink: &self.captured,
            });
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    fn run_captured(error: *const GError) -> (Option<String>, Vec<(String, String)>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };
        let message =
            tracing::subscriber::with_default(subscriber, || unsafe { trace_failure(error) });
        let captured = captured.lock().unwrap().clone();
        (message, captured)
    }

    /// The item-22 shape: a dead D-Bus peer behind the OAuth2 token fetch.
    /// Traced as `secret_store_failure`/`escalates_to_consent=false`, exactly
    /// as `jmap_backend_core::oauth2::access_token` already traces the same
    /// failure on the EDS side — this call site had no trace at all before.
    #[test]
    fn a_dead_dbus_peer_is_traced_as_a_secret_store_failure() {
        let error = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SERVICE_UNKNOWN,
            "The name :1.4 was not provided by any .service files",
        );
        let (message, captured) = run_captured(error);
        unsafe { g_error_free(error) };

        assert_eq!(
            message.as_deref(),
            Some("The name :1.4 was not provided by any .service files")
        );
        assert!(
            captured.contains(&("reason".to_owned(), "secret_store_failure".to_owned())),
            "expected reason=secret_store_failure, got {captured:?}"
        );
        assert!(
            captured.contains(&("escalates_to_consent".to_owned(), "false".to_owned())),
            "expected escalates_to_consent=false, got {captured:?}"
        );
    }

    /// A genuine "nobody has consented yet" failure still escalates, and is
    /// traced saying so — the message this crate has always returned is
    /// unchanged, only the trace is new.
    #[test]
    fn a_missing_consent_is_traced_as_escalating() {
        let error = error(
            unsafe { g_io_error_quark() },
            gio_sys::G_IO_ERROR_NOT_SUPPORTED,
            "no registered OAuth2 service",
        );
        let (message, captured) = run_captured(error);
        unsafe { g_error_free(error) };

        assert_eq!(message.as_deref(), Some("no registered OAuth2 service"));
        assert!(
            captured.contains(&("escalates_to_consent".to_owned(), "true".to_owned())),
            "expected escalates_to_consent=true, got {captured:?}"
        );
    }

    /// A NULL `GError*` — `access_token`'s own fallback message covers this,
    /// [`trace_failure`] must not dereference it.
    #[test]
    fn a_null_error_traces_without_a_message() {
        let (message, captured) = run_captured(std::ptr::null());
        assert_eq!(message, None);
        assert!(
            captured.contains(&("reason".to_owned(), "null_error".to_owned())),
            "expected reason=null_error, got {captured:?}"
        );
    }
}
