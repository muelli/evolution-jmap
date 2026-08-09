// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a mail synchronisation step failed.

/// A failure from one of [`crate::MailSync`]'s operations.
///
/// The [`SyncError::Client`] case is kept intact rather than flattened into a
/// string for the same reason as in `jmap-book-sync` and `jmap-cal-sync`: the
/// Camel layer above maps it onto the `CAMEL_SERVICE_ERROR` / `CAMEL_ERROR`
/// codes Evolution branches on, so an authentication failure has to stay
/// distinguishable from a network outage all the way up.
#[derive(Debug)]
pub enum SyncError {
    /// The server said no, could not be reached, or answered with something
    /// JMAP does not allow.
    Client(jmap_client::Error),
    /// A message was asked for by id and the account does not have it.
    ///
    /// Its own variant because it is the one failure here that is not a
    /// failure: a uid in a folder summary is a claim about the last listing,
    /// and another client deleting the message in the meantime is ordinary.
    /// Left as a client error it would be reported as a broken account —
    /// Camel's vocabulary has a code for exactly this, and the layer above can
    /// only reach for it if the distinction survives the crate boundary.
    NoSuchMessage(jmap_proto::Id),
    /// A mailbox was asked to change and the account does not have it.
    ///
    /// The folder-shaped counterpart of [`SyncError::NoSuchMessage`], and its
    /// own variant for the same reason: a folder in Camel's tree is a claim
    /// about the last listing, and another client deleting it in the meantime
    /// is ordinary. Camel has a code for exactly this — the store's
    /// `NO_FOLDER`, not the service's "the account is broken" — and the layer
    /// above can only reach for it if the distinction survives the crate
    /// boundary.
    NoSuchFolder(jmap_proto::Id),
    /// A message was to be sent from an address the account has no identity
    /// for, so there is nothing to submit it through.
    ///
    /// Its own variant, and not a client error, because nothing failed: the
    /// connection is fine and the server was never asked to send anything. It
    /// carries the address rather than an id because that is the part the user
    /// recognises — they chose it in the composer, and it is what they would
    /// have to change.
    NoIdentity(String),
}

impl SyncError {
    /// A protocol violation, phrased as a client error so it maps like one.
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Client(jmap_client::Error::Protocol(message.into()))
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
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::NoSuchMessage(_) | Self::NoSuchFolder(_) | Self::NoIdentity(_) => None,
        }
    }
}

impl From<jmap_client::Error> for SyncError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}
