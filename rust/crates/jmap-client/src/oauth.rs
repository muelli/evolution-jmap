// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a JMAP deployment's OAuth 2.0 endpoints and client id come from
//! (RFC 8414 discovery, RFC 7591 dynamic client registration).
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
//! This module is the part of that which can be built and tested here: fetch
//! and validate the metadata document ([`discover`]), where a deployment
//! offers it, register this client for a `client_id` ([`register_client`]),
//! and redeem an authorization code or a refresh token for an access token
//! ([`exchange_code`], [`refresh_access_token`]) — nothing here is a
//! compiled-in constant, unlike EDS's Google/Outlook/Yahoo services. What is
//! still missing is the consent exchange itself: sending the user to
//! `authorization_endpoint` and getting a code back needs a browser and a
//! real provider, and the vfuncs that wire any of this to EDS need the
//! `EOAuth2Service` interface, which is a later slice.
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

/// The grant types this client asks a registration endpoint to register it
/// for. Fixed rather than a parameter: every account this client drives is
/// the same kind of client, so there is nothing here for a caller to
/// legitimately vary.
const CLIENT_GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// The response types this client asks to be registered for — the
/// authorization-code grant only; this client does no implicit flow.
const CLIENT_RESPONSE_TYPES: [&str; 1] = ["code"];

/// RFC 8252 §8.4: a native application cannot keep a client secret
/// confidential, so it registers as a public client and relies on PKCE
/// (RFC 7636) rather than one. Requested here, not merely hoped for: a
/// server that ignores it and issues a secret anyway is handled too (see
/// [`ClientRegistration::client_secret`]), but the client never behaves as if
/// it can keep one.
const CLIENT_AUTH_METHOD: &str = "none";

/// What this client asks an RFC 7591 registration endpoint to register it
/// as. `client_name` and `redirect_uris` are the only fields that vary
/// between deployments; grant types, response types and the auth method
/// follow from this being one native EDS client talking the
/// authorization-code grant, and are not parameters.
#[derive(Debug, Clone)]
pub struct ClientRegistrationRequest<'a> {
    /// Shown to the user on the consent page a real IdP renders — RFC 7591
    /// §2's `client_name`.
    pub client_name: &'a str,
    /// Where the authorization code is delivered back to — RFC 7591 §2's
    /// `redirect_uris`. At least one is required by RFC 6749 §3.1.2 for the
    /// authorization-code grant this client registers for.
    pub redirect_uris: &'a [&'a str],
}

/// A client registered with a deployment, as RFC 7591 §3.2.1 hands one back.
///
/// Only [`ClientRegistration::parse`] and [`register_client`] construct one,
/// so a value of this type has already been checked for the one thing this
/// client cannot proceed without — a `client_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    /// RFC 7591 §3.2.1's one required response field.
    pub client_id: String,
    /// Present if the server issued one despite this client asking to
    /// register as public (see [`CLIENT_AUTH_METHOD`]) — a server is free to
    /// ignore that request, and a secret this client never sends back is not
    /// a reason to refuse the account.
    pub client_secret: Option<String>,
    /// RFC 7591 §3.2.1: required if `client_secret` was issued, `0` meaning
    /// it never expires. Left as the server stated it, including absent,
    /// rather than defaulted — a missing expiry on an issued secret is a
    /// malformed response, not one this client should guess about.
    pub client_secret_expires_at: Option<u64>,
}

/// The registration request body, RFC 7591 §2's fields this client sends and
/// no others.
#[derive(Serialize)]
struct RegistrationBody<'a> {
    client_name: &'a str,
    redirect_uris: &'a [&'a str],
    grant_types: &'a [&'a str],
    response_types: &'a [&'a str],
    token_endpoint_auth_method: &'a str,
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
///
/// Sent with no credentials, matching [`discover`]: registration is how a
/// client *obtains* an identity with the server, so there is nothing to
/// authenticate with yet.
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
        // `error_description`) rather than RFC 7807 problem details — a
        // different shape than the JMAP API answers with, and not parsed out
        // here for the same reason `discover` does not: the status already
        // says registration was refused.
        return Err(Error::Http {
            status: response.status,
            problem: None,
        });
    }

    ClientRegistration::parse(&response.body)
}

