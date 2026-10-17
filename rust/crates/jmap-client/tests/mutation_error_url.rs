// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage for jmap-client modules:
//! - error.rs (RFC 8620 §3.6.1, §3.6.2, §5.2, §5.3, RFC 6749 §5.2, RFC 7235)
//! - url.rs (RFC 8620 §6.1, §6.2, RFC 3986 §2.1, §2.3)

use std::error::Error as StdError;

use jmap_client::{Error, expand_url};
use jmap_proto::error::{MethodError, RequestError, SetError};

#[test]
fn error_predicates_and_conversions() {
    let cannot_calc = Error::Method(MethodError {
        error_type: jmap_proto::error::method::CANNOT_CALCULATE_CHANGES.into(),
        description: None,
        extra: Default::default(),
    });
    assert!(cannot_calc.is_cannot_calculate_changes());
    assert!(!cannot_calc.is_unauthorized());
    assert!(!cannot_calc.is_redirect_credentials_withheld());
    assert!(!cannot_calc.is_unrecognized_changes_state());

    let unauth = Error::Http {
        status: 401,
        problem: None,
    };
    assert!(unauth.is_unauthorized());
    assert!(!unauth.is_cannot_calculate_changes());
    assert!(!unauth.is_redirect_credentials_withheld());
    assert!(!unauth.is_unrecognized_changes_state());

    let withheld = Error::RedirectCredentialsWithheld {
        requested: "https://example.com/api".into(),
        followed: "https://other.com/api".into(),
    };
    assert!(withheld.is_redirect_credentials_withheld());
    assert!(!withheld.is_unauthorized());
    assert!(!withheld.is_cannot_calculate_changes());
    assert!(!withheld.is_unrecognized_changes_state());

    let invalid_state = Error::Method(MethodError {
        error_type: jmap_proto::error::method::INVALID_ARGUMENTS.into(),
        description: Some("invalid state parameter".into()),
        extra: Default::default(),
    });
    assert!(invalid_state.is_unrecognized_changes_state());
    assert!(!invalid_state.is_cannot_calculate_changes());
    assert!(!invalid_state.is_unauthorized());
    assert!(!invalid_state.is_redirect_credentials_withheld());
}

#[test]
fn error_source_returns_some_only_for_json_variant() {
    let json_err: Result<serde_json::Value, _> = serde_json::from_str("{invalid");
    let error: Error = json_err.unwrap_err().into();
    assert!(error.source().is_some());

    let transport = Error::Transport("failed".into());
    assert!(transport.source().is_none());

    let http = Error::Http {
        status: 500,
        problem: None,
    };
    assert!(http.source().is_none());

    let method = Error::Method(MethodError {
        error_type: "unknown".into(),
        description: None,
        extra: Default::default(),
    });
    assert!(method.source().is_none());

    let set = Error::Set(SetError {
        error_type: "invalidProperties".into(),
        description: None,
        properties: None,
        extra: Default::default(),
    });
    assert!(set.source().is_none());

    let protocol = Error::Protocol("broken".into());
    assert!(protocol.source().is_none());

    let cancelled = Error::Cancelled;
    assert!(cancelled.source().is_none());

    let too_large = Error::TooLarge {
        size: 200,
        limit: 100,
    };
    assert!(too_large.source().is_none());

    let req_too_large = Error::RequestTooLarge {
        size: 200,
        limit: 100,
    };
    assert!(req_too_large.source().is_none());

    let resp_too_large = Error::ResponseTooLarge { limit: 100 };
    assert!(resp_too_large.source().is_none());

    let oauth_refused = Error::OAuthTokenRefused {
        error: "invalid_grant".into(),
        description: None,
    };
    assert!(oauth_refused.source().is_none());

    let cross_origin = Error::CrossOriginRedirect {
        requested: "https://a".into(),
        followed: "https://b".into(),
        rebase_note: None,
    };
    assert!(cross_origin.source().is_none());

    let withheld = Error::RedirectCredentialsWithheld {
        requested: "https://a".into(),
        followed: "https://b".into(),
    };
    assert!(withheld.source().is_none());
}

