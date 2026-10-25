// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! OAuth 2.0 endpoint discovery and dynamic client registration.
//!
//! Implements RFC 8414 authorization server metadata discovery and RFC 7591 dynamic client registration.
//!
//! Endpoints are discovered using well-known URI paths.
//! The metadata provides authorization endpoints, token endpoints, supported grant types, and PKCE methods.
//!
//! This module provides [`discover`] to fetch metadata documents.
//! It also provides [`register_client`], [`exchange_code`], and [`refresh_access_token`].
//!
//! ## Validation rules
//!
//! RFC 8414 Section 3.3 requires that the document `issuer` matches the requested issuer URI exactly.
//! The client enforces this check to prevent IdP mix-up attacks.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::limits;
use crate::transport::{CancelFlag, HttpMethod, HttpRequest, Transport, TransportError};
use crate::url::encode_template_value;

/// The path RFC 8414 §3 reserves for the metadata document.
const WELL_KNOWN: &str = "/.well-known/oauth-authorization-server";

/// What RFC 8414 §2 says `grant_types_supported` means when it is absent.
const DEFAULT_GRANT_TYPES: [&str; 2] = ["authorization_code", "implicit"];

/// Authorization server metadata (RFC 8414 Section 2).
///
/// Parsed instances have validated issuer URIs and absolute endpoint URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationServer {
    /// The issuer identifier, equal to the (normalised) one discovery asked
    /// for: see this module's note on RFC 8414 §3.3.
    pub issuer: String,
    /// Where the user is sent to consent. `None` only for a server that does
    /// no grant type using it, which is not one a JMAP account can use.
    pub authorization_endpoint: Option<String>,
    /// Where an authorization code is exchanged for a token, and where a
    /// refresh token is redeemed.
    pub token_endpoint: Option<String>,
    /// RFC 7591 dynamic client registration, where the server offers it.
    pub registration_endpoint: Option<String>,
    /// RFC 8628 device authorization, for a flow that needs no browser in the
    /// client.
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
    /// `issuer` is the identifier the document was fetched *for*, normally
    /// what [`discover`] was given. It is normalised the same way
    /// [`metadata_url`] normalises it.
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

    /// Whether this server does the authorization-code flow: the one an
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

/// Fetch and validate an authorization server metadata document (RFC 8414).
///
/// Requests are unauthenticated because metadata endpoints are public (RFC 8414 Section 3).
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
        // OAuth discovery endpoints do not return JMAP problem details.
        return Err(Error::Http {
            status: response.status,
            problem: None,
        });
    }

    AuthorizationServer::parse(&issuer, &response.body)
}

/// Grant types requested during dynamic client registration.
const CLIENT_GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// The response types this client asks to be registered for: the
/// authorization-code grant only; this client does no implicit flow.
const CLIENT_RESPONSE_TYPES: [&str; 1] = ["code"];

/// Client authentication method for public clients using PKCE (RFC 8252 Section 8.4, RFC 7636).
const CLIENT_AUTH_METHOD: &str = "none";

/// What this client asks an RFC 7591 registration endpoint to register it
/// as. `client_name`, `redirect_uris` and `scope` are the fields that vary
/// between deployments.
#[derive(Debug, Clone)]
pub struct ClientRegistrationRequest<'a> {
    /// Shown to the user on the consent page a real IdP renders (RFC 7591
    /// §2 `client_name`).
    pub client_name: &'a str,
    /// Where the authorization code is delivered back to: RFC 7591 §2's
    /// `redirect_uris`. At least one is required by RFC 6749 §3.1.2.
    pub redirect_uris: &'a [&'a str],
    /// RFC 7591 §2's `scope`: a space-delimited list that becomes this
    /// client's *default* granted scope for an authorization request that
    /// itself names none (RFC 6749 §3.3).
    pub scope: Option<&'a str>,
}

