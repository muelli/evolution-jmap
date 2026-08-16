// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a JMAP deployment's OAuth 2.0 endpoints come from (RFC 8414).
//!
//! EDS authenticates an OAuth 2.0 account through an `EOAuth2Service`, whose
//! vfuncs are asked for an authorization URI, a token URI, a client id and a
//! redirect URI. The three services EDS ships — Google, Outlook, Yahoo —
//! answer those from constants compiled in, because each of them is exactly
//! one identity provider. A JMAP service cannot: "JMAP" is not a provider but
//! a protocol, and the endpoints belong to whichever deployment the account
//! lives on. Fastmail's are not Stalwart's, and a self-hosted Stalwart's are
//! not another one's.
//!
//! So they are discovered rather than compiled in. RFC 8414 is the mechanism
//! both target deployments already publish: a JSON document at a well-known
//! path naming the authorization and token endpoints, which grant types and
//! PKCE methods the server does, and — where the server supports RFC 7591
//! dynamic client registration — where to register for a client id.
//!
//! This module is the half of that which can be built and tested here: fetch,
//! validate, and hand back the document. Nothing in it performs a flow. The
//! consent exchange needs a browser and a real provider, and the vfuncs that
//! wrap this need EDS; both are for later slices.
//!
//! ## What is enforced, and what is not
//!
//! The security-relevant check is RFC 8414 §3.3: the `issuer` in the document
//! must be *identical* to the issuer the well-known URL was built from. Skip
//! it and a deployment can hand back another authorization server's endpoints,
//! sending the user to consent somewhere the client thinks is the deployment
//! and delivering the authorization code to a third party. It is the same
//! mix-up defence OpenID Connect's discovery carries, and it is why this
//! function takes the issuer as an argument rather than reading it out of the
//! answer.
//!
//! Not enforced here: that the issuer is `https`. RFC 8414 §2 requires it, but
//! nothing else in this client requires TLS either — the session document,
//! `apiUrl` and every blob URL are taken as the deployment states them — and
//! adding the rule in one place only would be a check the user cannot rely on
//! while making the plaintext mock untestable. Where TLS gets required it has
//! to be required of the account as a whole; that is its own piece of work.

use serde::Deserialize;

use crate::error::Error;
use crate::limits;
use crate::transport::{CancelFlag, HttpMethod, HttpRequest, Transport, TransportError};

/// The path RFC 8414 §3 reserves for the metadata document.
const WELL_KNOWN: &str = "/.well-known/oauth-authorization-server";

/// What RFC 8414 §2 says `grant_types_supported` means when it is absent.
const DEFAULT_GRANT_TYPES: [&str; 2] = ["authorization_code", "implicit"];

/// An authorization server's metadata, as RFC 8414 §2 defines it — the fields
/// an EDS `EOAuth2Service` for a JMAP account needs, and no others.
///
/// The document real servers publish carries many more; they are parsed past
/// rather than rejected, because a field this client has no use for is not a
/// reason to refuse an account.
///
/// Only [`discover`] and [`AuthorizationServer::parse`] construct one, and
/// both validate: a value of this type has been checked against the issuer it
/// was asked for, and every endpoint in it is an absolute URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationServer {
    /// The issuer identifier, equal to the (normalised) one discovery asked
    /// for — see this module's note on RFC 8414 §3.3.
    pub issuer: String,
    /// Where the user is sent to consent. `None` only for a server that does
    /// no grant type using it, which is not one a JMAP account can use.
    pub authorization_endpoint: Option<String>,
    /// Where an authorization code is exchanged for a token, and where a
    /// refresh token is redeemed. EDS asks for these two separately
    /// (`get_authentication_uri` and `get_refresh_uri`), RFC 6749 has one
    /// endpoint serve both, and this is it.
    pub token_endpoint: Option<String>,
    /// RFC 7591 dynamic client registration, where the server offers it.
    /// Its presence is what lets a self-hosted deployment be used at all
    /// without the user pasting in a client id by hand.
    pub registration_endpoint: Option<String>,
    /// RFC 8628 device authorization, for a flow that needs no browser in the
    /// client. Recorded because it is the one path to a token that does not
    /// need the display this project cannot assume.
    pub device_authorization_endpoint: Option<String>,
    /// Scopes the server advertises. JMAP's own are the capability URNs.
    pub scopes_supported: Vec<String>,
    /// Response types, RFC 8414 §2's one required field besides the issuer.
    pub response_types_supported: Vec<String>,
    /// Grant types, with §2's default applied when the server named none.
    pub grant_types_supported: Vec<String>,
    /// PKCE challenge methods (RFC 7636), where the server names any.
    pub code_challenge_methods_supported: Vec<String>,
}

