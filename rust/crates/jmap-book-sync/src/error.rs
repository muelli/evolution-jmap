// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a synchronisation step failed.

use jmap_vcard::VCardError;

/// A failure from one of [`crate::BookSync`]'s operations.
///
/// [`SyncError::Client`] keeps the client error, not a string. The GObject layer
/// maps it to the `E_CLIENT_ERROR` codes that Evolution examines. Thus, an
/// authentication failure stays different from a network outage.
#[derive(Debug)]
pub enum SyncError {
    /// The server refused the request, or the client could not reach the server.
    Client(jmap_client::Error),
    /// The vCard from Evolution did not parse.
    VCard(VCardError),
    /// No card on the server has this identifier.
    NotFound(String),
    /// The server refused the card as too large, and the inline photo causes the size.
    ///
    /// This is a separate variant, not [`SyncError::Client`]. A `PHOTO` is the
    /// only vCard part that is this large and that the user can change.
    /// `jmap_backend_book::ops` tells the user to choose a smaller picture.
    /// "set error: tooLarge" does not.
    PhotoTooLarge {
        /// The decoded picture size, equal to the size of the user's image file.
        photo_octets: u64,
    },
    /// The server and the user changed the same property after the base copy,
    /// and the caller asked for [`crate::OnConflict::Fail`]. The backend also
    /// uses it for an edited copy whose base the cache no longer keeps. The
    /// value is the card id.
    OutOfSync(String),
}

impl SyncError {
    /// Whether the server refused an incremental sync because the state is too
    /// old (RFC 8620 §5.2).
    /// Real Stalwart answers an unrecognized state with `invalidArguments`, not
    /// `cannotCalculateChanges` (`jmap_client::Error::is_unrecognized_changes_state`).
    ///
    /// This is not a real error. The caller then uses
    /// [`crate::BookSync::list_existing`], and the meta backend compares the full
    /// book. This predicate prevents string matches in callers.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(self, Self::Client(error) if error.needs_full_resync())
    }

    /// Whether the server rejected the request with HTTP 401.
    /// On a long-lived connection, this is usually a bearer token that expired
    /// during idle time. Refresh the token and try again once before you open
    /// the consent window.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_unauthorized())
    }

    /// A protocol violation, as a client error so that it maps as one.
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
            Self::PhotoTooLarge { photo_octets } => write!(
                f,
                "the server refused the contact as too large; its photo is {photo_octets} octets"
            ),
            Self::OutOfSync(uid) => {
                write!(f, "contact {uid} changed on the server after this copy")
            }
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::VCard(error) => Some(error),
            Self::NotFound(_) | Self::PhotoTooLarge { .. } | Self::OutOfSync(_) => None,
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
    fn a_client_invalid_arguments_about_an_unrecognized_state_cannot_calculate_changes_too() {
        assert!(
            SyncError::Client(jmap_client::Error::Method(
                jmap_proto::error::MethodError::new("invalidArguments")
                    .with_description("invalid JMAP State at line 1 column 1")
            ))
            .is_cannot_calculate_changes(),
            "real Stalwart answers an unrecognized sinceState this way, not with cannotCalculateChanges"
        );
    }

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

    #[test]
    fn not_cannot_calculate_changes_for_an_unrelated_failure() {
        assert!(
            !SyncError::Client(jmap_client::Error::Http {
                status: 403,
                problem: None,
            })
            .is_cannot_calculate_changes()
        );
        assert!(!SyncError::NotFound("uid-1".into()).is_cannot_calculate_changes());
    }

    #[test]
    fn sync_error_display_states_the_reason() {
        assert_eq!(
            SyncError::NotFound("uid-1".into()).to_string(),
            "no contact with identifier uid-1"
        );
        assert_eq!(
            SyncError::PhotoTooLarge { photo_octets: 2048 }.to_string(),
            "the server refused the contact as too large; its photo is 2048 octets"
        );
    }

    #[test]
    fn source_is_only_the_wrapped_errors() {
        use std::error::Error as _;

        assert!(
            SyncError::Client(jmap_client::Error::Http {
                status: 500,
                problem: None,
            })
            .source()
            .is_some()
        );
        assert!(
            SyncError::VCard(jmap_vcard::VCardError::NotAVCard)
                .source()
                .is_some()
        );
        assert!(SyncError::NotFound("uid-1".into()).source().is_none());
        assert!(
            SyncError::PhotoTooLarge { photo_octets: 1 }
                .source()
                .is_none()
        );
    }
}
