// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutation test coverage suite for jmap-client/src/oauth.rs.
//!
//! Pins OAuth 2.0 metadata URL parsing and derivation (RFC 8414), issuer verification
//! and endpoint protocol scheme checks, authorization server code grant capabilities,
//! PKCE verifier generation and S256 challenge derivation (RFC 7636), dynamic client
//! registration payloads and responses (RFC 7591), token request form encoding and
//! response parsing (RFC 6749), and transport cancellation and error mappings.

use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use jmap_client::Error;
use jmap_client::oauth::{
    AuthorizationServer, ClientRegistrationRequest, PkceVerifier, discover, exchange_code,
    metadata_url, refresh_access_token, register_client,
};
use jmap_client::transport::{
    CancelFlag, HttpMethod, HttpRequest, HttpResponse, Transport, TransportError,
};
use serde_json::json;
use sha2::{Digest, Sha256};

type TransportHandler =
    Box<dyn Fn(HttpRequest<'_>) -> Result<HttpResponse, TransportError> + Send + Sync>;

struct MockTransport {
    handler: TransportHandler,
}

impl MockTransport {
    fn new<F>(f: F) -> Self
    where
        F: Fn(HttpRequest<'_>) -> Result<HttpResponse, TransportError> + Send + Sync + 'static,
    {
        Self {
            handler: Box::new(f),
        }
    }
}

impl Transport for MockTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        (self.handler)(request)
    }
}

fn mock_response(status: u16, body: Vec<u8>, url: &str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: Some("application/json".to_owned()),
        body,
        final_url: url.to_owned(),
    }
}

#[test]
fn metadata_url_normalization_and_schemes() {
    assert_eq!(
        metadata_url("https://jmap.example.com").expect("valid https issuer"),
        "https://jmap.example.com/.well-known/oauth-authorization-server"
    );
    assert_eq!(
        metadata_url("https://jmap.example.com/").expect("valid trailing slash issuer"),
        "https://jmap.example.com/.well-known/oauth-authorization-server"
    );
    assert_eq!(
        metadata_url("https://example.com/tenant1").expect("valid tenant issuer"),
        "https://example.com/.well-known/oauth-authorization-server/tenant1"
    );
    assert_eq!(
        metadata_url("https://example.com/tenant1/").expect("valid tenant trailing slash"),
        "https://example.com/.well-known/oauth-authorization-server/tenant1"
    );
    assert_eq!(
        metadata_url("http://127.0.0.1:8080").expect("valid http port issuer"),
        "http://127.0.0.1:8080/.well-known/oauth-authorization-server"
    );
    assert_eq!(
        metadata_url("http://example.com").expect("valid plain http issuer"),
        "http://example.com/.well-known/oauth-authorization-server"
    );
    assert_eq!(
        metadata_url("HTTPS://EXAMPLE.COM/PATH").expect("uppercase scheme issuer"),
        "HTTPS://EXAMPLE.COM/.well-known/oauth-authorization-server/PATH"
    );

    let derived = metadata_url("https://example.com").unwrap();
    assert!(!derived.is_empty());
    assert_ne!(derived, "xyzzy");
}

