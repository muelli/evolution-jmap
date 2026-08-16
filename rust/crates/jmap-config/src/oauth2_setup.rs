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
use jmap_client::transport::Transport;

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

    let server = oauth::discover(transport, &issuer, cancel).map_err(Error::Client)?;
    if !server.supports_authorization_code() {
        return Err(Error::UnsupportedGrant);
    }
    let registration_endpoint = server.registration_endpoint.ok_or(Error::NoRegistration)?;

    let registered = oauth::register_client(
        transport,
        &registration_endpoint,
        &ClientRegistrationRequest {
            // TRANSLATORS: this client's name, shown on the consent page a real identity provider renders.
            client_name: &translate(c"Evolution"),
            redirect_uris: &[redirect_uri],
        },
        cancel,
    )
    .map_err(Error::Client)?;

    Ok(Config {
        client_id: Some(registered.client_id),
        client_secret: registered.client_secret,
        authorization_endpoint: server.authorization_endpoint,
        token_endpoint: server.token_endpoint,
        redirect_uri: Some(redirect_uri.to_owned()),
    })
}
