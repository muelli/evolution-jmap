// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a synchronisation step failed.

use jmap_ical::ICalError;

/// A failure from one of [`crate::CalSync`]'s operations.
///
/// The [`SyncError::Client`] case is kept intact rather than flattened into a
/// string because the GObject layer maps it onto the `E_CLIENT_ERROR` codes
/// Evolution branches on — an authentication failure has to stay
/// distinguishable from a network outage all the way up.
#[derive(Debug)]
pub enum SyncError {
    /// The server said no, or could not be reached.
    Client(jmap_client::Error),
    /// A component handed to us by Evolution could not be read.
    ICal(ICalError),
    /// No event on the server has this identifier.
    NotFound(String),
    /// A component Evolution handed us was read, and holds a value this
    /// mapping cannot state as JSCalendar — so the save was refused rather
    /// than carried out on an event that is not the one described.
    ///
    /// Only a *create* can end this way. An edit goes out as a PatchObject
    /// naming only what the round trip preserved, so a property that cannot be
    /// sent is simply left out and the server's own value stands; a create has
    /// no value to leave standing, and the choices are an event the user did
    /// not describe or no event at all. Refusing is the one of those the user
    /// finds out about.
    Unsendable(Unsendable),
}

/// What a component said that could not be stated as JSCalendar.
///
/// A reason rather than a sentence, and the difference is the point. The
/// sentence a user reads has to be translated, and gettext is bound on the
/// other side of the FFI — `jmap_backend_core`'s `i18n`, which an EDS-linked
/// crate can reach and the sync layer cannot, its tests having no EDS to link
/// against. So this crate says what happened and `jmap_backend_cal::ops` says
/// it in the user's language, over these values.
///
/// The variants are the two answers worth giving rather than every distinction
/// the code could draw: one of them names something the user can go and change,
/// and the other admits it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsendable {
    /// A recurrence whose end is an instant that could not be restated in the
    /// event's own time zone, because the entry defines that zone nowhere or
    /// defines it in a shape the evaluator will not guess at.
    ///
    /// `until` is the instant as the mapping kept it — normalised, so it is the
    /// digits the user is shown rather than the component's own spelling — and
    /// `zone` is the identifier the event names. Between them they say which
    /// appointment to change and which `VTIMEZONE` a bug report should quote.
    RecurrenceEnd {
        /// The end of the series, as the mapping kept it.
        until: String,
        /// The time zone the event is in.
        zone: String,
    },
    /// A recurrence rule this mapping cannot state, for any other reason.
    ///
    /// Deliberately carries nothing. The mapping knows the rule will not
    /// survive the round trip; which part of it is at fault is a question about
    /// our own narrowing rather than about the user's calendar, and a dialog is
    /// no place to answer it.
    Recurrence,
}

impl std::fmt::Display for Unsendable {
    /// The developer's account of the refusal, in English and untranslated.
    ///
    /// This is what reaches a log, and what a [`SyncError`] renders as. The
    /// user's sentence is the one in `jmap_backend_cal::ops`; keeping the two
    /// apart is what stops a translated string from being the text a bug report
    /// quotes, and stops one phrasing from having to serve both audiences.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecurrenceEnd { until, zone } => write!(
                f,
                "a recurrence ending {until}, which the time zone {zone} is not \
                 defined well enough to convert"
            ),
            Self::Recurrence => write!(f, "a recurrence rule that cannot be stated"),
        }
    }
}

impl SyncError {
    /// Whether the server refused an incremental sync because the state it
    /// was given is too old (RFC 8620 §5.2).
    ///
    /// This is not a real error: the caller's answer is to fall back to
    /// [`crate::CalSync::list_existing`] and let the meta backend diff the
    /// whole calendar, which is why it gets its own predicate rather than
    /// being left for callers to string-match.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_cannot_calculate_changes())
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
            Self::ICal(error) => write!(f, "{error}"),
            Self::NotFound(uid) => write!(f, "no calendar event with identifier {uid}"),
            Self::Unsendable(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::ICal(error) => Some(error),
            Self::NotFound(_) | Self::Unsendable(_) => None,
        }
    }
}

impl From<jmap_client::Error> for SyncError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Client(error)
    }
}

impl From<ICalError> for SyncError {
    fn from(error: ICalError) -> Self {
        Self::ICal(error)
    }
}