#[test]
fn metadata_url_refusal_for_invalid_issuers() {
    assert!(matches!(
        metadata_url("jmap.example.com"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        metadata_url("ftp://example.com"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        metadata_url("javascript://example.com/alert(1)"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        metadata_url("https://example.com/path?query=1"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        metadata_url("https://example.com/path#fragment"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        metadata_url("https://?query=1"),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(metadata_url("https://"), Err(Error::Protocol(_))));
}

#[test]
fn authorization_server_parse_issuer_matching_and_normalization() {
    let valid_doc = json!({
        "issuer": "https://jmap.example.com",
        "authorization_endpoint": "https://jmap.example.com/auth",
        "token_endpoint": "https://jmap.example.com/token",
        "grant_types_supported": ["authorization_code"]
    })
    .to_string();

    let server = AuthorizationServer::parse("https://jmap.example.com", valid_doc.as_bytes())
        .expect("exact issuer match must succeed");
    assert_eq!(server.issuer, "https://jmap.example.com");

    let server_slash =
        AuthorizationServer::parse("https://jmap.example.com/", valid_doc.as_bytes())
            .expect("issuer with trailing slash must normalize and match");
    assert_eq!(server_slash.issuer, "https://jmap.example.com");

    let mismatch_doc = json!({
        "issuer": "https://other.example.com",
        "authorization_endpoint": "https://other.example.com/auth"
    })
    .to_string();
    let err_mismatch =
        AuthorizationServer::parse("https://jmap.example.com", mismatch_doc.as_bytes())
            .expect_err("mismatched issuer must be rejected per RFC 8414 Section 3.3");
    assert!(
        matches!(err_mismatch, Error::Protocol(ref msg) if msg.contains("RFC 8414 §3.3")),
        "expected issuer mismatch error, got {err_mismatch:?}"
    );

    let no_issuer_doc = json!({
        "authorization_endpoint": "https://jmap.example.com/auth"
    })
    .to_string();
    let err_no_issuer =
        AuthorizationServer::parse("https://jmap.example.com", no_issuer_doc.as_bytes())
            .expect_err("document without issuer must be rejected");
    assert!(
        matches!(err_no_issuer, Error::Protocol(ref msg) if msg.contains("states no issuer")),
        "expected no issuer error, got {err_no_issuer:?}"
    );

    let err_invalid_json = AuthorizationServer::parse("https://jmap.example.com", b"{invalid json")
        .expect_err("invalid json must error");
    assert!(
        matches!(err_invalid_json, Error::Json(_)),
        "expected Json error, got {err_invalid_json:?}"
    );

    let err_invalid_url = AuthorizationServer::parse("not-a-url", valid_doc.as_bytes())
        .expect_err("non-url issuer argument must error");
    assert!(
        matches!(err_invalid_url, Error::Protocol(_)),
        "expected Protocol error, got {err_invalid_url:?}"
    );
}

#[test]
fn authorization_server_endpoint_validation_and_schemes() {
    let doc_endpoints = json!({
        "issuer": "https://jmap.example.com",
        "authorization_endpoint": "https://jmap.example.com/auth",
        "token_endpoint": "http://127.0.0.1:8080/token",
        "registration_endpoint": "https://jmap.example.com/register",
        "device_authorization_endpoint": "https://jmap.example.com/device"
    })
    .to_string();

    let server = AuthorizationServer::parse("https://jmap.example.com", doc_endpoints.as_bytes())
        .expect("valid endpoints must parse");
    assert_eq!(
        server.authorization_endpoint.as_deref(),
        Some("https://jmap.example.com/auth")
    );
    assert_ne!(
        server.authorization_endpoint.as_deref(),
        Some("xyzzy"),
        "authorization endpoint must not be dummy string"
    );
    assert_eq!(
        server.token_endpoint.as_deref(),
        Some("http://127.0.0.1:8080/token")
    );
    assert_eq!(
        server.registration_endpoint.as_deref(),
        Some("https://jmap.example.com/register")
    );
    assert_eq!(
        server.device_authorization_endpoint.as_deref(),
        Some("https://jmap.example.com/device")
    );

    let doc_omitted = json!({
        "issuer": "https://jmap.example.com"
    })
    .to_string();
    let server_omitted =
        AuthorizationServer::parse("https://jmap.example.com", doc_omitted.as_bytes())
            .expect("omitted endpoints must parse as None");
    assert_eq!(server_omitted.authorization_endpoint, None);
    assert_eq!(server_omitted.token_endpoint, None);
    assert_eq!(server_omitted.registration_endpoint, None);
    assert_eq!(server_omitted.device_authorization_endpoint, None);

    let doc_relative = json!({
        "issuer": "https://jmap.example.com",
        "authorization_endpoint": "/relative/path"
    })
    .to_string();
    let err_rel = AuthorizationServer::parse("https://jmap.example.com", doc_relative.as_bytes())
        .expect_err("relative endpoint must be rejected");
    assert!(
        matches!(err_rel, Error::Protocol(ref msg) if msg.contains("authorization_endpoint")),
        "expected error naming authorization_endpoint, got {err_rel:?}"
    );

    let doc_js = json!({
        "issuer": "https://jmap.example.com",
        "token_endpoint": "javascript://alert(1)"
    })
    .to_string();
    let err_js = AuthorizationServer::parse("https://jmap.example.com", doc_js.as_bytes())
        .expect_err("javascript endpoint must be rejected");
    assert!(
        matches!(err_js, Error::Protocol(ref msg) if msg.contains("token_endpoint")),
        "expected error naming token_endpoint, got {err_js:?}"
    );

    let doc_data = json!({
        "issuer": "https://jmap.example.com",
        "registration_endpoint": "data:text/plain;base64,SGVsbG8="
    })
    .to_string();
    let err_data = AuthorizationServer::parse("https://jmap.example.com", doc_data.as_bytes())
        .expect_err("data URI endpoint must be rejected");
    assert!(
        matches!(err_data, Error::Protocol(ref msg) if msg.contains("registration_endpoint")),
        "expected error naming registration_endpoint, got {err_data:?}"
    );
}

#[test]
fn authorization_server_supports_authorization_code_predicates() {
    let make_server = |auth: Option<&str>, token: Option<&str>, grants: Option<Vec<&str>>| {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "issuer".to_owned(),
            json!("https://auth.example.com".to_owned()),
        );
        if let Some(a) = auth {
            obj.insert("authorization_endpoint".to_owned(), json!(a));
        }
        if let Some(t) = token {
            obj.insert("token_endpoint".to_owned(), json!(t));
        }
        if let Some(g) = grants {
            obj.insert("grant_types_supported".to_owned(), json!(g));
        }
        let body = serde_json::Value::Object(obj).to_string();
        AuthorizationServer::parse("https://auth.example.com", body.as_bytes())
            .expect("must parse test server")
    };

    let full = make_server(
        Some("https://auth.example.com/a"),
        Some("https://auth.example.com/t"),
        Some(vec!["authorization_code", "refresh_token"]),
    );
    assert!(
        full.supports_authorization_code(),
        "full server must support authorization code"
    );

    let missing_auth = make_server(
        None,
        Some("https://auth.example.com/t"),
        Some(vec!["authorization_code"]),
    );
    assert!(
        !missing_auth.supports_authorization_code(),
        "server without authorization_endpoint must not support authorization code"
    );

    let missing_token = make_server(
        Some("https://auth.example.com/a"),
        None,
        Some(vec!["authorization_code"]),
    );
    assert!(
        !missing_token.supports_authorization_code(),
        "server without token_endpoint must not support authorization code"
    );

    let both_missing = make_server(None, None, Some(vec!["authorization_code"]));
    assert!(
        !both_missing.supports_authorization_code(),
        "server without either endpoint must not support authorization code"
    );

    let wrong_grant = make_server(
        Some("https://auth.example.com/a"),
        Some("https://auth.example.com/t"),
        Some(vec!["urn:ietf:params:oauth:grant-type:device_code"]),
    );
    assert!(
        !wrong_grant.supports_authorization_code(),
        "server without authorization_code grant must not support it"
    );

    let empty_grants = make_server(
        Some("https://auth.example.com/a"),
        Some("https://auth.example.com/t"),
        Some(vec![]),
    );
    assert!(
        !empty_grants.supports_authorization_code(),
        "server with empty grant_types_supported must not support authorization code"
    );

    let default_grants = make_server(
        Some("https://auth.example.com/a"),
        Some("https://auth.example.com/t"),
        None,
    );
    assert!(
        default_grants.supports_authorization_code(),
        "server omitting grant_types_supported defaults to authorization_code and implicit"
    );
}

#[test]
fn pkce_verifier_generation_length_alphabet_and_challenge() {
    let v1 = PkceVerifier::generate();
    let v2 = PkceVerifier::generate();

    assert_ne!(v1, v2);
    assert_ne!(v1.secret(), v2.secret());
    assert_ne!(v1.challenge(), v2.challenge());

    let secret = v1.secret();
    assert!(
        (43..=128).contains(&secret.len()),
        "RFC 7636 Section 4.1 requires 43 to 128 characters, got {}",
        secret.len()
    );
    assert!(!secret.is_empty());
    assert_ne!(secret, "xyzzy");
    assert!(
        secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "verifier secret must use base64url unreserved alphabet: {secret}"
    );

    let challenge = v1.challenge();
    assert!(!challenge.is_empty());
    assert_ne!(challenge, "xyzzy");

    let expected =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes()));
    assert_eq!(
        challenge, expected,
        "challenge must be base64url-encoded SHA-256 of the secret"
    );
}

#[test]
fn client_registration_request_and_parse() {
    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.com/cb"],
        scope: Some("urn:ietf:params:jmap:core"),
    };
    assert_eq!(request.client_name, "Evolution");
    assert_eq!(request.redirect_uris, &["https://client.example.com/cb"]);
    assert_eq!(request.scope, Some("urn:ietf:params:jmap:core"));

    let transport = MockTransport::new(|req| {
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://idp.example.com/register");
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "Accept" && v == "application/json")
        );

        let body_str = std::str::from_utf8(req.body.expect("body required")).unwrap();
        let body: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert_eq!(body["client_name"], "Evolution");
        assert_eq!(
            body["redirect_uris"],
            json!(["https://client.example.com/cb"])
        );
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["scope"], "urn:ietf:params:jmap:core");

        Ok(mock_response(
            201,
            json!({
                "client_id": "registered-client-42",
                "client_secret": "secret-xyz",
                "client_secret_expires_at": 1800000000u64,
                "extra_field": "ignored"
            })
            .to_string()
            .into_bytes(),
            req.url,
        ))
    });

    let reg = register_client(
        &transport,
        "https://idp.example.com/register",
        &request,
        None,
    )
    .expect("registration must succeed");
    assert_eq!(reg.client_id, "registered-client-42");
    assert_eq!(reg.client_secret.as_deref(), Some("secret-xyz"));
    assert_eq!(reg.client_secret_expires_at, Some(1800000000));
}