/// The document as it arrives, before anything about it has been checked.
///
/// Separate from [`AuthorizationServer`] so that the validated type cannot be
/// deserialised into directly: a cached document parsed straight into it would
/// be one that never met the issuer check.
#[derive(Debug, Deserialize)]
struct Document {
    issuer: Option<String>,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    registration_endpoint: Option<String>,
    device_authorization_endpoint: Option<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
    #[serde(default)]
    response_types_supported: Vec<String>,
    grant_types_supported: Option<Vec<String>>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

impl AuthorizationServer {
    /// Parse and validate a metadata document said to be `issuer`'s.
    ///
    /// `issuer` is the identifier the document was fetched *for* — normally
    /// what [`discover`] was given. It is normalised the same way
    /// [`metadata_url`] normalises it, so the comparison is against the string
    /// the well-known URL was actually built from.
    pub fn parse(issuer: &str, body: &[u8]) -> Result<Self, Error> {
        let (issuer, _) = well_known_for(issuer)?;
        let document: Document = serde_json::from_slice(body)?;

        // RFC 8414 §3.3, and the reason this function takes an issuer at all.
        match document.issuer.as_deref() {
            Some(stated) if stated == issuer => {}
            Some(stated) => {
                return Err(Error::Protocol(format!(
                    "the metadata document at {issuer} names issuer {stated}; \
                     RFC 8414 §3.3 requires them to be identical"
                )));
            }
            None => {
                return Err(Error::Protocol(format!(
                    "the metadata document at {issuer} states no issuer"
                )));
            }
        }

        Ok(Self {
            issuer,
            authorization_endpoint: endpoint(
                "authorization_endpoint",
                document.authorization_endpoint,
            )?,
            token_endpoint: endpoint("token_endpoint", document.token_endpoint)?,
            registration_endpoint: endpoint(
                "registration_endpoint",
                document.registration_endpoint,
            )?,
            device_authorization_endpoint: endpoint(
                "device_authorization_endpoint",
                document.device_authorization_endpoint,
            )?,
            scopes_supported: document.scopes_supported,
            response_types_supported: document.response_types_supported,
            grant_types_supported: document
                .grant_types_supported
                .unwrap_or_else(|| DEFAULT_GRANT_TYPES.map(str::to_owned).to_vec()),
            code_challenge_methods_supported: document.code_challenge_methods_supported,
        })
    }

    /// Whether this server does the authorization-code flow — the one an
    /// `EOAuth2Service` drives.
    pub fn supports_authorization_code(&self) -> bool {
        self.authorization_endpoint.is_some()
            && self.token_endpoint.is_some()
            && self
                .grant_types_supported
                .iter()
                .any(|grant| grant == "authorization_code")
    }
}

/// Fetch and validate `issuer`'s metadata document.
///
/// Asks with no credentials, which is not an omission: the document says where
/// to *obtain* credentials, so RFC 8414 §3 has it publicly readable and a
/// client that had to authenticate for it could never get started.
pub fn discover(
    transport: &dyn Transport,
    issuer: &str,
    cancel: Option<&CancelFlag>,
) -> Result<AuthorizationServer, Error> {
    let (issuer, url) = well_known_for(issuer)?;

    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(Error::Cancelled);
    }

    let headers = [("Accept".to_owned(), "application/json".to_owned())];
    let response = transport
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &url,
            headers: &headers,
            body: None,
            cancel,
            max_response_bytes: limits::MAX_OAUTH_METADATA_BYTES,
        })
        .map_err(|error| match error {
            TransportError::Cancelled => Error::Cancelled,
            TransportError::Failed(message) => Error::Transport(message),
            TransportError::ResponseTooLarge { limit } => Error::ResponseTooLarge { limit },
        })?;

    if !(200..300).contains(&response.status) {
        // No RFC 7807 problem details are parsed out of this one: it is not a
        // JMAP endpoint, and a 404 here means only "this deployment publishes
        // no OAuth 2.0 metadata", which the status already says.
        return Err(Error::Http {
            status: response.status,
            problem: None,
        });
    }

    AuthorizationServer::parse(&issuer, &response.body)
}

/// The URL `issuer`'s metadata document lives at.
///
/// RFC 8414 §3.1 *inserts* the well-known path between the host and the path
/// of the issuer identifier rather than appending it — so a deployment at
/// `https://example.com/tenant1` publishes at
/// `https://example.com/.well-known/oauth-authorization-server/tenant1`. The
/// intuitive append gives `…/tenant1/.well-known/…`, which is what OpenID
/// Connect Discovery does and what RFC 8414 deliberately does not.
pub fn metadata_url(issuer: &str) -> Result<String, Error> {
    well_known_for(issuer).map(|(_, url)| url)
}