/// RFC 7636 Proof Key for Code Exchange: a secret generated fresh for one
/// authorization attempt, whose SHA-256 hash — not the secret itself — is
/// sent with the authorization request, and whose plaintext is sent only
/// once, later, to the token endpoint. Redeeming the code then proves
/// possession of the same secret that produced the challenge the
/// authorization request carried.
///
/// Without this, an authorization code intercepted in transit — a real risk
/// for a native app's redirect URI, RFC 8252 §8.1 — is redeemable by whoever
/// captured it; PKCE is what makes a stolen code alone insufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// A fresh verifier: 32 octets from the OS random source, base64url
    /// (no padding) encoded — 43 characters, within RFC 7636 §4.1's required
    /// 43-to-128 and drawn from an alphabet that is a subset of the
    /// `unreserved` characters §4.1 requires a verifier be built from.
    pub fn generate() -> Self {
        let mut octets = [0u8; 32];
        getrandom::fill(&mut octets).expect("the operating system's random source is unavailable");
        Self(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(octets))
    }

    /// The verifier itself — sent to the token endpoint alongside the code it
    /// challenged ([`exchange_code`]), and nowhere else.
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// The S256 challenge (RFC 7636 §4.2) to send in the authorization
    /// request: `BASE64URL-ENCODE(SHA256(verifier))`. This client only ever
    /// offers S256 — `plain` exists in the RFC for a client that cannot hash,
    /// which is not this one.
    pub fn challenge(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// A successful token response (RFC 6749 §5.1) — the fields an
/// `EOAuth2Service` needs to hand back to EDS's credential store, and no
/// others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    /// RFC 6749 §5.1's one required field.
    pub access_token: String,
    /// RFC 6749 §7.1's token type, checked rather than merely recorded — see
    /// the module-level note on why a non-`bearer` value is refused.
    pub token_type: String,
    /// Seconds until `access_token` expires, if the server states one.
    pub expires_in: Option<u64>,
    /// A new refresh token, present only if the server issued one — RFC 6749
    /// §6 lets a server rotate it, omit it from a refresh response (meaning
    /// the old one is still valid), or never issue one at all.
    pub refresh_token: Option<String>,
    /// The scope actually granted, if the server states it (RFC 6749 §5.1:
    /// required only if it differs from what was requested).
    pub scope: Option<String>,
}

/// The success document as it arrives, before it has been checked for the
/// fields this client cannot proceed without.
#[derive(Debug, Deserialize)]
struct TokenDocument {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

/// RFC 6749 §5.2's error object — the one thing this client parses out of a
/// token-endpoint failure, and the reason [`Error::OAuthTokenRefused`] exists
/// (see its doc comment for why the status code alone is not enough here).
#[derive(Debug, Deserialize)]
struct TokenErrorDocument {
    error: String,
    error_description: Option<String>,
}

/// Redeem an authorization code for tokens (RFC 6749 §4.1.3), proving with
/// `verifier` that this is the same client the authorization request came
/// from (RFC 7636 §4.5).
///
/// `redirect_uri` must be byte-identical to the one the authorization request
/// named — RFC 6749 §4.1.3 requires the token endpoint to check it, and a
/// mismatch here is the server refusing to trust that this exchange follows
/// the same authorization the user consented to.
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

/// Redeem a refresh token for a new access token (RFC 6749 §6).
///
/// No PKCE verifier here: a verifier proves who redeemed the *original*
/// code, not who is refreshing it, and no RFC ties one to a refresh grant —
/// this request is only as trustworthy as `refresh_token` itself.
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

/// POST a token request and parse whichever of RFC 6749 §5.1/§5.2 comes back.
///
/// Sent with no separate client authentication: this client registers as a
/// public, PKCE-only client (see [`CLIENT_AUTH_METHOD`]), so `client_id` in
/// the body is all the identification RFC 6749 §3.2.1 asks of one.
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

/// Parse whichever of RFC 6749 §5.1 (success) or §5.2 (error) a token-endpoint
/// response turns out to be, from the status and body alone — separate from
/// [`token_request`] so it can be exercised directly against hostile bodies,
/// the same reason [`AuthorizationServer::parse`] is separate from
/// [`discover`].
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
        // Every credential this client carries end to end is a bearer token
        // (`Credentials::Bearer`); a server naming any other type is handing
        // back something this client has no per-request scheme for, and
        // using it as a bearer token anyway would send it the wrong way.
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
/// Reuses [`encode_template_value`]'s RFC 3986 unreserved-set percent-encoding
/// rather than a second encoder: it escapes a space as `%20` where this media
/// type's own history prefers `+`, but a percent-decoder — which any correct
/// parser of it applies before treating `+` as space — reads the two
/// identically, so nothing this client sends is lost or misread.
fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", encode_template_value(value)))
        .collect::<Vec<_>>()
        .join("&")
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
        // Not a proof of randomness, but a regression test against the bug
        // that would defeat PKCE entirely: a verifier that is constant or
        // derived from something predictable.
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
        // The whole reason `OAuthTokenRefused` parses the body: every
        // refusal here answers the same HTTP 400, so two different failures
        // must not collapse into the same error.
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