#[test]
fn client_registration_parse_failure_modes() {
    let transport_no_id = MockTransport::new(|req| {
        Ok(mock_response(
            201,
            json!({ "client_secret": "foo" }).to_string().into_bytes(),
            req.url,
        ))
    });
    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.com/cb"],
        scope: None,
    };
    let err_no_id = register_client(
        &transport_no_id,
        "https://idp.example.com/register",
        &request,
        None,
    )
    .expect_err("registration without client_id must fail");
    assert!(
        matches!(err_no_id, Error::Protocol(ref msg) if msg.contains("client_id")),
        "expected missing client_id error, got {err_no_id:?}"
    );

    let transport_empty_id = MockTransport::new(|req| {
        Ok(mock_response(
            201,
            json!({ "client_id": "" }).to_string().into_bytes(),
            req.url,
        ))
    });
    let err_empty_id = register_client(
        &transport_empty_id,
        "https://idp.example.com/register",
        &request,
        None,
    )
    .expect_err("registration with empty client_id must fail");
    assert!(
        matches!(err_empty_id, Error::Protocol(ref msg) if msg.contains("client_id")),
        "expected empty client_id error, got {err_empty_id:?}"
    );
}

#[test]
fn token_exchange_and_refresh_form_encoding_and_parsing() {
    let verifier = PkceVerifier::generate();
    let secret = verifier.secret().to_owned();

    let transport_exchange = MockTransport::new(move |req| {
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://idp.example.com/token");
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/x-www-form-urlencoded")
        );

        let body_str = std::str::from_utf8(req.body.expect("body required")).unwrap();
        assert!(body_str.contains("grant_type=authorization_code"));
        assert!(body_str.contains("code=auth-code-77"));
        assert!(body_str.contains("redirect_uri=https%3A%2F%2Fclient.example.com%2Fcb"));
        assert!(body_str.contains("client_id=client-xyz"));
        assert!(body_str.contains(&format!("code_verifier={secret}")));

        Ok(mock_response(
            200,
            json!({
                "access_token": "token-abc-123",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-def-456",
                "scope": "urn:ietf:params:jmap:core"
            })
            .to_string()
            .into_bytes(),
            req.url,
        ))
    });

    let token_resp = exchange_code(
        &transport_exchange,
        "https://idp.example.com/token",
        "client-xyz",
        "auth-code-77",
        "https://client.example.com/cb",
        &verifier,
        None,
    )
    .expect("token exchange must succeed");

    assert_eq!(token_resp.access_token, "token-abc-123");
    assert_eq!(token_resp.token_type, "Bearer");
    assert_eq!(token_resp.expires_in, Some(3600));
    assert_eq!(token_resp.refresh_token.as_deref(), Some("refresh-def-456"));
    assert_eq!(
        token_resp.scope.as_deref(),
        Some("urn:ietf:params:jmap:core")
    );

    let transport_refresh = MockTransport::new(|req| {
        assert_eq!(req.method, HttpMethod::Post);
        let body_str = std::str::from_utf8(req.body.expect("body required")).unwrap();
        assert!(body_str.contains("grant_type=refresh_token"));
        assert!(body_str.contains("refresh_token=refresh-def-456"));
        assert!(body_str.contains("client_id=client-xyz"));
        assert!(
            !body_str.contains("code_verifier"),
            "refresh request must not send code_verifier"
        );

        Ok(mock_response(
            200,
            json!({
                "access_token": "new-token-789",
                "token_type": "bearer",
            })
            .to_string()
            .into_bytes(),
            req.url,
        ))
    });

    let refreshed = refresh_access_token(
        &transport_refresh,
        "https://idp.example.com/token",
        "client-xyz",
        "refresh-def-456",
        None,
    )
    .expect("refresh token must succeed");

    assert_eq!(refreshed.access_token, "new-token-789");
    assert_eq!(refreshed.token_type, "bearer");
    assert_eq!(refreshed.expires_in, None);
    assert_eq!(refreshed.refresh_token, None);
}

