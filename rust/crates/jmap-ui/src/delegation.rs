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

use std::collections::{BTreeMap, BTreeSet};

use jmap_client::{Client, Error};
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarRights};
use jmap_proto::contacts::{AddressBook, AddressBookRights};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    /// The level for an existing grant of `rights`, for showing a delegate
    /// this page did not necessarily create itself. Each level's rights are a
    /// superset of the one below it, so this reads the highest-ranked level
    /// whose defining right `rights` already has; a grant with no matching
    /// right at all reads as `None`.
    pub fn from_rights(rights: &CalendarRights) -> Self {
        if rights.may_write_all == Some(true) {
            Self::Editor
        } else if rights.may_write_own == Some(true) {
            Self::Author
        } else if rights.may_read_items == Some(true) {
            Self::Reviewer
        } else if rights.may_read_free_busy == Some(true) {
            Self::FreeBusyOnly
        } else {
            Self::None
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    /// The level for an existing grant of `rights`, for showing a delegate
    /// this page did not necessarily create itself. Author and Editor grant
    /// identical rights (RFC 9610 has no own/all write distinction), so a
    /// write grant always reads back as `Author`, never `Editor`.
    pub fn from_rights(rights: &AddressBookRights) -> Self {
        if rights.may_write == Some(true) {
            Self::Author
        } else if rights.may_read == Some(true) {
            Self::Reviewer
        } else {
            Self::None
        }
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

/// One principal's calendar and address book access, read back from
/// `shareWith`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    pub principal_id: Id,
    pub calendar_level: CalendarDelegationLevel,
    pub address_book_level: AddressBookDelegationLevel,
}

/// Every principal named in any of `calendars`' or `address_books`'
/// `shareWith`, each with the highest level found for its type.
///
/// Section 5.5 of the design note grants a level across the account's own
/// collections, not one calendar or book at a time, so a principal shared on
/// more than one calendar (or book) shows the highest level found on any of
/// them, not a list of them. A principal shared on a calendar only, or a
/// book only, gets `None` for the other type.
pub fn list_delegates(calendars: &[Calendar], address_books: &[AddressBook]) -> Vec<Delegate> {
    let mut calendar_levels: BTreeMap<Id, CalendarDelegationLevel> = BTreeMap::new();
    for calendar in calendars {
        for (principal_id, rights) in calendar.share_with.iter().flatten() {
            let level = CalendarDelegationLevel::from_rights(rights);
            calendar_levels
                .entry(principal_id.clone())
                .and_modify(|current| *current = (*current).max(level))
                .or_insert(level);
        }
    }
    let mut book_levels: BTreeMap<Id, AddressBookDelegationLevel> = BTreeMap::new();
    for book in address_books {
        for (principal_id, rights) in book.share_with.iter().flatten() {
            let level = AddressBookDelegationLevel::from_rights(rights);
            book_levels
                .entry(principal_id.clone())
                .and_modify(|current| *current = (*current).max(level))
                .or_insert(level);
        }
    }
    let principals: BTreeSet<Id> = calendar_levels
        .keys()
        .chain(book_levels.keys())
        .cloned()
        .collect();
    principals
        .into_iter()
        .map(|principal_id| Delegate {
            calendar_level: calendar_levels
                .get(&principal_id)
                .copied()
                .unwrap_or(CalendarDelegationLevel::None),
            address_book_level: book_levels
                .get(&principal_id)
                .copied()
                .unwrap_or(AddressBookDelegationLevel::None),
            principal_id,
        })
        .collect()
}

/// Fetch every calendar and address book of `account_id` and list their
/// delegates (`Calendar/get` and `AddressBook/get`, `ids: null`).
pub fn account_delegates(client: &Client, account_id: &Id) -> Result<Vec<Delegate>, Error> {
    let calendars = client.calendars(account_id)?;
    let address_books = client.address_books(account_id)?;
    Ok(list_delegates(&calendars, &address_books))
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

    #[test]
    fn every_calendar_level_reads_back_from_its_own_rights() {
        for level in [
            CalendarDelegationLevel::FreeBusyOnly,
            CalendarDelegationLevel::Reviewer,
            CalendarDelegationLevel::Author,
            CalendarDelegationLevel::Editor,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(CalendarDelegationLevel::from_rights(&rights), level);
        }
    }

    #[test]
    fn no_calendar_right_at_all_reads_back_as_none() {
        assert_eq!(
            CalendarDelegationLevel::from_rights(&CalendarRights::default()),
            CalendarDelegationLevel::None
        );
    }

    #[test]
    fn every_address_book_level_reads_back_from_its_own_rights() {
        for level in [
            AddressBookDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(AddressBookDelegationLevel::from_rights(&rights), level);
        }
        // Editor grants the identical rights Author does, so it can only
        // ever read back as Author, never as itself.
        let editor_rights = AddressBookDelegationLevel::Editor.to_rights().unwrap();
        assert_eq!(
            AddressBookDelegationLevel::from_rights(&editor_rights),
            AddressBookDelegationLevel::Author
        );
    }

    #[test]
    fn no_address_book_right_at_all_reads_back_as_none() {
        assert_eq!(
            AddressBookDelegationLevel::from_rights(&AddressBookRights::default()),
            AddressBookDelegationLevel::None
        );
    }

    /// `Calendar` and `AddressBook` are `#[non_exhaustive]`, so a fixture
    /// with a `shareWith` must go through `Deserialize`, not a struct
    /// literal, the same as `vacation/form.rs`'s tests build a response.
    fn calendar_with_share(name: &str, share_with: BTreeMap<Id, CalendarRights>) -> Calendar {
        serde_json::from_value(json!({"name": name, "shareWith": share_with})).unwrap()
    }

    fn address_book_with_share(
        name: &str,
        share_with: BTreeMap<Id, AddressBookRights>,
    ) -> AddressBook {
        serde_json::from_value(json!({"name": name, "shareWith": share_with})).unwrap()
    }

    #[test]
    fn no_shared_calendars_or_books_list_no_delegates() {
        let calendar = Calendar::new("Work");
        let book = AddressBook::new("Contacts");
        assert_eq!(list_delegates(&[calendar], &[book]), vec![]);
    }

    #[test]
    fn a_delegate_shared_on_only_one_type_gets_none_for_the_other() {
        let bob = Id::new("P-bob");
        let calendar = calendar_with_share(
            "Work",
            BTreeMap::from([(
                bob.clone(),
                CalendarDelegationLevel::Reviewer.to_rights().unwrap(),
            )]),
        );
        let delegates = list_delegates(&[calendar], &[]);
        assert_eq!(
            delegates,
            vec![Delegate {
                principal_id: bob,
                calendar_level: CalendarDelegationLevel::Reviewer,
                address_book_level: AddressBookDelegationLevel::None,
            }]
        );
    }

    #[test]
    fn a_delegate_shared_on_two_calendars_shows_the_higher_level() {
        let bob = Id::new("P-bob");
        let reviewed = calendar_with_share(
            "Work",
            BTreeMap::from([(
                bob.clone(),
                CalendarDelegationLevel::Reviewer.to_rights().unwrap(),
            )]),
        );
        let edited = calendar_with_share(
            "Home",
            BTreeMap::from([(bob, CalendarDelegationLevel::Editor.to_rights().unwrap())]),
        );
        let delegates = list_delegates(&[reviewed, edited], &[]);
        assert_eq!(delegates.len(), 1);
        assert_eq!(delegates[0].calendar_level, CalendarDelegationLevel::Editor);
    }

    #[test]
    fn delegates_from_calendars_and_books_merge_by_principal() {
        let bob = Id::new("P-bob");
        let carol = Id::new("P-carol");
        let calendar = calendar_with_share(
            "Work",
            BTreeMap::from([(
                bob.clone(),
                CalendarDelegationLevel::Author.to_rights().unwrap(),
            )]),
        );
        let book = address_book_with_share(
            "Contacts",
            BTreeMap::from([
                (
                    bob.clone(),
                    AddressBookDelegationLevel::Reviewer.to_rights().unwrap(),
                ),
                (
                    carol.clone(),
                    AddressBookDelegationLevel::Author.to_rights().unwrap(),
                ),
            ]),
        );
        let delegates = list_delegates(&[calendar], &[book]);
        assert_eq!(
            delegates,
            vec![
                Delegate {
                    principal_id: bob,
                    calendar_level: CalendarDelegationLevel::Author,
                    address_book_level: AddressBookDelegationLevel::Reviewer,
                },
                Delegate {
                    principal_id: carol,
                    calendar_level: CalendarDelegationLevel::None,
                    address_book_level: AddressBookDelegationLevel::Author,
                },
            ]
        );
    }
}
