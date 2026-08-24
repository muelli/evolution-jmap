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
    /// More octets than the session's `maxSizeUpload` (RFC 8620 §6.1) — the
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
    /// Distinct from [`Self::TooLarge`] because the remedy is: an upload that
    /// is too big cannot be made smaller by the caller, so that one is the end
    /// of the road, while a request that is too long can nearly always be sent
    /// as several. This variant is what is left when it cannot — a single id so
    /// long that a call naming only it is still over the limit — and it says
    /// so with both numbers rather than with the server's
    /// `urn:ietf:params:jmap:error:limit`, which is the same string for every
    /// request-level limit there is.
    RequestTooLarge { size: u64, limit: u64 },
    /// The server's answer was longer than the octets that request said it
    /// would take (see [`crate::limits`]), and was abandoned unread rather than
    /// buffered.
    ///
    /// Only the limit, because the size is not known: the body was never read
    /// to the end, which is the point of having a ceiling. Distinct from
    /// [`Self::Transport`] because it is not a failure that retrying mends —
    /// the same message will be the same length next time — and distinct from
    /// the two limits above because those are refusals of something this client
    /// was about to *send*.
    ResponseTooLarge { limit: u64 },
    /// An OAuth 2.0 token endpoint refused a grant (RFC 6749 §5.2).
    ///
    /// Its own variant rather than [`Self::Http`] with no problem: every
    /// refusal at a token endpoint answers the same HTTP 400 (401 only for a
    /// confidential client's bad credentials, which this client never is), so
    /// the status carries none of the reason — only the body's `error` does,
    /// and `invalid_grant` (re-authenticate) is not `invalid_client`
    /// (the registration itself is gone).
    OAuthTokenRefused {
        error: String,
        description: Option<String>,
    },
    /// A blob download was redirected to a different origin than the one the
    /// session's own `downloadUrl` named.
    ///
    /// The transport already drops `Authorization` on a cross-host redirect
    /// (`UreqTransport::new`'s `redirect_auth_headers`, `SameHost`) — RFC
    /// 7235's safe default — but it still follows the redirect and hands back
    /// whatever answered. A JSON response with the wrong shape fails to parse
    /// and surfaces as [`Self::Json`] or [`Self::Protocol`]; a blob is raw
    /// bytes with no shape to be wrong, so an unrelated 200 (a captive
    /// portal, a CDN's catch-all page — Fastmail's own marketing homepage was
    /// the case that found this) would otherwise be stored as if it were the
    /// message. `download_blob` checks the origin actually reached against
    /// the one it requested and refuses to treat a mismatch as data.
    ///
    /// `rebase_note` is set when `ClientBuilder::rebase_urls_to_origin` (the
    /// `JMAP_LIVE_SERVER_REBASE_URLS` opt-in) actually rewrote `downloadUrl`'s
    /// origin before this request: a rebase silently pointed *this* request
    /// somewhere other than the server's own advertised host, so an operator
    /// reading this error needs to know that before chasing it as an
    /// ordinary redirect bug — a leftover, process-global rebase env var
    /// poisoning an unrelated account already cost one full debugging
    /// session by leaving no trace of itself in the error it produced.
    CrossOriginRedirect {
        requested: String,
        followed: String,
        rebase_note: Option<String>,
    },
}

impl Error {
    /// Whether the server refused an incremental sync because the state it was
    /// given is too old, or was never one of its own (RFC 8620 §5.2).
    ///
    /// Not really an error: every caller's answer is to list the collection in
    /// full and carry on, so the question is asked here rather than left for
    /// each of them to string-match.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(
            self,
            Self::Method(error)
                if error.error_type == jmap_proto::error::method::CANNOT_CALCULATE_CHANGES
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
                // The property list is often the ONLY diagnostic a strict
                // server sends (observed: Fastmail's `invalidProperties`
                // with no description) — dropping it turns a precise
                // rejection into a mystery.
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
}
