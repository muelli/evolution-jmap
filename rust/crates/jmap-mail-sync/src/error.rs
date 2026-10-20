// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cause of a failed mail synchronisation step.

/// A failure from one of the [`crate::MailSync`] operations.
///
/// [`SyncError::Client`] keeps the client error intact, as in `jmap-book-sync`
/// and `jmap-cal-sync`. The Camel layer maps it to the `CAMEL_SERVICE_ERROR`
/// or `CAMEL_ERROR` codes of Evolution, so an authentication failure must stay
/// different from a network failure.
#[derive(Debug)]
pub enum SyncError {
    /// The server refused, was not reachable, or sent a reply that JMAP does
    /// not allow.
    Client(jmap_client::Error),
    /// The account does not have the message with this id.
    ///
    /// This is not a broken account: a uid in a folder summary comes from the
    /// last listing, and another client can delete the message after it.
    /// Camel has an error code for this case, and the layer above can use it
    /// only if this variant is different from a client error.
    NoSuchMessage(jmap_proto::Id),
    /// The account does not have the mailbox that a change is for.
    ///
    /// The folder equivalent of [`SyncError::NoSuchMessage`], for the same
    /// reason: another client can delete the mailbox after the last listing.
    /// The layer above maps it to the store code `NO_FOLDER`, not to a service
    /// error for a broken account.
    NoSuchFolder(jmap_proto::Id),
    /// The account has no identity for the sender address of a message, so it
    /// cannot submit the message.
    ///
    /// This is not a client error: the connection is good and the server got
    /// no request. It holds the address, not an id, because the user selected
    /// the address in the composer and must change it.
    NoIdentity(String),
    /// The account has no mailbox for an outgoing message: no Drafts and no
    /// Sent.
    ///
    /// Not a client error, for the reason of [`SyncError::NoIdentity`]. It
    /// holds nothing, because the missing mailbox has no name. RFC 8621 §7
    /// lets a client submit only a message that is already in the account, so
    /// the send cannot start. Refer to
    /// [`OutgoingMailboxes::of`](crate::send::OutgoingMailboxes::of) for why
    /// the Inbox is not used.
    NoOutgoingFolder,
}

impl SyncError {
    /// A protocol violation, phrased as a client error so it maps like one.
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Client(jmap_client::Error::Protocol(message.into()))
    }

    /// Whether the server refused the request with HTTP 401. On a long-lived
    /// connection, this usually means that the bearer token expired while the
    /// connection was idle. Refresh the token and try again once before you
    /// open the consent window.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_unauthorized())
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => write!(f, "{error}"),
            Self::NoSuchMessage(uid) => {
                write!(f, "the account no longer holds the message {uid}")
            }
            Self::NoSuchFolder(id) => {
                write!(f, "the account no longer holds the mailbox {id}")
            }
            Self::NoIdentity(address) => {
                write!(f, "the account cannot send mail as {address}")
            }
            Self::NoOutgoingFolder => {
                f.write_str("the account has no Drafts or Sent folder to send a message from")
            }
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::NoSuchMessage(_)
            | Self::NoSuchFolder(_)
            | Self::NoIdentity(_)
            | Self::NoOutgoingFolder => None,
        }
    }
}

impl From<jmap_client::Error> for SyncError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}

#[cfg(test)]
mod tests {
    use super::SyncError;
    use std::error::Error as _;

    #[test]
    fn only_a_client_error_has_a_source() {
        assert!(
            SyncError::Client(jmap_client::Error::Http {
                status: 401,
                problem: None,
            })
            .source()
            .is_some()
        );
        assert!(SyncError::NoOutgoingFolder.source().is_none());
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
        assert!(!SyncError::NoOutgoingFolder.is_unauthorized());
    }
}
