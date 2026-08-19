// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Free/busy for the meeting scheduler — `get_free_busy_sync`.
//!
//! Evolution's meeting editor asks a calendar backend "when are these people
//! busy?" and gets iCalendar `VFREEBUSY` text back. JMAP answers the same
//! question in two steps, and this module is both of them:
//!
//! 1. `Principal/query` filtered by email, to turn the address the editor
//!    holds into a principal id — the server's own notion of that person;
//! 2. `Principal/getAvailability` for that principal over the requested
//!    window, which yields the busy periods
//!    [`jmap_ical::busy_periods_to_vfreebusy`] renders.
//!
//! ## Which failures are answers and which are failures
//!
//! Most of the design here is that distinction, because getting it wrong is
//! not a cosmetic bug: a scheduler that is told an attendee is free books the
//! slot.
//!
//! - **An address no principal matches** is an answer. Inviting somebody
//!   outside the organisation is the ordinary case, and `Principal/query`
//!   returning nothing is how the server says so. That attendee gets no
//!   component and the editor leaves their row unknown.
//! - **`getAvailability` answering `notFound`** is an answer. The draft
//!   (draft-ietf-jmap-calendars §2.2) uses that one error for both "no such
//!   principal" and "you may not see this one", so it means "not this person",
//!   not "something went wrong". Same treatment.
//! - **Anything else** is a failure and is reported. This is where this module
//!   parts company with the CalDAV backend, which clears every per-user error
//!   and falls through silently. A server that could not be reached and a
//!   server that said nobody is busy are not the same statement, and only one
//!   of them should let the user drop a meeting into the slot. The vfunc above
//!   still falls back to `ECalMetaBackend`'s cache when the answer is merely
//!   *empty* — which is the useful half of the CalDAV behaviour, kept.

use jmap_client::Error;
use jmap_ical::busy_periods_to_vfreebusy;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::state::UtcDate;

use crate::CalSync;
use crate::error::SyncError;

/// One attendee's free/busy, as `get_free_busy_sync` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeBusy {
    /// The address EDS asked about, echoed back exactly as it was spelled so
    /// the caller can pair the answer with the row it came from.
    pub user: String,
    /// A bare `VFREEBUSY` component as iCalendar text.
    pub icalendar: String,
}

impl CalSync {
    /// The busy periods of each of `users` between `utc_start` and `utc_end` —
    /// `get_free_busy_sync`.
    ///
    /// The window is stated as JMAP `UTCDate`s (RFC 3339, `Z`), which is what
    /// the caller has after converting the vfunc's two `time_t`s.
    ///
    /// Attendees are answered independently and in order; one that the server
    /// has nothing to say about is left out rather than reported, so a shorter
    /// list than `users` is the normal result. See the module docs for exactly
    /// which silences are answers.
    ///
    /// The account asked is this calendar's, not this calendar itself:
    /// availability is a question about a person's whole diary, and RFC 9670
    /// hangs principals off the account.
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
            // A component that cannot be rendered is dropped rather than
            // reported, and it is the one silence here that is not an answer
            // from the server: `busy_periods_to_vfreebusy` refuses when a
            // period cannot be read, and refusing is already its safe
            // direction — the attendee reads as unknown, never as free. There
            // is nothing better to report, since the periods that *did* read
            // are exactly the ones it will not state on their own.
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

    /// `Ok(None)` when the server has nothing to say about this address;
    /// `Err` only for a failure that is not about the address.
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
            // The meeting editor renders busy blocks, not their contents, and
            // an attendee's event titles are theirs. Asking for details we do
            // not draw would be reading someone's diary for nothing.
            false,
        ) {
            Ok(periods) => Ok(Some(periods)),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(SyncError::Client(error)),
        }
    }
}

/// The bare address inside whatever EDS handed us.
///
/// `ECalMetaBackend`, the CalDAV backend and the Microsoft 365 backend all
/// build their `ATTENDEE` by prepending `mailto:` to the string in `users`, so
/// a bare address is what it holds. Stripping the scheme anyway costs one
/// comparison, and without it a `mailto:`-prefixed entry would be looked up as
/// an email address and match nobody — a silent wrong answer rather than a
/// visible failure.
fn address_of(user: &str) -> &str {
    user.split_at_checked("mailto:".len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("mailto:"))
        .map_or(user, |(_, address)| address)
}

/// Whether the server said "not this principal" rather than "something went
/// wrong" — the draft spells both of its meanings, absent and forbidden, as
/// this one error.
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
        // Not a scheme this understands, and not one to guess at either.
        assert_eq!(address_of("https://example.com"), "https://example.com");
    }
}
