// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Client error type.

use jmap_proto::error::{MethodError, RequestError, SetError};

#[derive(Debug)]
pub enum Error {
    /// Network/transport failure (DNS, connect, TLS, I/O).
    Transport(String),
    /// Non-success HTTP status.
    ///
    /// Includes parsed problem details when provided (RFC 8620 Section 3.6.1).
    Http {
        status: u16,
        problem: Option<RequestError>,
    },
    /// Method-level error response (RFC 8620 Section 3.6.2).
    Method(MethodError),
    /// Per-record `/set` failure (RFC 8620 Section 5.3), such as a rejected create.
    Set(SetError),
    /// Serialization or deserialization error.
    Json(serde_json::Error),
    /// Valid JSON that violates the protocol, such as a missing response for a call ID.
    Protocol(String),
    /// The operation was cancelled via [`crate::transport::CancelFlag`].
    Cancelled,
    /// Payload exceeds `maxSizeUpload` (RFC 8620 Section 6.1).
    ///
    /// The client detects this limit locally without querying the server.
    /// Callers can inspect the size and limit values to show progress or link the file.
    TooLarge { size: u64, limit: u64 },
    /// Request size exceeds `maxSizeRequest` (RFC 8620 Section 2).
    ///
    /// Unlike blob uploads, callers can usually split oversized requests into batches.
    /// Returns this error when a single ID alone exceeds the limit.
    RequestTooLarge { size: u64, limit: u64 },
    /// Response exceeds the expected byte limit (see [`crate::limits`]).
    ///
    /// The client aborts reading rather than buffering excessive data.
    /// Only the limit is known because the full body was not read.
    ResponseTooLarge { limit: u64 },
    /// OAuth 2.0 token endpoint refused a grant (RFC 6749 Section 5.2).
    ///
    /// Token endpoints return HTTP 400 for multiple distinct failure conditions.
    /// The response error code distinguishes cases such as `invalid_grant` from `invalid_client`.
    OAuthTokenRefused {
        error: String,
        description: Option<String>,
    },
    /// Blob download redirected to an unexpected origin.
    ///
    /// The transport drops credentials on cross-host redirects per RFC 7235.
    /// The client rejects redirected responses to prevent accepting untrusted content.
    ///
    /// `rebase_note` contains origin details when URL rebasing was active.
    CrossOriginRedirect {
        requested: String,
        followed: String,
        rebase_note: Option<String>,
    },
    /// Cross-origin redirect omitted credentials and received HTTP 401.
    ///
    /// The transport withholds credentials on cross-origin redirects per RFC 7235.
    /// This represents a connection routing failure rather than an invalid token.
    /// Callers must not trigger re-authentication prompts for this error.
    RedirectCredentialsWithheld { requested: String, followed: String },
}

