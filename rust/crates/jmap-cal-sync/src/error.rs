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
    /// The server was asked to send the change's scheduling messages and
    /// refused, so the change was not applied.
    ///
    /// Its own case rather than the server's error left in
    /// [`SyncError::Client`] because Evolution has been told the server mails
    /// the guests: the user has to learn that nobody was told, and what to
    /// change, in a sentence `jmap_backend_cal::ops` translates.
    Unscheduled(Unscheduled),
}

/// Why the server would not send a change's scheduling messages
/// (`docs/MEETING-SCHEDULING.md`, rule 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unscheduled {
    /// `noSupportedScheduleMethods` (draft-ietf-jmap-calendars section 5.9.2):
    /// some guest has no calendar address the server can send to.
    ///
    /// The server does not say which, so `addresses` are the ones the event
    /// gives that are not `mailto:`, the one transport (iMIP) every server we
    /// know delivers by. Empty when the event was not at hand (a delete) or
    /// every address is a `mailto:` the server still would not take.
    Unreachable {
        /// The event's non-`mailto:` calendar addresses, sorted.
        addresses: Vec<String>,
    },
    /// `invalidArguments` on a call that asked the server to schedule: by
    /// RFC 8620 section 3.9, a server that does not implement
    /// `sendSchedulingMessages`. Only the account's switch can fix this.
    NotImplemented,
}

impl std::fmt::Display for Unscheduled {
    /// The developer's account, in English; see [`Unsendable`]'s for why.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable { addresses } if addresses.is_empty() => {
                f.write_str("the server cannot send scheduling messages to every participant")
            }
            Self::Unreachable { addresses } => write!(
                f,
                "the server cannot send scheduling messages to {}",
                addresses.join(", ")
            ),
            Self::NotImplemented => {
                f.write_str("the server does not implement sendSchedulingMessages")
            }
        }
    }
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
    /// was given is too old (RFC 8620 §5.2) — or, against real Stalwart,
    /// unrecognized altogether, which it answers as `invalidArguments`
    /// rather than the RFC's own `cannotCalculateChanges`
    /// (`jmap_client::Error::is_unrecognized_changes_state`,
    /// `STALWART-RFC-FINDINGS.md` finding 17).
    ///
    /// This is not a real error: the caller's answer is to fall back to
    /// [`crate::CalSync::list_existing`] and let the meta backend diff the
    /// whole calendar, which is why it gets its own predicate rather than
    /// being left for callers to string-match.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(
            self,
            Self::Client(error)
                if error.is_cannot_calculate_changes() || error.is_unrecognized_changes_state()
        )
    }

    /// Whether the server rejected the request with HTTP 401 — on a
    /// long-lived connection, what a bearer token that expired while the
    /// connection sat idle looks like. The caller's answer is to refresh the
    /// token and retry once, not to reopen the consent window immediately.
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
            Self::ICal(error) => write!(f, "{error}"),
            Self::NotFound(uid) => write!(f, "no calendar event with identifier {uid}"),
            Self::Unsendable(reason) => write!(f, "{reason}"),
            Self::Unscheduled(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::ICal(error) => Some(error),
            Self::NotFound(_) | Self::Unsendable(_) | Self::Unscheduled(_) => None,
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

#[cfg(test)]
mod tests {
    use super::SyncError;

    #[test]
    fn a_client_invalid_arguments_about_an_unrecognized_state_cannot_calculate_changes_too() {
        assert!(
            SyncError::Client(jmap_client::Error::Method(jmap_proto::error::MethodError {
                error_type: "invalidArguments".into(),
                description: Some("invalid JMAP State at line 1 column 1".into()),
                extra: Default::default(),
            }))
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
    fn unscheduled_names_the_addresses_it_could_not_reach() {
        assert_eq!(
            super::Unscheduled::Unreachable {
                addresses: vec!["bob@example.com".into(), "carol@example.com".into()],
            }
            .to_string(),
            "the server cannot send scheduling messages to bob@example.com, carol@example.com"
        );
        assert_eq!(
            super::Unscheduled::Unreachable { addresses: vec![] }.to_string(),
            "the server cannot send scheduling messages to every participant"
        );
        assert_eq!(
            super::Unscheduled::NotImplemented.to_string(),
            "the server does not implement sendSchedulingMessages"
        );
    }

    #[test]
    fn unsendable_says_which_recurrence_was_refused() {
        assert_eq!(
            super::Unsendable::RecurrenceEnd {
                until: "2026-09-01T00:00:00".into(),
                zone: "Mars/Olympus_Mons".into(),
            }
            .to_string(),
            "a recurrence ending 2026-09-01T00:00:00, which the time zone \
             Mars/Olympus_Mons is not defined well enough to convert"
        );
        assert_eq!(
            super::Unsendable::Recurrence.to_string(),
            "a recurrence rule that cannot be stated"
        );
    }

    #[test]
    fn sync_error_display_states_the_reason() {
        assert_eq!(
            SyncError::NotFound("uid-1".into()).to_string(),
            "no calendar event with identifier uid-1"
        );
        assert_eq!(
            SyncError::Unsendable(super::Unsendable::Recurrence).to_string(),
            "a recurrence rule that cannot be stated"
        );
        assert_eq!(
            SyncError::Unscheduled(super::Unscheduled::NotImplemented).to_string(),
            "the server does not implement sendSchedulingMessages"
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
            SyncError::ICal(jmap_ical::ICalError::NoEvent)
                .source()
                .is_some()
        );
        assert!(SyncError::NotFound("uid-1".into()).source().is_none());
        assert!(
            SyncError::Unsendable(super::Unsendable::Recurrence)
                .source()
                .is_none()
        );
        assert!(
            SyncError::Unscheduled(super::Unscheduled::NotImplemented)
                .source()
                .is_none()
        );
    }
}
