// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Changes a host name into a stored [`crate::oauth2::Config`]: RFC 8414
//! discovery and RFC 7591 dynamic client registration. This is the network
//! part of the OAuth 2.0 configuration of an account.
//!
//! [`crate::oauth2_service`]'s vfuncs and [`crate::oauth2`]'s storage both
//! need the endpoints and a client id of a deployment before an account can
//! use OAuth 2.0. Neither can get them: `EOAuth2Service`'s vfuncs are
//! synchronous, so no vfunc can make the network calls when EDS first asks.
//! [`discover_and_register`] makes these calls one time, when an account is
//! configured. Its result is exactly the [`crate::oauth2::Config`] that
//! [`crate::oauth2::apply`] stores, unchanged.
//!
//! ## What this module deliberately does not do
//!
//! It does not choose the redirect URI. It does not send the user to
//! `AuthorizationServer::authorization_endpoint` or capture the reply. That is
//! the consent exchange, which needs a browser and a display that this crate's
//! tests do not have. This gap is still open.
//! Thus [`discover_and_register`] takes `redirect_uri` as a plain argument.
//! This module discovers the endpoints of a deployment and registers a
//! `client_id` for the redirect URI that the consent-flow increment chooses.
//! It does not choose that URI.

use jmap_backend_core::i18n::translate;
use jmap_backend_core::source::{self, SourceError};
use jmap_client::CancelFlag;
use jmap_client::oauth::{self, ClientRegistrationRequest};
use jmap_client::transport::{HttpMethod, HttpRequest, Transport};

use crate::oauth2::Config;

/// The failures that stop [`discover_and_register`] before it makes a usable
/// [`Config`].
#[derive(Debug)]
pub enum Error {
    /// [`source::origin`] cannot make an issuer from `host`. This is the same
    /// rule (and the same TLS-for-non-loopback requirement) that applies to all
    /// other JMAP connections in this project.
    Host(SourceError),
    /// RFC 8414 discovery or RFC 7591 registration failed: a transport error, a
    /// malformed document, or a deployment that refused the request.
    Client(jmap_client::Error),
    /// The deployment's metadata does not offer the authorization-code grant.
    /// That grant is the only flow that `EOAuth2Service`'s vfuncs can operate.
    UnsupportedGrant,
    /// The deployment offers no RFC 7591 registration endpoint, so this client
    /// cannot get a `client_id` itself. EDS's compiled-in Google/Outlook/Yahoo
    /// services have pre-issued ids, but JMAP has no registry of such ids.
    NoRegistration,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
            Self::UnsupportedGrant => f.write_str(&translate(
                // TRANSLATORS: shown when a JMAP server's OAuth 2.0 metadata
                // does not list the authorization-code grant.
                c"this server does not offer the OAuth 2.0 authorization-code grant",
            )),
            Self::NoRegistration => f.write_str(&translate(
                // TRANSLATORS: shown when a JMAP server offers no RFC 7591
                // dynamic client registration endpoint.
                c"this server offers no OAuth 2.0 client registration; an OAuth 2.0 account against it needs a client id from elsewhere",
            )),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Allocates a `GError` that describes this failure. [`crate::config_lookup`]'s
    /// `run()` returns it through its `error` out-parameter, so the failure is
    /// not silent. The caller owns the `GError`. It must `g_error_free` it or
    /// give it to a C caller that frees it.
    ///
    /// Neither of `EConfigLookupWorkerError`'s two variants (a missing password,
    /// a certificate to trust) fits these failures, so this uses `G_IO_ERROR`:
    ///
    /// - `G_IO_ERROR_CANCELLED` for [`Error::Client`] that wraps
    ///   [`jmap_client::Error::Cancelled`]. All EDS layers use this code for
    ///   "the user pressed Stop" (`jmap_mail::connect::ConnectError::to_gerror`
    ///   reports cancellation the same way).
    /// - `G_IO_ERROR_FAILED` with the message of this error for all other
    ///   failures.
    pub fn to_gerror(&self) -> *mut glib_sys::GError {
        let message = jmap_backend_core::error::cstring_lossy(&self.to_string());
        let code = if matches!(self, Self::Client(jmap_client::Error::Cancelled)) {
            gio_sys::G_IO_ERROR_CANCELLED
        } else {
            gio_sys::G_IO_ERROR_FAILED
        };
        // SAFETY: the quark function takes no arguments. `message` is a live,
        // NUL-terminated `CString` during the call, and the call copies it.
        unsafe {
            glib_sys::g_error_new_literal(gio_sys::g_io_error_quark(), code, message.as_ptr())
        }
    }
}

