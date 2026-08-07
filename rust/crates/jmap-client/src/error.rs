// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Client error type.

use jmap_proto::error::{MethodError, RequestError};

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
    /// Response could not be (de)serialized.
    Json(serde_json::Error),
    /// Structurally valid JSON that violates the protocol (e.g. missing
    /// response for a call id).
    Protocol(String),
    /// The operation was cancelled via [`crate::transport::CancelFlag`].
    Cancelled,
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
            Error::Json(error) => write!(f, "JSON error: {error}"),
            Error::Protocol(message) => write!(f, "protocol error: {message}"),
            Error::Cancelled => f.write_str("operation cancelled"),
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
