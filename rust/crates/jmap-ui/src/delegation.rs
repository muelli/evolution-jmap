// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Delegation levels for the planned Delegates page (ROADMAP item 164).
//!
//! `notes/SHARED-MAILBOXES-2026-10-30.md` section 5 recommends building
//! delegation as "sharing with a nicer picker": the evo-ews vocabulary
//! (None, Reviewer, Author, Editor, plus a free/busy-only tier for
//! calendars) mapped onto the `shareWith` rights `Calendar` and
//! `AddressBook` already model. This module holds that mapping and the
//! `shareWith` patch and client calls it drives; the page and the
//! `Principal/query` picker are later claims.

use jmap_client::{Client, Error};
use jmap_proto::Id;
use jmap_proto::calendars::CalendarRights;
use jmap_proto::contacts::AddressBookRights;
use jmap_proto::text::escape_pointer_segment;
use serde_json::{Value, json};

/// Build the `shareWith` patch (RFC 8620 §5.3 PatchObject) that grants `rights`
/// to `principal_id`, or removes the share when `rights` is `None`.
fn share_with_patch(principal_id: &Id, rights: Option<Value>) -> Value {
    let path = format!(
        "shareWith/{}",
        escape_pointer_segment(principal_id.as_str())
    );
    json!({ path: rights.unwrap_or(Value::Null) })
}

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

    /// Build the `shareWith` patch that applies this level to `principal_id`.
    pub fn share_with_patch(self, principal_id: &Id) -> Value {
        share_with_patch(
            principal_id,
            self.to_rights()
                .map(|rights| serde_json::to_value(rights).unwrap()),
        )
    }
}

/// Apply `level` to `principal_id` on calendar `calendar_id` (`Calendar/set`
/// with a `shareWith` patch, RFC 8620 §5.3).
pub fn apply_calendar_delegation(
    client: &Client,
    account_id: &Id,
    calendar_id: &Id,
    principal_id: &Id,
    level: CalendarDelegationLevel,
) -> Result<(), Error> {
    client.calendar_update(
        account_id,
        calendar_id,
        level.share_with_patch(principal_id),
    )
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

    /// Build the `shareWith` patch that applies this level to `principal_id`.
    pub fn share_with_patch(self, principal_id: &Id) -> Value {
        share_with_patch(
            principal_id,
            self.to_rights()
                .map(|rights| serde_json::to_value(rights).unwrap()),
        )
    }
}

/// Apply `level` to `principal_id` on address book `book_id` (`AddressBook/set`
/// with a `shareWith` patch, RFC 8620 §5.3).
pub fn apply_address_book_delegation(
    client: &Client,
    account_id: &Id,
    book_id: &Id,
    principal_id: &Id,
    level: AddressBookDelegationLevel,
) -> Result<(), Error> {
    client.address_book_update(account_id, book_id, level.share_with_patch(principal_id))
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

    #[test]
    fn calendar_none_patch_removes_the_share() {
        let principal = Id::new("P-bob");
        let patch = CalendarDelegationLevel::None.share_with_patch(&principal);
        assert_eq!(patch, json!({"shareWith/P-bob": null}));
    }

    #[test]
    fn calendar_reviewer_patch_sets_read_only_rights() {
        let principal = Id::new("P-bob");
        let patch = CalendarDelegationLevel::Reviewer.share_with_patch(&principal);
        assert_eq!(
            patch,
            json!({"shareWith/P-bob": {"mayReadFreeBusy": true, "mayReadItems": true}})
        );
    }

    #[test]
    fn address_book_none_patch_removes_the_share() {
        let principal = Id::new("P-bob");
        let patch = AddressBookDelegationLevel::None.share_with_patch(&principal);
        assert_eq!(patch, json!({"shareWith/P-bob": null}));
    }

    #[test]
    fn address_book_author_patch_sets_read_and_write() {
        let principal = Id::new("P-bob");
        let patch = AddressBookDelegationLevel::Author.share_with_patch(&principal);
        assert_eq!(
            patch,
            json!({"shareWith/P-bob": {"mayRead": true, "mayWrite": true}})
        );
    }

    #[test]
    fn a_principal_id_with_a_slash_or_tilde_is_pointer_escaped() {
        // RFC 6901 §3: `~` becomes `~0`, `/` becomes `~1`, `~` escaped first.
        let principal = Id::new("P/weird~id");
        let patch = CalendarDelegationLevel::None.share_with_patch(&principal);
        assert_eq!(patch, json!({"shareWith/P~1weird~0id": null}));
    }
}