/// Discovers the OAuth 2.0 endpoints of `host` (RFC 8414) and registers this
/// client with it (RFC 7591). The result is the [`Config`] that
/// [`crate::oauth2::apply`] stores.
///
/// `host`, `port` and `secure` have the same meaning as for all JMAP
/// connections in this project (see [`source::origin`]). In all deployments
/// that this project targets, the deployment is its own authorization server.
/// Thus discovery uses the same origin as `Session/get`. `redirect_uri` is
/// registered exactly as given. The module docs tell why this function does
/// not choose it.
pub fn discover_and_register(
    transport: &dyn Transport,
    host: &str,
    port: u16,
    secure: bool,
    redirect_uri: &str,
    cancel: Option<&CancelFlag>,
) -> Result<Config, Error> {
    let issuer = source::origin(Some(host), port, secure).map_err(Error::Host)?;
    tracing::debug!(
        issuer,
        "discovering OAuth 2.0 authorization server metadata"
    );

    let server = match oauth::discover(transport, &issuer, cancel) {
        Ok(server) => server,
        Err(error) => {
            tracing::warn!(issuer, %error, "OAuth 2.0 discovery failed");
            return Err(Error::Client(error));
        }
    };
    if !server.supports_authorization_code() {
        tracing::warn!(
            issuer,
            "OAuth 2.0 server does not offer authorization code grant"
        );
        return Err(Error::UnsupportedGrant);
    }
    let registration_endpoint = match server.registration_endpoint {
        Some(endpoint) => endpoint,
        None => {
            tracing::warn!(issuer, "OAuth 2.0 server offers no registration endpoint");
            return Err(Error::NoRegistration);
        }
    };

    // Register (and later request) only the scopes that this client uses, from
    // the scopes that the deployment advertises. These are the JMAP data scopes
    // (spelled as Fastmail advertises them, below) and `offline_access` for a
    // refresh token.
    // A registration without `scope` is wrong: it can get an *empty* default,
    // which is a token with no JMAP access.
    // A registration with all advertised scopes is also wrong: the registered
    // set becomes the RFC 6749 §3.3 default. A server rejected a default with provider extras
    // (Fastmail's MCP scope, OpenID Connect identity scopes) with
    // `error=invalid_scope` at the authorization endpoint (seen live 2026-08-23).
    // If a deployment advertises no scopes (the pure RFC 8620 case,
    // `scopes_supported` absent), no `scope` is sent and all access is implicit.
    const REQUESTED_SCOPES: [&str; 4] = [
        "urn:ietf:params:oauth:scope:mail",
        "urn:ietf:params:oauth:scope:contacts",
        "urn:ietf:params:oauth:scope:calendars",
        "offline_access",
    ];
    let picked: Vec<&str> = server
        .scopes_supported
        .iter()
        .map(String::as_str)
        .filter(|advertised| REQUESTED_SCOPES.contains(advertised))
        .collect();
    let scope = (!picked.is_empty()).then(|| picked.join(" "));

    tracing::debug!(
        issuer,
        registration_endpoint,
        ?scope,
        "registering OAuth 2.0 dynamic client"
    );

    let registered = match oauth::register_client(
        transport,
        &registration_endpoint,
        &ClientRegistrationRequest {
            // TRANSLATORS: this client's name, shown on the consent page a real identity provider renders.
            client_name: &translate(c"Evolution"),
            redirect_uris: &[redirect_uri],
            scope: scope.as_deref(),
        },
        cancel,
    ) {
        Ok(reg) => reg,
        Err(error) => {
            tracing::warn!(
                issuer,
                registration_endpoint,
                %error,
                "OAuth 2.0 client registration failed"
            );
            return Err(Error::Client(error));
        }
    };

    tracing::debug!(
        issuer,
        client_id = registered.client_id,
        "registered OAuth 2.0 dynamic client"
    );

    Ok(Config {
        client_id: Some(registered.client_id),
        client_secret: registered.client_secret,
        authorization_endpoint: server.authorization_endpoint,
        token_endpoint: server.token_endpoint,
        redirect_uri: Some(redirect_uri.to_owned()),
        scope,
        resource: probe_resource(transport, &issuer, cancel),
    })
}

