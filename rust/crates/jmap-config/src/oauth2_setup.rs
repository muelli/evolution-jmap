// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a host name into a stored [`crate::oauth2::Config`] — RFC 8414
//! discovery and RFC 7591 dynamic client registration, the network half of
//! setting an account up for OAuth 2.0.
//!
//! [`crate::oauth2_service`]'s vfuncs and [`crate::oauth2`]'s storage both
//! need a deployment's endpoints and a client id before an account can
//! authenticate this way, and neither can go get them: `EOAuth2Service`'s
//! vfuncs are synchronous, so there is no vfunc that could make the network
//! calls the first time EDS asks one. [`discover_and_register`] is that
//! network call, made once by whatever sets an account up; its answer is
//! exactly the [`crate::oauth2::Config`] [`crate::oauth2::apply`] already
//! knows how to store, unchanged.
//!
//! ## What this deliberately does not do
//!
//! Choose the redirect URI, or drive the user to
//! `AuthorizationServer::authorization_endpoint` and capture what comes back.
//! That is the consent exchange itself, which needs a browser and a display
//! this crate's tests do not have — `docs/NIGHT-LOG.md` tracks it as still
//! open. [`discover_and_register`] takes `redirect_uri` as a plain argument
//! for exactly that reason: this module's job stops at discovering a
//! deployment's endpoints and registering a `client_id` against whichever
//! redirect URI the consent-flow increment settles on, not at picking one.

use jmap_backend_core::i18n::translate;
use jmap_backend_core::source::{self, SourceError};
use jmap_client::CancelFlag;
use jmap_client::oauth::{self, ClientRegistrationRequest};
use jmap_client::transport::{HttpMethod, HttpRequest, Transport};

use crate::oauth2::Config;

/// Everything that can keep [`discover_and_register`] from producing a usable
/// [`Config`].
#[derive(Debug)]
pub enum Error {
    /// `host` is not one [`source::origin`] will build an issuer from — the
    /// same rule (and the same TLS-for-non-loopback requirement) every other
    /// JMAP connection in this project is held to.
    Host(SourceError),
    /// RFC 8414 discovery or RFC 7591 registration itself failed: a transport
    /// error, a malformed document, or a deployment that refused the
    /// request.
    Client(jmap_client::Error),
    /// The deployment's metadata does not offer the authorization-code grant
    /// — the one flow `EOAuth2Service`'s vfuncs can drive.
    UnsupportedGrant,
    /// The deployment offers no RFC 7591 registration endpoint, so this
    /// client has no way to obtain a `client_id` on its own. Unlike EDS's
    /// compiled-in Google/Outlook/Yahoo services, JMAP names no registry of
    /// pre-issued ids to fall back to.
    NoRegistration,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
            Self::UnsupportedGrant => {
                f.write_str("this server does not offer the OAuth 2.0 authorization-code grant")
            }
            Self::NoRegistration => f.write_str(
                "this server offers no OAuth 2.0 client registration; an OAuth 2.0 account \
                 against it needs a client id from elsewhere",
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Discover `host`'s OAuth 2.0 endpoints (RFC 8414) and register this client
/// with it (RFC 7591), producing the [`Config`] [`crate::oauth2::apply`]
/// stores.
///
/// `host`, `port` and `secure` are read the same way every JMAP connection in
/// this project is (see [`source::origin`]): the authorization server for a
/// JMAP deployment's own account is, in every deployment this project
/// targets, the deployment itself, so discovery is asked of the same origin
/// `Session/get` is. `redirect_uri` is registered exactly as given — see the
/// module docs for why choosing it is not this function's job.
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

    // Register — and later request — exactly the scopes this client uses,
    // chosen from those the deployment advertises: the JMAP data scopes (as
    // `docs/OAUTH-FASTMAIL.md` records their spelling) plus `offline_access`
    // for a refresh token. Naming none is wrong (a registration naming no
    // `scope` can be issued an *empty* default — a token with no JMAP access
    // at all), and naming everything advertised is wrong too: the registered
    // set becomes the RFC 6749 §3.3 default, and a default containing
    // provider extras (Fastmail's MCP scope, OpenID Connect identity scopes)
    // has been seen rejected outright — `error=invalid_scope` at the
    // authorization endpoint, observed live 2026-08-23. A deployment naming
    // none (the pure RFC 8620 case, `scopes_supported` absent) keeps today's
    // behaviour: no `scope` sent, everything granted implicitly.
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

/// The RFC 8707 `resource` indicator for this deployment: the URL its JMAP
/// session resource actually lives at, found by asking `.well-known/jmap`
/// unauthenticated and taking the URL that answers (redirects followed) —
/// a 401/403 answers the question as well as a 200 does, since only the
/// location matters here, not the content.
///
/// That definition is not guessed: a deployment that publishes RFC 9728
/// protected-resource metadata names the same URL as its canonical
/// `resource` — verified live against Fastmail 2026-08-23, whose session's
/// 401 carries `WWW-Authenticate: Bearer resource_metadata="…"` naming
/// metadata with `"resource": "https://api.fastmail.com/jmap/session"`,
/// exactly the URL this probe lands on. `None` (network failure) omits the
/// parameter, which is the pre-RFC 8707 behaviour a deployment that never
/// heard of resource indicators expects.
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
