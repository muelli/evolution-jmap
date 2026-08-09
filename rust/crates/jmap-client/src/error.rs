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
            Error::Set(error) => write!(
                f,
                "set error: {} ({})",
                error.error_type,
                error.description.as_deref().unwrap_or("no description")
            ),
            Error::Json(error) => write!(f, "JSON error: {error}"),
            Error::Protocol(message) => write!(f, "protocol error: {message}"),
            Error::Cancelled => f.write_str("operation cancelled"),
            Error::TooLarge { size, limit } => write!(
                f,
                "{size} bytes is larger than the {limit} bytes this account accepts in one upload"
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