/// A client registered with a deployment, as RFC 7591 §3.2.1 hands one back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    /// RFC 7591 §3.2.1's one required response field.
    pub client_id: String,
    /// Present if the server issued one despite this client asking to
    /// register as public (see `CLIENT_AUTH_METHOD`).
    pub client_secret: Option<String>,
    /// RFC 7591 §3.2.1: required if `client_secret` was issued, `0` meaning
    /// it never expires. Left as the server stated it, including absent,
    /// rather than defaulted: a missing expiry on an issued secret is a
    /// malformed response.
    pub client_secret_expires_at: Option<u64>,
}

/// The registration request body, RFC 7591 §2's fields this client sends and
/// no others. `scope` is omitted entirely, not sent empty, when the caller
/// named none: an absent field is what asks the server for its own default.
#[derive(Serialize)]
struct RegistrationBody<'a> {
    client_name: &'a str,
    redirect_uris: &'a [&'a str],
    grant_types: &'a [&'a str],
    response_types: &'a [&'a str],
    token_endpoint_auth_method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
}

/// The response as it arrives, before it has been checked for a `client_id`.
#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    client_id: Option<String>,
    client_secret: Option<String>,
    client_secret_expires_at: Option<u64>,
}

impl ClientRegistration {
    /// Parse and validate a registration response.
    fn parse(body: &[u8]) -> Result<Self, Error> {
        let response: RegistrationResponse = serde_json::from_slice(body)?;
        let client_id = response
            .client_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                Error::Protocol(
                    "the registration response states no client_id; RFC 7591 §3.2.1 requires one"
                        .to_owned(),
                )
            })?;
        Ok(Self {
            client_id,
            client_secret: response.client_secret,
            client_secret_expires_at: response.client_secret_expires_at,
        })
    }
}

/// Register this client with a deployment's RFC 7591 endpoint, discovered as
/// [`AuthorizationServer::registration_endpoint`].
pub fn register_client(
    transport: &dyn Transport,
    registration_endpoint: &str,
    request: &ClientRegistrationRequest<'_>,
    cancel: Option<&CancelFlag>,
) -> Result<ClientRegistration, Error> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(Error::Cancelled);
    }

    let body = serde_json::to_vec(&RegistrationBody {
        client_name: request.client_name,
        redirect_uris: request.redirect_uris,
        grant_types: &CLIENT_GRANT_TYPES,
        response_types: &CLIENT_RESPONSE_TYPES,
        token_endpoint_auth_method: CLIENT_AUTH_METHOD,
        scope: request.scope,
    })?;

    let headers = [
        ("Accept".to_owned(), "application/json".to_owned()),
        ("Content-Type".to_owned(), "application/json".to_owned()),
    ];
    let response = transport
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: registration_endpoint,
            headers: &headers,
            body: Some(&body),
            cancel,
            max_response_bytes: limits::MAX_OAUTH_REGISTRATION_BYTES,
        })
        .map_err(|error| match error {
            TransportError::Cancelled => Error::Cancelled,
            TransportError::Failed(message) => Error::Transport(message),
            TransportError::ResponseTooLarge { limit } => Error::ResponseTooLarge { limit },
        })?;

    if !(200..300).contains(&response.status) {
        // RFC 7591 §3.2.2 has its own error object (`error`/
        // `error_description`) rather than RFC 7807 problem details, a
        // different shape than the JMAP API answers with.
        return Err(Error::Http {
            status: response.status,
            problem: None,
        });
    }

    ClientRegistration::parse(&response.body)
}

