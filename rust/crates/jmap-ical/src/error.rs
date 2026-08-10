// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parse failures.

/// Why a string could not be read as an iCalendar object.
///
/// The semantic mapping treats anything it does not recognise as absent
/// rather than as an error, because an event that loses a property is still
/// better than a calendar that refuses to open. It has exactly one failure of
/// its own, [`NoEvent`](Self::NoEvent): a document with nothing in it to map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ICalError {
    /// The input does not start with `BEGIN:VCALENDAR`.
    NotACalendar,
    /// A `BEGIN` was never closed by a matching `END`.
    Unterminated(String),
    /// An `END` names a component other than the innermost open one.
    Mismatched {
        /// The component that is open.
        expected: String,
        /// The component the `END` line named.
        found: String,
    },
    /// Content follows the end of the calendar. Dropping it silently would
    /// lose whole events when a stream carries more than one `VCALENDAR`.
    Trailing(String),
    /// Components nested past [`MAX_DEPTH`], naming the one that went too far.
    /// The tree a parse returns is dropped recursively, so a document deeper
    /// than the stack can hold would abort the process rather than fail.
    ///
    /// [`MAX_DEPTH`]: crate::syntax::MAX_DEPTH
    TooDeep(String),
    /// A well-formed calendar that holds no `VEVENT` — a `VTODO` or a bare
    /// `VTIMEZONE`. There is no event to hand back and nothing to store.
    NoEvent,
}

impl std::fmt::Display for ICalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACalendar => f.write_str("not an iCalendar object: missing BEGIN:VCALENDAR"),
            Self::Unterminated(name) => write!(f, "truncated iCalendar: missing END:{name}"),
            Self::Mismatched { expected, found } => {
                write!(f, "END:{found} closes nothing; END:{expected} was due")
            }
            Self::Trailing(line) => write!(f, "content after END:VCALENDAR: {line}"),
            Self::TooDeep(name) => write!(
                f,
                "iCalendar components nested more than {} deep at BEGIN:{name}",
                crate::syntax::MAX_DEPTH
            ),
            Self::NoEvent => f.write_str("iCalendar object contains no VEVENT"),
        }
    }
}

impl std::error::Error for ICalError {}
