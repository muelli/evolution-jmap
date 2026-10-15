// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Client error type.

use jmap_proto::error::{MethodError, RequestError, SetError};

#[derive(Debug)]
pub enum Error {
    /// Network/transport failure (DNS, connect, TLS, I/O).
    Transport(String),
    /// Non-success HTTP status; carries the parsed problem details when the
    /// server sent any (RFC 8620 §3.6.1).
    Http {
        status: u16,
        problem: Option<RequestError>,
    },
    /// A method-level `error` response (RFC 8620 §3.6.2).
    Method(MethodError),
    /// A per-record `/set` failure (RFC 8620 §5.3), e.g. a rejected create.
    Set(SetError),
    /// Response could not be (de)serialized.
    Json(serde_json::Error),
    /// Structurally valid JSON that violates the protocol (e.g. missing
    /// response for a call id).
    Protocol(String),
    /// The operation was cancelled via [`crate::transport::CancelFlag`].
    Cancelled,
    /// More octets than the session's `maxSizeUpload` (RFC 8620 §6.1): the
    /// one failure here the server was never asked about.
    ///
    /// Its own variant rather than a `Protocol` string because it is the one
    /// thing a caller can act on: the size and the limit are both numbers a
    /// layer above can put in front of the user, or use to decide to send the
    /// attachment as a link instead. Flattened into prose they would have to be
    /// parsed back out.
    TooLarge { size: u64, limit: u64 },
    /// More octets in one request to `apiUrl` than the session's
    /// `maxSizeRequest` (RFC 8620 §2).
    ///
    /// Distinct from [`Self::TooLarge`] because an upload that is too big
    /// cannot be made smaller by the caller, while an oversized request can
    /// nearly always be split into several requests. This variant is returned
    /// when a single id alone exceeds the limit.
    RequestTooLarge { size: u64, limit: u64 },
    /// The server's answer was longer than the octets that request said it
    /// would take (see [`crate::limits`]), and was abandoned unread rather than
    /// buffered.
    ///
    /// Only the limit is reported because the body was never read to the end.
    /// Distinct from [`Self::Transport`] because retrying will not resolve it
    /// (the same message will be the same length next time).
    ResponseTooLarge { limit: u64 },
    /// An OAuth 2.0 token endpoint refused a grant (RFC 6749 §5.2).
    ///
    /// Its own variant rather than [`Self::Http`] with no problem: every
    /// refusal at a token endpoint answers HTTP 400, so the status carries
    /// none of the reason; only the body's `error` does, and `invalid_grant`
    /// (re-authenticate) is distinct from `invalid_client`.
    OAuthTokenRefused {
        error: String,
        description: Option<String>,
    },
    /// A blob download was redirected to a different origin than the one the
    /// session's own `downloadUrl` named.
    ///
    /// The transport drops `Authorization` on a cross-host redirect
    /// (`UreqTransport::new`'s `redirect_auth_headers`, `SameHost`), RFC
    /// 7235's safe default, but it still follows the redirect. An unrelated
    /// 200 response (e.g. a captive portal or catch-all page) would otherwise
    /// be stored as if it were the message. `download_blob` checks the origin
    /// actually reached against the requested one and refuses a mismatch.
    ///
    /// `rebase_note` is set when `ClientBuilder::rebase_urls_to_origin`
    /// rewritten `downloadUrl`'s origin before this request.
    CrossOriginRedirect {
        requested: String,
        followed: String,
        rebase_note: Option<String>,
    },
    /// A request was redirected to a different origin than requested, credentials
    /// were withheld by the transport to prevent leaking them to an untrusted
    /// origin (RFC 7235), and the target server answered HTTP 401 Unauthorized.
    ///
    /// Distinct from [`Error::Http`] with status 401: a 401 on a redirected request
    /// where credentials were deliberately withheld is a connection or redirection
    /// failure, not a rejected token or stale credentials, so callers must not
    /// escalate to interactive user consent.
    RedirectCredentialsWithheld { requested: String, followed: String },
}

impl Error {
    /// Whether the server refused an incremental sync because the state it was
    /// given is too old, or was never one of its own (RFC 8620 §5.2).
    ///
    /// Not an error: callers list the collection in full and carry on, so the
    /// check is exposed here rather than requiring string matching.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(
            self,
            Self::Method(error)
                if error.error_type == jmap_proto::error::method::CANNOT_CALCULATE_CHANGES
        )
    }

    /// Whether the server rejected the request with HTTP 401.
    ///
    /// Indicates an expired or invalid bearer token on long-lived connections.
    /// The single source of truth that backend callers delegate to.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Http { status: 401, .. })
    }

    /// Whether the request was redirected to a different origin, credentials
    /// were withheld by the transport, and the target server answered HTTP 401.
    pub fn is_redirect_credentials_withheld(&self) -> bool {
        matches!(self, Self::RedirectCredentialsWithheld { .. })
    }

    /// Whether a `/changes` call's `sinceState` was rejected as
    /// `invalidArguments` rather than RFC 8620 §5.2's `cannotCalculateChanges`.
    ///
    /// Some servers answer `invalidArguments` for unrecognized state strings
    /// instead of the RFC's standard error type.
    ///
    /// Its own predicate rather than folded into
    /// [`Self::is_cannot_calculate_changes`]'s meaning: that one answers a
    /// standard condition for any caller, while this one is a server
    /// quirk identified by description text in `/changes` responses.
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
                // The property list is often the only diagnostic a strict
                // server sends: dropping it turns a precise rejection into a mystery.
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
}