#[test]
fn token_response_parsing_failure_modes() {
    let make_token_transport = |status: u16, body: serde_json::Value| {
        MockTransport::new(move |req| {
            Ok(mock_response(
                status,
                body.to_string().into_bytes(),
                req.url,
            ))
        })
    };

    let verifier = PkceVerifier::generate();

    let t_no_token = make_token_transport(200, json!({ "token_type": "bearer" }));
    let err_no_token = exchange_code(
        &t_no_token,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("missing access_token must error");
    assert!(
        matches!(err_no_token, Error::Protocol(ref msg) if msg.contains("access_token")),
        "expected missing access_token, got {err_no_token:?}"
    );

    let t_empty_token =
        make_token_transport(200, json!({ "access_token": "", "token_type": "bearer" }));
    let err_empty_token = exchange_code(
        &t_empty_token,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("empty access_token must error");
    assert!(
        matches!(err_empty_token, Error::Protocol(ref msg) if msg.contains("access_token")),
        "expected empty access_token, got {err_empty_token:?}"
    );

    let t_no_type = make_token_transport(200, json!({ "access_token": "tok" }));
    let err_no_type = exchange_code(
        &t_no_type,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("missing token_type must error");
    assert!(
        matches!(err_no_type, Error::Protocol(ref msg) if msg.contains("token_type")),
        "expected missing token_type, got {err_no_type:?}"
    );

    let t_bad_type =
        make_token_transport(200, json!({ "access_token": "tok", "token_type": "mac" }));
    let err_bad_type = exchange_code(
        &t_bad_type,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("non-bearer token_type must error");
    assert!(
        matches!(err_bad_type, Error::Protocol(ref msg) if msg.contains("mac")),
        "expected mac token_type rejection, got {err_bad_type:?}"
    );

    let t_refused = make_token_transport(
        400,
        json!({
            "error": "invalid_grant",
            "error_description": "the authorization code has expired"
        }),
    );
    let err_refused = exchange_code(
        &t_refused,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("error response must map to OAuthTokenRefused");
    assert!(
        matches!(
            err_refused,
            Error::OAuthTokenRefused {
                ref error,
                ref description
            } if error == "invalid_grant" && description.as_deref() == Some("the authorization code has expired")
        ),
        "expected OAuthTokenRefused, got {err_refused:?}"
    );

    let t_server_err =
        MockTransport::new(|req| Ok(mock_response(502, b"Bad Gateway".to_vec(), req.url)));
    let err_server = exchange_code(
        &t_server_err,
        "https://idp.example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        None,
    )
    .expect_err("non-json error response must fall back to Error::Http");
    assert!(
        matches!(
            err_server,
            Error::Http {
                status: 502,
                problem: None
            }
        ),
        "expected Http 502 error, got {err_server:?}"
    );
}

#[test]
fn oauth_transport_cancellation_and_error_mappings() {
    let call_count = AtomicUsize::new(0);
    let transport = MockTransport::new(move |req| {
        call_count.fetch_add(1, Ordering::SeqCst);
        Ok(mock_response(200, vec![], req.url))
    });

    let cancelled_flag = CancelFlag::new();
    cancelled_flag.cancel();

    let err_disc_cancel = discover(&transport, "https://example.com", Some(&cancelled_flag))
        .expect_err("cancelled discover must error");
    assert!(matches!(err_disc_cancel, Error::Cancelled));

    let reg_req = ClientRegistrationRequest {
        client_name: "Test",
        redirect_uris: &["https://cb"],
        scope: None,
    };
    let err_reg_cancel = register_client(
        &transport,
        "https://example.com/reg",
        &reg_req,
        Some(&cancelled_flag),
    )
    .expect_err("cancelled register must error");
    assert!(matches!(err_reg_cancel, Error::Cancelled));

    let verifier = PkceVerifier::generate();
    let err_exch_cancel = exchange_code(
        &transport,
        "https://example.com/token",
        "cid",
        "code",
        "https://cb",
        &verifier,
        Some(&cancelled_flag),
    )
    .expect_err("cancelled exchange must error");
    assert!(matches!(err_exch_cancel, Error::Cancelled));

    let err_ref_cancel = refresh_access_token(
        &transport,
        "https://example.com/token",
        "cid",
        "ref",
        Some(&cancelled_flag),
    )
    .expect_err("cancelled refresh must error");
    assert!(matches!(err_ref_cancel, Error::Cancelled));

    let transport_cancelled = MockTransport::new(|_| Err(TransportError::Cancelled));
    let err_transport_cancel = discover(&transport_cancelled, "https://example.com", None)
        .expect_err("transport cancelled must map to Error::Cancelled");
    assert!(matches!(err_transport_cancel, Error::Cancelled));

    let transport_failed =
        MockTransport::new(|_| Err(TransportError::Failed("connection aborted".to_owned())));
    let err_transport_failed = discover(&transport_failed, "https://example.com", None)
        .expect_err("transport failed must map to Error::Transport");
    assert!(
        matches!(err_transport_failed, Error::Transport(ref msg) if msg == "connection aborted")
    );

    let transport_too_large =
        MockTransport::new(|_| Err(TransportError::ResponseTooLarge { limit: 1048576 }));
    let err_too_large = discover(&transport_too_large, "https://example.com", None)
        .expect_err("transport response too large must map to Error::ResponseTooLarge");
    assert!(matches!(
        err_too_large,
        Error::ResponseTooLarge { limit: 1048576 }
    ));

    let transport_http_404 =
        MockTransport::new(|req| Ok(mock_response(404, b"Not Found".to_vec(), req.url)));
    let err_http = discover(&transport_http_404, "https://example.com", None)
        .expect_err("404 status must map to Error::Http");
    assert!(
        matches!(
            err_http,
            Error::Http {
                status: 404,
                problem: None
            }
        ),
        "expected Http 404 error, got {err_http:?}"
    );

    let err_reg_http = register_client(
        &transport_http_404,
        "https://example.com/reg",
        &reg_req,
        None,
    )
    .expect_err("404 status on register must map to Error::Http");
    assert!(
        matches!(
            err_reg_http,
            Error::Http {
                status: 404,
                problem: None
            }
        ),
        "expected Http 404 error on register, got {err_reg_http:?}"
    );
}
