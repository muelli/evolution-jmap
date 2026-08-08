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
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
        }
    }
}

impl From<jmap_client::Error> for SyncError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}
