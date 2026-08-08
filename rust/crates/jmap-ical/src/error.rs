// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parse failures.

/// Why a string could not be read as an iCalendar object.
///
/// Only the syntax layer fails: the semantic mapping treats anything it does
/// not recognise as absent rather than as an error, because an event that
/// loses a property is still better than a calendar that refuses to open.
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
    /// A content line has no `name:value` structure.
    Malformed(String),
    /// Content follows the end of the calendar. Dropping it silently would
    /// lose whole events when a stream carries more than one `VCALENDAR`.
    Trailing(String),
}

impl std::fmt::Display for ICalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotACalendar => f.write_str("not an iCalendar object: missing BEGIN:VCALENDAR"),
            Self::Unterminated(name) => write!(f, "truncated iCalendar: missing END:{name}"),
            Self::Mismatched { expected, found } => {
                write!(f, "END:{found} closes nothing; END:{expected} was due")
            }
            Self::Malformed(line) => write!(f, "malformed iCalendar content line: {line}"),
            Self::Trailing(line) => write!(f, "content after END:VCALENDAR: {line}"),
        }
    }
}

impl std::error::Error for ICalError {}
