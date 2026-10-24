// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Free/busy for the meeting scheduler (`get_free_busy_sync`).
//!
//! The meeting editor of Evolution asks a calendar backend "when are these
//! people busy?" and gets iCalendar `VFREEBUSY` text. JMAP answers the same
//! question in two steps, and this module does the two steps:
//!
//! 1. `Principal/query` filtered by email, to convert the address of the editor
//!    into a principal id (the server's identity for that person);
//! 2. `Principal/getAvailability` for that principal over the requested window.
//!    This gives the busy periods that [`jmap_ical::busy_periods_to_vfreebusy`]
//!    renders.
//!
//! ## Which failures are answers and which are failures
//!
//! Most of the design is this distinction. An error here is not cosmetic: if a
//! scheduler thinks that an attendee is free, it books the slot.
//!
//! - **An address that no principal matches** is an answer. An invitation to a
//!   person outside the organisation is the usual case. An empty
//!   `Principal/query` result is how the server tells this. That attendee gets
//!   no component, and the editor shows their row as unknown.
//! - **`getAvailability` with the answer `notFound`** is an answer. The draft
//!   (draft-ietf-jmap-calendars §2.2) uses that one error for "no such
//!   principal" and for "you cannot see this principal". Thus it means "not
//!   this person", not "an error occurred". The treatment is the same.
//! - **All other errors** are failures, and this module reports them. Here this
//!   module is different from the CalDAV backend, which clears each per-user
//!   error and continues silently. "The server is not available" and "nobody is
//!   busy" are different statements. Only the second must let the user put a
//!   meeting into the slot. The vfunc above still uses the `ECalMetaBackend`
//!   cache when the answer is only *empty*. This keeps the useful half of the
//!   CalDAV behaviour.

use jmap_client::Error;
use jmap_ical::busy_periods_to_vfreebusy;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::state::UtcDate;

use crate::CalSync;
use crate::error::SyncError;

/// The free/busy of one attendee, as `get_free_busy_sync` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeBusy {
    /// The address that EDS asked about, with its exact original spelling, so
    /// that the caller can pair the answer with its row.
    pub user: String,
    /// A bare `VFREEBUSY` component as iCalendar text.
    pub icalendar: String,
}

impl CalSync {
    /// The busy periods of each of `users` between `utc_start` and `utc_end`
    /// (`get_free_busy_sync`).
    ///
    /// The window is in JMAP `UTCDate`s (RFC 3339, `Z`). The caller makes these
    /// from the two `time_t` values of the vfunc.
    ///
    /// Each attendee gets a separate answer, in sequence. If the server has no
    /// data for an attendee, the result omits that attendee and reports no
    /// error. Thus a list shorter than `users` is the normal result. The module
    /// docs tell which silences are answers.
    ///
    /// The query uses the account of this calendar, not the calendar itself.
    /// Availability is about the full diary of a person, and RFC 9670 attaches
    /// principals to the account.
    pub fn free_busy(
        &self,
        users: &[String],
        utc_start: &str,
        utc_end: &str,
    ) -> Result<Vec<FreeBusy>, SyncError> {
        let (utc_start, utc_end) = (UtcDate::from(utc_start), UtcDate::from(utc_end));
        let mut answers = Vec::new();

        for user in users {
            let Some(periods) = self.availability_of(user, &utc_start, &utc_end)? else {
                continue;
            };
            // If a component cannot be rendered, this drops it and does not
            // report it. This is the only silence here that is not a server
            // answer: `busy_periods_to_vfreebusy` refuses when it cannot read a
            // period. A refusal is already the safe result, because the
            // attendee shows as unknown, never as free. No better report is
            // possible, because the function does not state the periods that it
            // *did* read without the others.
            if let Some(icalendar) = busy_periods_to_vfreebusy(user, &utc_start, &utc_end, &periods)
            {
                answers.push(FreeBusy {
                    user: user.clone(),
                    icalendar,
                });
            }
        }

        Ok(answers)
    }

    /// `Ok(None)` when the server has no data for this address. `Err` only for
    /// a failure that is not about the address.
    fn availability_of(
        &self,
        user: &str,
        utc_start: &UtcDate,
        utc_end: &UtcDate,
    ) -> Result<Option<Vec<jmap_proto::principals::BusyPeriod>>, SyncError> {
        let filter = PrincipalQueryFilter::email(address_of(user));
        let ids = self.client().principal_query(self.account_id(), filter)?;
        let Some(principal_id) = ids.first() else {
            return Ok(None);
        };

        match self.client().get_availability(
            self.account_id(),
            principal_id,
            utc_start.clone(),
            utc_end.clone(),
            // The meeting editor shows busy blocks, not their contents, and the
            // event titles of an attendee are private. A request for details
            // that the editor does not show would read a person's diary for no
            // purpose.
            false,
        ) {
            Ok(periods) => Ok(Some(periods)),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(SyncError::Client(error)),
        }
    }
}

/// The bare address in the string that EDS gave.
///
/// `ECalMetaBackend`, the CalDAV backend and the Microsoft 365 backend all make
/// their `ATTENDEE` with `mailto:` in front of the string in `users`. Thus the
/// string is a bare address. To remove the scheme anyway costs one comparison.
/// Without it, the lookup of a `mailto:` entry as an email address matches
/// nobody. That is a silent wrong answer, not a visible failure.
fn address_of(user: &str) -> &str {
    user.split_at_checked("mailto:".len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto:"))
        .map_or(user, |(_, address)| address)
}

/// Whether the server said "not this principal", not "an error occurred". The
/// draft uses this one error for its two meanings, absent and forbidden.
fn is_not_found(error: &Error) -> bool {
    matches!(error, Error::Method(method) if method.error_type == "notFound")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mailto_scheme_is_stripped_however_it_is_cased() {
        assert_eq!(address_of("mailto:bob@example.com"), "bob@example.com");
        assert_eq!(address_of("MAILTO:bob@example.com"), "bob@example.com");
        assert_eq!(address_of("MaIlTo:bob@example.com"), "bob@example.com");
    }

    #[test]
    fn anything_else_is_already_an_address() {
        assert_eq!(address_of("bob@example.com"), "bob@example.com");
        assert_eq!(address_of(""), "");
        // This is not a known scheme, and the function does not guess.
        assert_eq!(address_of("https://example.com"), "https://example.com");
    }

    #[test]
    fn only_a_notfound_method_error_is_not_found() {
        assert!(is_not_found(&Error::Method(
            jmap_proto::error::MethodError::new("notFound")
        )));
        assert!(!is_not_found(&Error::Method(
            jmap_proto::error::MethodError::new("forbidden")
        )));
        assert!(!is_not_found(&Error::Http {
            status: 500,
            problem: None,
        }));
    }
}