#[test]
fn error_display_formats_all_variants_comprehensively() {
    assert_eq!(
        Error::Transport("connection reset".into()).to_string(),
        "transport error: connection reset"
    );

    assert_eq!(
        Error::Http {
            status: 404,
            problem: None,
        }
        .to_string(),
        "HTTP 404"
    );

    assert_eq!(
        Error::Http {
            status: 400,
            problem: Some(RequestError {
                error_type: "urn:ietf:params:jmap:error:notJSON".into(),
                status: Some(400),
                detail: Some("syntax error".into()),
                extra: Default::default(),
            }),
        }
        .to_string(),
        "HTTP 400: urn:ietf:params:jmap:error:notJSON (syntax error)"
    );

    assert_eq!(
        Error::Http {
            status: 400,
            problem: Some(RequestError {
                error_type: "urn:ietf:params:jmap:error:notJSON".into(),
                status: Some(400),
                detail: None,
                extra: Default::default(),
            }),
        }
        .to_string(),
        "HTTP 400: urn:ietf:params:jmap:error:notJSON (no detail)"
    );

    assert_eq!(
        Error::Method(MethodError {
            error_type: "unknownMethod".into(),
            description: Some("method not found".into()),
            extra: Default::default(),
        })
        .to_string(),
        "method error: unknownMethod (method not found)"
    );

    assert_eq!(
        Error::Method(MethodError {
            error_type: "unknownMethod".into(),
            description: None,
            extra: Default::default(),
        })
        .to_string(),
        "method error: unknownMethod (no description)"
    );

    assert_eq!(
        Error::Set(SetError {
            error_type: "invalidProperties".into(),
            description: Some("invalid date".into()),
            properties: Some(vec!["start".into(), "duration".into()]),
            extra: Default::default(),
        })
        .to_string(),
        "set error: invalidProperties (invalid date) [properties: start, duration]"
    );

    assert_eq!(
        Error::Set(SetError {
            error_type: "forbidden".into(),
            description: None,
            properties: None,
            extra: Default::default(),
        })
        .to_string(),
        "set error: forbidden (no description)"
    );

    assert_eq!(
        Error::Set(SetError {
            error_type: "notFound".into(),
            description: None,
            properties: Some(vec![]),
            extra: Default::default(),
        })
        .to_string(),
        "set error: notFound (no description)"
    );

    assert_eq!(
        Error::Protocol("missing call id".into()).to_string(),
        "protocol error: missing call id"
    );

    assert_eq!(Error::Cancelled.to_string(), "operation cancelled");

    assert_eq!(
        Error::TooLarge {
            size: 500,
            limit: 100,
        }
        .to_string(),
        "500 bytes is larger than the 100 bytes this account accepts in one upload"
    );

    assert_eq!(
        Error::RequestTooLarge {
            size: 800,
            limit: 200,
        }
        .to_string(),
        "800 bytes is larger than the 200 bytes this account accepts in one request"
    );

    assert_eq!(
        Error::ResponseTooLarge { limit: 1024 }.to_string(),
        "the server's answer is larger than the 1024 bytes this request allowed for it"
    );

    assert_eq!(
        Error::OAuthTokenRefused {
            error: "invalid_client".into(),
            description: Some("client id unknown".into()),
        }
        .to_string(),
        "OAuth 2.0 token request refused: invalid_client (client id unknown)"
    );

    assert_eq!(
        Error::OAuthTokenRefused {
            error: "invalid_grant".into(),
            description: None,
        }
        .to_string(),
        "OAuth 2.0 token request refused: invalid_grant (no description)"
    );

    assert_eq!(
        Error::CrossOriginRedirect {
            requested: "https://example.com/blob".into(),
            followed: "https://other.com/blob".into(),
            rebase_note: None,
        }
        .to_string(),
        "download redirected from https://example.com/blob to a different origin (https://other.com/blob); refusing to treat its answer as the blob"
    );

    assert_eq!(
        Error::CrossOriginRedirect {
            requested: "https://example.com/blob".into(),
            followed: "https://other.com/blob".into(),
            rebase_note: Some("via env".into()),
        }
        .to_string(),
        "download redirected from https://example.com/blob to a different origin (https://other.com/blob); refusing to treat its answer as the blob (via env)"
    );

    assert_eq!(
        Error::RedirectCredentialsWithheld {
            requested: "https://example.com/api".into(),
            followed: "https://other.com/api".into(),
        }
        .to_string(),
        "request redirected from https://example.com/api to a different origin (https://other.com/api); credentials were withheld and the server answered 401"
    );
}

#[test]
fn expand_url_template_variable_substitution_and_escaping() {
    let template = "https://example.com/events/{types}/{closeafter}/{ping}";
    let expanded = expand_url(template, &["Mailbox", "Email"], true, 30);
    assert_eq!(
        expanded,
        "https://example.com/events/Mailbox,Email/state/30"
    );

    let simple_template =
        "https://example.com/events?types={types}&closeafter={closeafter}&ping={ping}";
    let expanded_simple = expand_url(simple_template, &["Mailbox"], false, 60);
    assert_eq!(
        expanded_simple,
        "https://example.com/events?types=Mailbox&closeafter=no&ping=60"
    );

    let bare_url = "https://example.com/events";
    let appended = expand_url(bare_url, &["Mailbox"], false, 60);
    assert_eq!(
        appended,
        "https://example.com/events?types=Mailbox&closeafter=no&ping=60"
    );
}
