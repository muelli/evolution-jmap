// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parse failures.

/// Describes why a string could not be parsed as an iCalendar object.
///
/// Unrecognised properties are treated as absent rather than errors.
/// [`NoEvent`](Self::NoEvent) indicates that the document contains no event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ICalError {
    /// The input does not start with `BEGIN:VCALENDAR`.
    NotACalendar,
    /// A `BEGIN` tag has no matching `END` tag.
    Unterminated(String),
    /// An `END` tag names a component other than the innermost open component.
    Mismatched {
        /// The component that is open.
        expected: String,
        /// The component named by the `END` line.
        found: String,
    },
    /// Content follows the end of the calendar.
    Trailing(String),
    /// Components nested beyond [`MAX_DEPTH`].
    ///
    /// [`MAX_DEPTH`]: crate::event::MAX_DEPTH
    TooDeep(String),
    /// The calendar object contains no `VEVENT` component.
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
                crate::event::MAX_DEPTH
            ),
            Self::NoEvent => f.write_str("iCalendar object contains no VEVENT"),
        }
    }
}

impl std::error::Error for ICalError {}