/// Proof Key for Code Exchange verifier (RFC 7636).
///
/// Generates a secret for an authorization attempt.
/// The SHA-256 challenge hash is sent with the authorization request.
/// The raw verifier secret is sent to the token endpoint during code exchange.
/// This protects against authorization code interception attacks (RFC 8252 Section 8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// A fresh verifier: 32 octets from the OS random source, base64url
    /// (no padding) encoded: 43 characters, within RFC 7636 §4.1's required
    /// 43-to-128 characters.
    pub fn generate() -> Self {
        let mut octets = [0u8; 32];
        getrandom::fill(&mut octets).expect("the operating system's random source is unavailable");
        Self(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(octets))
    }

    /// The verifier itself: sent to the token endpoint alongside the code it
    /// challenged ([`exchange_code`]), and nowhere else.
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// The S256 challenge (RFC 7636 §4.2) to send in the authorization
    /// request: `BASE64URL-ENCODE(SHA256(verifier))`. This client only ever
    /// offers S256: `plain` exists in the RFC for a client that cannot hash.
    pub fn challenge(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// A successful token response (RFC 6749 §5.1): the fields an
/// `EOAuth2Service` needs to hand back to EDS's credential store, and no
/// others.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenResponse {
    /// RFC 6749 §5.1's one required field.
    pub access_token: String,
    /// RFC 6749 §7.1's token type, checked rather than merely recorded: see
    /// the module-level note on why a non-`bearer` value is refused.
    pub token_type: String,
    /// Seconds until `access_token` expires, if the server states one.
    pub expires_in: Option<u64>,
    /// A new refresh token, present only if the server issued one (RFC 6749 §6).
    pub refresh_token: Option<String>,
    /// The scope actually granted, if the server states it (RFC 6749 §5.1:
    /// required only if it differs from what was requested).
    pub scope: Option<String>,
}

/// Shows no token. For the refresh token, it shows only if one is present.
impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &format_args!("<redacted>"))
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("scope", &self.scope)
            .finish()
    }
}

/// The success document as it arrives, before it has been checked for the
/// fields this client cannot proceed without. It has no `Debug`, because it
/// holds the tokens.
#[derive(Deserialize)]
struct TokenDocument {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

/// RFC 6749 §5.2's error object: the one thing this client parses out of a
/// token-endpoint failure, and the reason [`Error::OAuthTokenRefused`] exists.
#[derive(Debug, Deserialize)]
struct TokenErrorDocument {
    error: String,
    error_description: Option<String>,
}

/// Exchange an authorization code for access and refresh tokens (RFC 6749 Section 4.1.3).
///
/// Proves possession of the PKCE verifier (RFC 7636 Section 4.5).
/// `redirect_uri` must match the value sent in the authorization request.
pub fn exchange_code(
    transport: &dyn Transport,
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &PkceVerifier,
    cancel: Option<&CancelFlag>,
) -> Result<TokenResponse, Error> {
    let body = form_body(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier.secret()),
    ]);
    token_request(transport, token_endpoint, body, cancel)
}

/// Redeem a refresh token for a new access token (RFC 6749 Section 6).
pub fn refresh_access_token(
    transport: &dyn Transport,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
    cancel: Option<&CancelFlag>,
) -> Result<TokenResponse, Error> {
    let body = form_body(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ]);
    token_request(transport, token_endpoint, body, cancel)
}

/// Send a token request and parse the response (RFC 6749 Section 5).
///
/// Public clients authenticate using `client_id` in the request body.
fn token_request(
    transport: &dyn Transport,
    token_endpoint: &str,
    body: String,
    cancel: Option<&CancelFlag>,
) -> Result<TokenResponse, Error> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(Error::Cancelled);
    }

    let headers = [
        ("Accept".to_owned(), "application/json".to_owned()),
        (
            "Content-Type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        ),
    ];
    let response = transport
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: token_endpoint,
            headers: &headers,
            body: Some(body.as_bytes()),
            cancel,
            max_response_bytes: limits::MAX_OAUTH_TOKEN_BYTES,
        })
        .map_err(|error| match error {
            TransportError::Cancelled => Error::Cancelled,
            TransportError::Failed(message) => Error::Transport(message),
            TransportError::ResponseTooLarge { limit } => Error::ResponseTooLarge { limit },
        })?;

    parse_token_response(response.status, &response.body)
}