/// Returns the RFC 8707 `resource` indicator for this deployment: the URL of
/// its JMAP session resource. The probe sends an unauthenticated request to
/// `.well-known/jmap` and takes the URL that answers (after redirects). Only
/// the location is important, not the content, so a 401/403 is as good as a 200.
///
/// This definition is not a guess. A deployment that publishes RFC 9728
/// protected-resource metadata gives the same URL as its canonical `resource`.
/// A live test against Fastmail (2026-08-23) showed this: the session's 401
/// has `WWW-Authenticate: Bearer resource_metadata="…"`, and that metadata has
/// `"resource": "https://api.fastmail.com/jmap/session"`, which is the probe URL.
/// `None` (network failure, or a status other than 200/401/403) omits the
/// parameter. That is the pre-RFC 8707 behaviour that older deployments expect.
fn probe_resource(
    transport: &dyn Transport,
    issuer: &str,
    cancel: Option<&CancelFlag>,
) -> Option<String> {
    let url = format!("{issuer}/.well-known/jmap");
    tracing::debug!(issuer, url, "probing RFC 8707 protected-resource indicator");
    let response = match transport.execute(HttpRequest {
        method: HttpMethod::Get,
        url: &url,
        headers: &[("Accept".to_owned(), "application/json".to_owned())],
        body: None,
        cancel,
        max_response_bytes: 64 * 1024,
    }) {
        Ok(resp) => resp,
        Err(error) => {
            tracing::debug!(
                issuer,
                url,
                ?error,
                "protected-resource indicator probe failed"
            );
            return None;
        }
    };
    let matched = matches!(response.status, 200 | 401 | 403);
    tracing::debug!(
        issuer,
        url,
        status = response.status,
        final_url = response.final_url,
        matched,
        "protected-resource indicator probe finished"
    );
    matched.then_some(response.final_url)
}

#[cfg(test)]
mod tests {
    use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};

    use super::{Error, probe_resource};

    /// A [`Transport`] that always gives the same answer. It tests branches of
    /// `probe_resource` that `jmap-mock` cannot reach: a transport failure, or a
    /// status outside the 200/401/403 set that counts as a match.
    enum Fixed {
        Fails,
        Answers {
            status: u16,
            final_url: &'static str,
        },
    }

    impl Transport for Fixed {
        fn execute(&self, _request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            match self {
                Self::Fails => Err(TransportError::Failed("simulated failure".into())),
                Self::Answers { status, final_url } => Ok(HttpResponse {
                    status: *status,
                    content_type: None,
                    body: Vec::new(),
                    final_url: (*final_url).to_owned(),
                }),
            }
        }
    }

    #[test]
    fn a_transport_failure_omits_the_resource_indicator() {
        assert_eq!(
            probe_resource(&Fixed::Fails, "https://jmap.example.com", None),
            None
        );
    }

    #[test]
    fn a_status_outside_200_401_403_omits_the_resource_indicator() {
        let transport = Fixed::Answers {
            status: 500,
            final_url: "https://jmap.example.com/.well-known/jmap",
        };
        assert_eq!(
            probe_resource(&transport, "https://jmap.example.com", None),
            None
        );
    }

    #[test]
    fn cancellation_reads_back_as_g_io_error_cancelled() {
        let error = Error::Client(jmap_client::Error::Cancelled).to_gerror();
        assert!(!error.is_null());
        // SAFETY: this test owns this newly allocated GError.
        unsafe {
            assert_eq!((*error).domain, gio_sys::g_io_error_quark());
            assert_eq!((*error).code, gio_sys::G_IO_ERROR_CANCELLED);
            glib_sys::g_error_free(error);
        }
    }

    #[test]
    fn display_names_the_underlying_cause_for_every_variant() {
        assert!(
            Error::Host(jmap_backend_core::source::SourceError::MissingHost)
                .to_string()
                .contains("JMAP server")
        );
        assert!(
            Error::Client(jmap_client::Error::Transport("no route to host".into()))
                .to_string()
                .contains("no route to host")
        );
        assert!(
            Error::UnsupportedGrant
                .to_string()
                .contains("authorization-code grant")
        );
        assert!(
            Error::NoRegistration
                .to_string()
                .contains("client registration")
        );
    }

    #[test]
    fn every_other_failure_reads_back_as_g_io_error_failed_with_its_message() {
        let cases = [
            Error::UnsupportedGrant,
            Error::NoRegistration,
            Error::Client(jmap_client::Error::Transport("no route to host".into())),
        ];
        for err in cases {
            let message = err.to_string();
            let error = err.to_gerror();
            assert!(!error.is_null());
            // SAFETY: this test owns this newly allocated GError.
            unsafe {
                assert_eq!((*error).domain, gio_sys::g_io_error_quark());
                assert_eq!((*error).code, gio_sys::G_IO_ERROR_FAILED);
                assert_eq!(
                    std::ffi::CStr::from_ptr((*error).message).to_str().unwrap(),
                    message
                );
                glib_sys::g_error_free(error);
            }
        }
    }
}
