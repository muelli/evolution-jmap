// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a synchronisation step failed.

use jmap_ical::ICalError;

/// A failure from one of the operations of [`crate::CalSync`].
///
/// [`SyncError::Client`] keeps the client error and does not convert it into a
/// string. The GObject layer maps it to the `E_CLIENT_ERROR` codes that
/// Evolution uses. Thus an authentication failure stays different from a
/// network failure through all layers.
#[derive(Debug)]
pub enum SyncError {
    /// The server refused the request, or the server was not available.
    Client(jmap_client::Error),
    /// Evolution gave a component that this crate cannot read.
    ICal(ICalError),
    /// No event on the server has this identifier.
    NotFound(String),
    /// Evolution gave a component that this crate can read, but it has a value
    /// that this mapping cannot state as JSCalendar. Thus the save is refused,
    /// and no different event is filed.
    ///
    /// Only a *create* can fail this way. An edit is a PatchObject that names
    /// only what the round trip kept, so a property that cannot be sent is
    /// omitted and the server value stays. A create has no server value. The
    /// only alternatives are an event that the user did not describe, or no
    /// event. The user sees a refusal, so this is the choice.
    Unsendable(Unsendable),
    /// The server got a request to send the scheduling messages of the change,
    /// but refused. Thus the change is not applied.
    ///
    /// This is a separate case, and not the server error in
    /// [`SyncError::Client`], because Evolution knows that the server sends
    /// mail to the guests. The user must know that no guest got a message, and
    /// what to change. `jmap_backend_cal::ops` gives this in a translated
    /// sentence.
    Unscheduled(Unscheduled),
}

/// Why the server did not send the scheduling messages of a change
/// (`docs/MEETING-SCHEDULING.md`, rule 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unscheduled {
    /// `noSupportedScheduleMethods` (draft-ietf-jmap-calendars section 5.9.2):
    /// the server cannot send to the calendar address of one or more guests.
    ///
    /// The server does not tell which guest. Thus `addresses` are the event
    /// addresses that are not `mailto:`, because iMIP (`mailto:`) is the only
    /// transport that all known servers use. The list is empty when the event
    /// is not available (a delete), or when each address is a `mailto:` that
    /// the server also refused.
    Unreachable {
        /// The calendar addresses of the event that are not `mailto:`, sorted.
        addresses: Vec<String>,
    },
    /// `invalidArguments` on a call that asked the server to schedule. By RFC
    /// 8620 section 3.9, this is a server that does not implement
    /// `sendSchedulingMessages`. Only the account switch can correct this.
    NotImplemented,
}

impl std::fmt::Display for Unscheduled {
    /// The developer's text, in English. The [`Unsendable`] implementation
    /// tells why.
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

/// What a component contains that cannot be stated as JSCalendar.
///
/// This is a reason, not a sentence, and that difference is intentional. The
/// sentence for the user must be translated, and gettext is bound on the other
/// side of the FFI, in the `i18n` of `jmap_backend_core`. An EDS-linked crate
/// can use it, but the sync layer cannot, because its tests have no EDS to link
/// against. Thus this crate tells what occurred, and `jmap_backend_cal::ops`
/// tells it in the user's language from these values.
///
/// The variants are the two useful answers, not each distinction that the code
/// can make. One names a thing that the user can change. The other tells that
/// the code cannot identify the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsendable {
    /// A recurrence whose end is an instant that cannot be restated in the time
    /// zone of the event. The entry does not define that zone, or defines it in
    /// a shape that the evaluator does not accept.
    ///
    /// `until` is the instant as the mapping kept it. It is normalised, so it
    /// has the digits that the user sees, not the spelling in the component.
    /// `zone` is the identifier that the event names. Together they tell which
    /// appointment to change and which `VTIMEZONE` a bug report must quote.
    RecurrenceEnd {
        /// The end of the series, as the mapping kept it.
        until: String,
        /// The time zone of the event.
        zone: String,
    },
    /// A recurrence rule that this mapping cannot state, for a different
    /// reason.
    ///
    /// This variant intentionally contains no data. The mapping knows that the
    /// rule cannot do the round trip. The part that causes the problem is a
    /// question about the narrowing in the mapping, not about the user's
    /// calendar. A dialog is not the correct place for that question.
    Recurrence,
}

impl std::fmt::Display for Unsendable {
    /// The developer's text for the refusal, in English and not translated.
    ///
    /// A log gets this text, and a [`SyncError`] shows as this text. The user's
    /// sentence is in `jmap_backend_cal::ops`. The separation keeps translated
    /// strings out of bug reports, and gives each audience its own phrasing.
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
    /// Whether the server refused an incremental sync because the state is too
    /// old (RFC 8620 §5.2). Real Stalwart also refuses an unrecognized state,
    /// but with `invalidArguments`, not with the RFC's `cannotCalculateChanges`
    /// (`jmap_client::Error::is_unrecognized_changes_state`).
    ///
    /// This is not a real error. The caller then uses
    /// [`crate::CalSync::list_existing`], and the meta backend compares the
    /// full calendar. Thus this predicate exists, so that callers do not
    /// compare strings.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        matches!(
            self,
            Self::Client(error)
                if error.is_cannot_calculate_changes() || error.is_unrecognized_changes_state()
        )
    }

    /// Whether the server refused the request with HTTP 401. On a long-lived
    /// connection, this occurs when a bearer token expires while the connection
    /// is idle. The caller then refreshes the token and tries again one time.
    /// It does not open the consent window immediately.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Client(error) if error.is_unauthorized())
    }

    /// A protocol violation, as a client error so that it maps the same way.
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
