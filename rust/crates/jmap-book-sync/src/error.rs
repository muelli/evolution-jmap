// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a synchronisation step failed.

use jmap_vcard::VCardError;

/// A failure from one of [`crate::BookSync`]'s operations.
///
/// The [`SyncError::Client`] case is kept intact rather than flattened into a
/// string because the GObject layer maps it onto the `E_CLIENT_ERROR` codes
/// Evolution branches on — an authentication failure has to stay
/// distinguishable from a network outage all the way up.
#[derive(Debug)]
pub enum SyncError {
    /// The server said no, or could not be reached.
    Client(jmap_client::Error),
    /// A vCard handed to us by Evolution could not be parsed.
    VCard(VCardError),
    /// No card on the server has this identifier.
    NotFound(String),
}

impl SyncError {
    /// Whether the server refused an incremental sync because the state it
    /// was given is too old (RFC 8620 §5.2).
    ///
    /// This is not a real error: the caller's answer is to fall back to
    /// [`crate::BookSync::list_existing`] and let the meta backend diff the
    /// whole book, which is why it gets its own predicate rather than being
    /// left for callers to string-match.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_cannot_calculate_changes())
    }

    /// Whether the server rejected the request with HTTP 401 — on a
    /// long-lived connection, what a bearer token that expired while the
    /// connection sat idle looks like. The caller's answer is to refresh the
    /// token and retry once, not to reopen the consent window immediately
    /// (`docs/ROADMAP.md` item 23).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_unauthorized())
    }

    /// A protocol violation, phrased as a client error so it maps like one.
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Client(jmap_client::Error::Protocol(message.into()))
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => write!(f, "{error}"),
            Self::VCard(error) => write!(f, "{error}"),
            Self::NotFound(uid) => write!(f, "no contact with identifier {uid}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::VCard(error) => Some(error),
            Self::NotFound(_) => None,
        }
    }
}

impl From<jmap_client::Error> for SyncError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}

impl From<VCardError> for SyncError {
    fn from(error: VCardError) -> Self {
        Self::VCard(error)
    }
}

#[cfg(test)]
mod tests {
    use super::SyncError;

    #[test]
    fn only_a_client_401_is_unauthorized() {
        assert!(
            SyncError::Client(jmap_client::Error::Http {
                status: 401,
                problem: None,
            })
            .is_unauthorized()
        );
        assert!(
            !SyncError::Client(jmap_client::Error::Http {
                status: 403,
                problem: None,
            })
            .is_unauthorized()
        );
        assert!(!SyncError::NotFound("uid-1".into()).is_unauthorized());
    }
}