impl Error {
    /// Return true if the server refused incremental sync due to state mismatch (RFC 8620 Section 5.2).
    ///
    /// Callers fall back to a full sync when this occurs.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(
            self,
            Self::Method(error)
                if error.error_type == jmap_proto::error::method::CANNOT_CALCULATE_CHANGES
        )
    }

    /// Return true if the server rejected the request with HTTP 401.
    ///
    /// Indicates an expired or invalid bearer token.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Http { status: 401, .. })
    }

    /// Return true if credentials were withheld during cross-origin redirect and resulted in HTTP 401.
    pub fn is_redirect_credentials_withheld(&self) -> bool {
        matches!(self, Self::RedirectCredentialsWithheld { .. })
    }

    /// Return true if `/changes` returned `invalidArguments` for an unknown state.
    ///
    /// Some non-conforming servers return `invalidArguments` instead of `cannotCalculateChanges` (RFC 8620 Section 5.2).
    /// This method inspects the error description to identify this behavior.
    pub fn is_unrecognized_changes_state(&self) -> bool {
        matches!(
            self,
            Self::Method(error)
                if error.error_type == jmap_proto::error::method::INVALID_ARGUMENTS
                    && error
                        .description
                        .as_deref()
                        .is_some_and(|description| description.to_lowercase().contains("state"))
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(message) => write!(f, "transport error: {message}"),
            Error::Http { status, problem } => match problem {
                Some(problem) => write!(
                    f,
                    "HTTP {status}: {} ({})",
                    problem.error_type,
                    problem.detail.as_deref().unwrap_or("no detail")
                ),
                None => write!(f, "HTTP {status}"),
            },
            Error::Method(error) => write!(
                f,
                "method error: {} ({})",
                error.error_type,
                error.description.as_deref().unwrap_or("no description")
            ),
            Error::Set(error) => {
                write!(
                    f,
                    "set error: {} ({})",
                    error.error_type,
                    error.description.as_deref().unwrap_or("no description")
                )?;
                // Include invalid properties in error output when present.
                if let Some(properties) = error
                    .properties
                    .as_deref()
                    .filter(|properties| !properties.is_empty())
                {
                    write!(f, " [properties: {}]", properties.join(", "))?;
                }
                Ok(())
            }
            Error::Json(error) => write!(f, "JSON error: {error}"),
            Error::Protocol(message) => write!(f, "protocol error: {message}"),
            Error::Cancelled => f.write_str("operation cancelled"),
            Error::TooLarge { size, limit } => write!(
                f,
                "{size} bytes is larger than the {limit} bytes this account accepts in one upload"
            ),
            Error::RequestTooLarge { size, limit } => write!(
                f,
                "{size} bytes is larger than the {limit} bytes this account accepts in one request"
            ),
            Error::ResponseTooLarge { limit } => write!(
                f,
                "the server's answer is larger than the {limit} bytes this request allowed for it"
            ),
            Error::OAuthTokenRefused { error, description } => write!(
                f,
                "OAuth 2.0 token request refused: {error} ({})",
                description.as_deref().unwrap_or("no description")
            ),
            Error::CrossOriginRedirect {
                requested,
                followed,
                rebase_note,
            } => {
                write!(
                    f,
                    "download redirected from {requested} to a different origin \
                     ({followed}); refusing to treat its answer as the blob"
                )?;
                if let Some(note) = rebase_note {
                    write!(f, " ({note})")?;
                }
                Ok(())
            }
            Error::RedirectCredentialsWithheld {
                requested,
                followed,
            } => write!(
                f,
                "request redirected from {requested} to a different origin \
                 ({followed}); credentials were withheld and the server answered 401"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn a_set_error_display_names_the_offending_properties() {
        let error = Error::Set(jmap_proto::error::SetError {
            error_type: "invalidProperties".into(),
            description: None,
            properties: Some(vec!["start".into(), "duration".into()]),
            extra: Default::default(),
        });
        assert_eq!(
            error.to_string(),
            "set error: invalidProperties (no description) [properties: start, duration]"
        );
    }

    #[test]
    fn a_set_error_without_properties_reads_as_before() {
        let error = Error::Set(jmap_proto::error::SetError {
            error_type: "forbidden".into(),
            description: Some("nope".into()),
            properties: None,
            extra: Default::default(),
        });
        assert_eq!(error.to_string(), "set error: forbidden (nope)");
    }

    #[test]
    fn invalid_arguments_mentioning_state_is_an_unrecognized_changes_state() {
        assert!(
            Error::Method(jmap_proto::error::MethodError {
                error_type: "invalidArguments".into(),
                description: Some("invalid JMAP State at line 1 column 42".into()),
                extra: Default::default(),
            })
            .is_unrecognized_changes_state(),
            "this is the exact shape real Stalwart answers an unrecognized sinceState with"
        );
    }

    #[test]
    fn invalid_arguments_about_something_else_is_not_an_unrecognized_changes_state() {
        assert!(
            !Error::Method(jmap_proto::error::MethodError {
                error_type: "invalidArguments".into(),
                description: Some("accountId is not a valid id".into()),
                extra: Default::default(),
            })
            .is_unrecognized_changes_state(),
            "an invalidArguments about a different argument must not be swallowed"
        );
        assert!(
            !Error::Method(jmap_proto::error::MethodError {
                error_type: "invalidArguments".into(),
                description: None,
                extra: Default::default(),
            })
            .is_unrecognized_changes_state(),
            "no description at all gives nothing to recognize the state-shaped case by"
        );
    }

    #[test]
    fn cannot_calculate_changes_is_not_also_an_unrecognized_changes_state() {
        assert!(
            !Error::Method(jmap_proto::error::MethodError {
                error_type: "cannotCalculateChanges".into(),
                description: None,
                extra: Default::default(),
            })
            .is_unrecognized_changes_state(),
            "the two predicates stay disjoint so a caller can tell which server shape it saw"
        );
    }

    #[test]
    fn only_http_401_is_unauthorized() {
        assert!(
            Error::Http {
                status: 401,
                problem: None,
            }
            .is_unauthorized()
        );
        assert!(
            !Error::Http {
                status: 403,
                problem: None,
            }
            .is_unauthorized()
        );
        assert!(!Error::Transport("connection refused".into()).is_unauthorized());
    }

    #[test]
    fn cannot_calculate_changes_predicate_exactness() {
        let cannot_calc = Error::Method(jmap_proto::error::MethodError {
            error_type: jmap_proto::error::method::CANNOT_CALCULATE_CHANGES.into(),
            description: None,
            extra: Default::default(),
        });
        assert!(cannot_calc.is_cannot_calculate_changes());

        let invalid_args = Error::Method(jmap_proto::error::MethodError {
            error_type: jmap_proto::error::method::INVALID_ARGUMENTS.into(),
            description: None,
            extra: Default::default(),
        });
        assert!(!invalid_args.is_cannot_calculate_changes());

        let http = Error::Http {
            status: 400,
            problem: None,
        };
        assert!(!http.is_cannot_calculate_changes());

        let transport = Error::Transport("failed".into());
        assert!(!transport.is_cannot_calculate_changes());
    }

    #[test]
    fn redirect_credentials_withheld_predicate_exactness() {
        let withheld = Error::RedirectCredentialsWithheld {
            requested: "https://example.com/api".into(),
            followed: "https://other.com/api".into(),
        };
        assert!(withheld.is_redirect_credentials_withheld());

        let http_401 = Error::Http {
            status: 401,
            problem: None,
        };
        assert!(!http_401.is_redirect_credentials_withheld());

        let cross_origin = Error::CrossOriginRedirect {
            requested: "https://example.com/dl".into(),
            followed: "https://other.com/dl".into(),
            rebase_note: None,
        };
        assert!(!cross_origin.is_redirect_credentials_withheld());

        let transport = Error::Transport("failed".into());
        assert!(!transport.is_redirect_credentials_withheld());
    }

    #[test]
    fn error_source_returns_some_only_for_json_variant() {
        use std::error::Error as StdError;

        let json_err: Result<serde_json::Value, _> = serde_json::from_str("{invalid");
        let error = Error::Json(json_err.unwrap_err());
        assert!(error.source().is_some());

        let transport = Error::Transport("failed".into());
        assert!(transport.source().is_none());

        let http = Error::Http {
            status: 500,
            problem: None,
        };
        assert!(http.source().is_none());

        let method = Error::Method(jmap_proto::error::MethodError {
            error_type: "unknown".into(),
            description: None,
            extra: Default::default(),
        });
        assert!(method.source().is_none());
    }
}