/// Parse token endpoint response bodies (RFC 6749 Section 5.1 and 5.2).
fn parse_token_response(status: u16, body: &[u8]) -> Result<TokenResponse, Error> {
    if (200..300).contains(&status) {
        let document: TokenDocument = serde_json::from_slice(body)?;
        let access_token = document
            .access_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                Error::Protocol(
                    "the token response states no access_token; RFC 6749 §5.1 requires one"
                        .to_owned(),
                )
            })?;
        let token_type = document.token_type.ok_or_else(|| {
            Error::Protocol(
                "the token response states no token_type; RFC 6749 §5.1 requires one".to_owned(),
            )
        })?;
        // Verify that the token type is bearer.
        if !token_type.eq_ignore_ascii_case("bearer") {
            return Err(Error::Protocol(format!(
                "the token response names token_type {token_type}, which this client cannot use \
                 (only bearer tokens are supported end to end)"
            )));
        }
        Ok(TokenResponse {
            access_token,
            token_type,
            expires_in: document.expires_in,
            refresh_token: document.refresh_token,
            scope: document.scope,
        })
    } else {
        match serde_json::from_slice::<TokenErrorDocument>(body) {
            Ok(error) => Err(Error::OAuthTokenRefused {
                error: error.error,
                description: error.error_description,
            }),
            Err(_) => Err(Error::Http {
                status,
                problem: None,
            }),
        }
    }
}

/// Build an `application/x-www-form-urlencoded` body (RFC 6749 §4.1.3's
/// request shape).
///
/// Reuses [`encode_template_value`]'s RFC 3986 unreserved-set percent-encoding:
/// escapes a space as `%20` where historical forms prefer `+`, but
/// percent-decoders read both identically.
fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", encode_template_value(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Return the metadata document URL for an issuer (RFC 8414 Section 3.1).
///
/// Inserts the well-known path between host and path components.
pub fn metadata_url(issuer: &str) -> Result<String, Error> {
    well_known_for(issuer).map(|(_, url)| url)
}

/// Derive normalized issuer identifier and well-known metadata URL.
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