/// The normalised issuer identifier and the URL its document lives at, which
/// are derived together because §3.3's check compares against the former and
/// only the latter is fetched.
fn well_known_for(issuer: &str) -> Result<(String, String), Error> {
    let (scheme, rest) = issuer
        .split_once("://")
        .ok_or_else(|| Error::Protocol(format!("{issuer} is not an absolute URL")))?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(Error::Protocol(format!("{issuer} is not an http(s) URL")));
    }
    // RFC 8414 §2: an issuer identifier has neither. Left in, they would also
    // make the inserted path meaningless.
    if rest.contains('?') || rest.contains('#') {
        return Err(Error::Protocol(format!(
            "{issuer} carries a query or fragment; RFC 8414 §2 forbids both in an issuer"
        )));
    }

    let (authority, path) = match rest.find('/') {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(Error::Protocol(format!("{issuer} names no host")));
    }

    // A trailing slash is dropped so that `https://example.com/` and
    // `https://example.com` are one issuer rather than two. Normalising the
    // caller's string is the only latitude taken: what comes *back* is
    // compared byte for byte, per §3.3.
    let path = path.trim_end_matches('/');
    Ok((
        format!("{scheme}://{authority}{path}"),
        format!("{scheme}://{authority}{WELL_KNOWN}{path}"),
    ))
}

/// An endpoint URL as it may be used — absolute, and http(s).
///
/// A relative or otherwise odd value would be handed to a browser widget or
/// posted to; `javascript:` and `data:` are the reason the scheme is checked
/// rather than merely the absoluteness.
fn endpoint(name: &str, value: Option<String>) -> Result<Option<String>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    let scheme = value
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(Error::Protocol(format!(
            "{name} is {value}, which is not an absolute http(s) URL"
        )));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::{AuthorizationServer, metadata_url};
    use crate::error::Error;

    #[test]
    fn a_deployment_at_the_root_publishes_at_the_well_known_path() {
        assert_eq!(
            metadata_url("https://jmap.example.com").expect("a valid issuer"),
            "https://jmap.example.com/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn a_trailing_slash_names_the_same_deployment() {
        assert_eq!(
            metadata_url("https://jmap.example.com/").expect("a valid issuer"),
            "https://jmap.example.com/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn a_path_carrying_issuer_has_the_well_known_path_inserted_before_it() {
        // RFC 8414 §3.1's own example, and the whole reason this is not a
        // string concatenation.
        assert_eq!(
            metadata_url("https://example.com/issuer1").expect("a valid issuer"),
            "https://example.com/.well-known/oauth-authorization-server/issuer1"
        );
    }

    #[test]
    fn a_port_is_part_of_the_authority_not_the_path() {
        assert_eq!(
            metadata_url("http://127.0.0.1:8080").expect("a valid issuer"),
            "http://127.0.0.1:8080/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn an_issuer_that_is_not_a_url_is_refused() {
        assert!(matches!(
            metadata_url("jmap.example.com"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn an_issuer_carrying_a_query_or_fragment_is_refused() {
        assert!(matches!(
            metadata_url("https://example.com/t?a=b"),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            metadata_url("https://example.com/t#f"),
            Err(Error::Protocol(_))
        ));
    }

    /// A minimal well-formed document for `https://jmap.example.com`.
    fn document(body: &str) -> Result<AuthorizationServer, Error> {
        AuthorizationServer::parse("https://jmap.example.com", body.as_bytes())
    }

    #[test]
    fn a_document_stating_no_issuer_is_refused() {
        assert!(matches!(
            document(r#"{"authorization_endpoint": "https://jmap.example.com/a"}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn an_endpoint_that_is_not_an_absolute_url_is_refused() {
        // A server that answered with a path would otherwise have the client
        // resolve it against whatever base happened to be around.
        let error =
            document(r#"{"issuer": "https://jmap.example.com", "token_endpoint": "/oauth/token"}"#)
                .expect_err("a relative endpoint");
        assert!(
            matches!(&error, Error::Protocol(message) if message.contains("token_endpoint")),
            "got {error:?}"
        );
    }

    #[test]
    fn an_endpoint_with_a_scheme_a_browser_would_run_is_refused() {
        assert!(matches!(
            document(
                r#"{"issuer": "https://jmap.example.com",
                    "authorization_endpoint": "javascript://x/alert(1)"}"#
            ),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn fields_this_client_has_no_use_for_are_parsed_past() {
        let discovered = document(
            r#"{"issuer": "https://jmap.example.com",
                "authorization_endpoint": "https://jmap.example.com/a",
                "token_endpoint": "https://jmap.example.com/t",
                "grant_types_supported": ["authorization_code"],
                "op_policy_uri": "https://jmap.example.com/policy",
                "ui_locales_supported": ["en-GB"]}"#,
        )
        .expect("unknown fields are not an error");
        assert!(discovered.supports_authorization_code());
    }

    #[test]
    fn a_server_without_the_authorization_code_grant_is_not_one_eds_can_drive() {
        let discovered = document(
            r#"{"issuer": "https://jmap.example.com",
                "authorization_endpoint": "https://jmap.example.com/a",
                "token_endpoint": "https://jmap.example.com/t",
                "grant_types_supported": ["urn:ietf:params:oauth:grant-type:device_code"]}"#,
        )
        .expect("a valid document");
        assert!(!discovered.supports_authorization_code());
    }
}
