// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Delegation levels for the planned Delegates page (ROADMAP item 164).
//!
//! `notes/SHARED-MAILBOXES-2026-10-30.md` section 5 recommends building
//! delegation as "sharing with a nicer picker": the evo-ews vocabulary
//! (None, Reviewer, Author, Editor, plus a free/busy-only tier for
//! calendars) mapped onto the `shareWith` rights `Calendar` and
//! `AddressBook` already model. This module holds only that mapping; the
//! page, the `Principal/query` picker and the `shareWith` wiring are later
//! claims.

use jmap_proto::calendars::CalendarRights;
use jmap_proto::contacts::AddressBookRights;

/// A delegation level offered for a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarDelegationLevel {
    /// No access. Removes an existing share.
    None,
    /// Free/busy times only, no event detail.
    FreeBusyOnly,
    /// Full read access to events.
    Reviewer,
    /// Reviewer, plus create and edit events the delegate owns.
    Author,
    /// Author, plus edit every event and other users' private properties.
    Editor,
}

impl CalendarDelegationLevel {
    /// The `shareWith` entry for this level, or `None` to remove the share.
    pub fn to_rights(self) -> Option<CalendarRights> {
        match self {
            Self::None => None,
            Self::FreeBusyOnly => Some(CalendarRights::default().with_may_read_free_busy(true)),
            Self::Reviewer => Some(CalendarRights::read_only()),
            Self::Author => Some(
                CalendarRights::read_only()
                    .with_may_write_own(true)
                    .with_may_rsvp(true),
            ),
            Self::Editor => Some(
                CalendarRights::read_only()
                    .with_may_write_own(true)
                    .with_may_rsvp(true)
                    .with_may_write_all(true)
                    .with_may_update_private(true),
            ),
        }
    }
}

/// A delegation level offered for an address book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressBookDelegationLevel {
    /// No access. Removes an existing share.
    None,
    /// Full read access to cards.
    Reviewer,
    /// Reviewer, plus create, update and destroy cards.
    Author,
    /// Same rights as Author.
    ///
    /// RFC 9610 models one `mayWrite` right, not the separate own/all write
    /// rights `CalendarRights` has, so this level cannot grant anything
    /// Author does not already grant.
    Editor,
}

impl AddressBookDelegationLevel {
    /// The `shareWith` entry for this level, or `None` to remove the share.
    pub fn to_rights(self) -> Option<AddressBookRights> {
        match self {
            Self::None => None,
            Self::Reviewer => Some(AddressBookRights::read_only()),
            Self::Author | Self::Editor => {
                Some(AddressBookRights::read_only().with_may_write(true))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_none_removes_the_share() {
        assert_eq!(CalendarDelegationLevel::None.to_rights(), None);
    }

    #[test]
    fn calendar_free_busy_only_grants_no_item_access() {
        let rights = CalendarDelegationLevel::FreeBusyOnly.to_rights().unwrap();
        assert_eq!(rights.may_read_free_busy, Some(true));
        assert_eq!(rights.may_read_items, None);
        assert!(!rights.is_writable());
    }

    #[test]
    fn calendar_reviewer_is_read_only() {
        let rights = CalendarDelegationLevel::Reviewer.to_rights().unwrap();
        assert_eq!(rights.may_read_free_busy, Some(true));
        assert_eq!(rights.may_read_items, Some(true));
        assert!(!rights.is_writable());
        assert_eq!(rights.may_write_all, None);
    }

    #[test]
    fn calendar_author_can_write_and_rsvp_but_not_everyone_elses_events() {
        let rights = CalendarDelegationLevel::Author.to_rights().unwrap();
        assert_eq!(rights.may_read_items, Some(true));
        assert_eq!(rights.may_write_own, Some(true));
        assert_eq!(rights.may_rsvp, Some(true));
        assert_eq!(rights.may_write_all, None);
        assert_eq!(rights.may_update_private, None);
    }

    #[test]
    fn calendar_editor_can_write_every_event() {
        let rights = CalendarDelegationLevel::Editor.to_rights().unwrap();
        assert_eq!(rights.may_write_all, Some(true));
        assert_eq!(rights.may_update_private, Some(true));
        assert_eq!(rights.may_write_own, Some(true));
    }

    #[test]
    fn calendar_levels_never_grant_delete_or_share() {
        for level in [
            CalendarDelegationLevel::FreeBusyOnly,
            CalendarDelegationLevel::Reviewer,
            CalendarDelegationLevel::Author,
            CalendarDelegationLevel::Editor,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(rights.may_delete, None);
            assert_eq!(rights.may_share, None);
        }
    }

    #[test]
    fn address_book_none_removes_the_share() {
        assert_eq!(AddressBookDelegationLevel::None.to_rights(), None);
    }

    #[test]
    fn address_book_reviewer_is_read_only() {
        let rights = AddressBookDelegationLevel::Reviewer.to_rights().unwrap();
        assert_eq!(rights.may_read, Some(true));
        assert!(!rights.is_writable());
    }

    #[test]
    fn address_book_author_and_editor_grant_identical_rights() {
        // RFC 9610 has no own/all write distinction, unlike CalendarRights:
        // record that gap here instead of promising a level this server
        // cannot tell apart.
        assert_eq!(
            AddressBookDelegationLevel::Author.to_rights(),
            AddressBookDelegationLevel::Editor.to_rights()
        );
        let rights = AddressBookDelegationLevel::Author.to_rights().unwrap();
        assert_eq!(rights.may_read, Some(true));
        assert!(rights.is_writable());
    }

    #[test]
    fn address_book_levels_never_grant_delete_or_share() {
        for level in [
            AddressBookDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
            AddressBookDelegationLevel::Editor,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(rights.may_delete, None);
            assert_eq!(rights.may_share, None);
        }
    }
}