/// Validate that an endpoint URL is absolute and uses `http://` or `https://`.
fn endpoint(name: &str, value: Option<String>) -> Result<Option<String>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if crate::url::split_http_scheme(&value).is_none() {
        return Err(Error::Protocol(format!(
            "{name} is {value}, which is not an absolute http(s) URL"
        )));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    use super::{
        AuthorizationServer, ClientRegistration, PkceVerifier, metadata_url, parse_token_response,
    };
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

    fn registration(body: &str) -> Result<ClientRegistration, Error> {
        ClientRegistration::parse(body.as_bytes())
    }

    #[test]
    fn a_registration_response_with_no_client_id_is_refused() {
        assert!(matches!(
            registration(r#"{"client_secret": "s3cret"}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_registration_response_with_an_empty_client_id_is_refused() {
        assert!(matches!(
            registration(r#"{"client_id": ""}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_public_client_registration_needs_no_secret() {
        let registered = registration(r#"{"client_id": "abc123"}"#).expect("a valid response");
        assert_eq!(registered.client_id, "abc123");
        assert_eq!(registered.client_secret, None);
        assert_eq!(registered.client_secret_expires_at, None);
    }

    #[test]
    fn a_server_that_issues_a_secret_anyway_is_read_but_not_required() {
        let registered = registration(
            r#"{"client_id": "abc123", "client_secret": "s3cret",
                "client_secret_expires_at": 0}"#,
        )
        .expect("a valid response");
        assert_eq!(registered.client_secret.as_deref(), Some("s3cret"));
        assert_eq!(registered.client_secret_expires_at, Some(0));
    }

    #[test]
    fn fields_a_registration_response_carries_that_this_client_has_no_use_for_are_parsed_past() {
        let registered = registration(
            r#"{"client_id": "abc123", "client_id_issued_at": 1700000000,
                "redirect_uris": ["https://client.example.org/cb"],
                "grant_types": ["authorization_code", "refresh_token"]}"#,
        )
        .expect("unknown/echoed fields are not an error");
        assert_eq!(registered.client_id, "abc123");
    }

    #[test]
    fn a_pkce_verifier_meets_rfc_7636s_length_and_alphabet() {
        let verifier = PkceVerifier::generate();
        let secret = verifier.secret();
        assert!(
            (43..=128).contains(&secret.len()),
            "RFC 7636 §4.1 requires 43 to 128 characters, got {}",
            secret.len()
        );
        assert!(
            secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "a base64url verifier must be drawn from RFC 7636 §4.1's unreserved set: {secret}"
        );
    }

    #[test]
    fn two_verifiers_are_never_the_same() {
        // Verify that verifier generation produces distinct secrets.
        assert_ne!(
            PkceVerifier::generate().secret(),
            PkceVerifier::generate().secret()
        );
    }

    #[test]
    fn the_challenge_is_the_verifiers_sha256_base64url_encoded() {
        let verifier = PkceVerifier::generate();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.secret().as_bytes()));
        assert_eq!(verifier.challenge(), expected);
    }

    #[test]
    fn a_token_response_with_no_access_token_is_refused() {
        assert!(matches!(
            parse_token_response(200, br#"{"token_type": "bearer"}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_token_response_with_an_empty_access_token_is_refused() {
        assert!(matches!(
            parse_token_response(200, br#"{"access_token": "", "token_type": "bearer"}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_token_response_naming_no_token_type_is_refused() {
        assert!(matches!(
            parse_token_response(200, br#"{"access_token": "tok"}"#),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_non_bearer_token_type_is_refused() {
        let error = parse_token_response(200, br#"{"access_token": "tok", "token_type": "mac"}"#)
            .expect_err("this client cannot use a mac token");
        assert!(
            matches!(&error, Error::Protocol(message) if message.contains("mac")),
            "got {error:?}"
        );
    }

    #[test]
    fn the_bearer_token_type_check_is_case_insensitive() {
        let response =
            parse_token_response(200, br#"{"access_token": "tok", "token_type": "Bearer"}"#)
                .expect("RFC 6749 §7.1: token_type is case-insensitive");
        assert_eq!(response.access_token, "tok");
    }

    #[test]
    fn a_successful_response_carries_the_optional_fields_it_states() {
        let response = parse_token_response(
            200,
            br#"{"access_token": "tok", "token_type": "bearer", "expires_in": 3600,
                "refresh_token": "rtok", "scope": "urn:ietf:params:jmap:mail"}"#,
        )
        .expect("a valid response");
        assert_eq!(response.access_token, "tok");
        assert_eq!(response.expires_in, Some(3600));
        assert_eq!(response.refresh_token.as_deref(), Some("rtok"));
        assert_eq!(response.scope.as_deref(), Some("urn:ietf:params:jmap:mail"));
    }

    #[test]
    fn fields_a_token_response_carries_that_this_client_has_no_use_for_are_parsed_past() {
        let response = parse_token_response(
            200,
            br#"{"access_token": "tok", "token_type": "bearer", "id_token": "eyJ..."}"#,
        )
        .expect("unknown fields are not an error");
        assert_eq!(response.access_token, "tok");
    }

    #[test]
    fn a_refused_grant_surfaces_its_own_error_code() {
        let error = parse_token_response(
            400,
            br#"{"error": "invalid_grant", "error_description": "refresh token expired"}"#,
        )
        .expect_err("the server refused the grant");
        assert!(
            matches!(&error, Error::OAuthTokenRefused { error, description }
                if error == "invalid_grant"
                    && description.as_deref() == Some("refresh token expired")),
            "got {error:?}"
        );
    }

    #[test]
    fn distinct_error_codes_are_distinguishable() {
        // Distinct token error codes must produce distinct error variants.
        let invalid_grant =
            parse_token_response(400, br#"{"error": "invalid_grant"}"#).expect_err("refused");
        let invalid_client =
            parse_token_response(400, br#"{"error": "invalid_client"}"#).expect_err("refused");
        assert!(matches!(
            invalid_grant,
            Error::OAuthTokenRefused { error, .. } if error == "invalid_grant"
        ));
        assert!(matches!(
            invalid_client,
            Error::OAuthTokenRefused { error, .. } if error == "invalid_client"
        ));
    }

    #[test]
    fn an_unparseable_error_body_falls_back_to_the_http_status() {
        let error =
            parse_token_response(500, b"internal server error").expect_err("a failed request");
        assert!(
            matches!(
                error,
                Error::Http {
                    status: 500,
                    problem: None
                }
            ),
            "got {error:?}"
        );
    }
}
