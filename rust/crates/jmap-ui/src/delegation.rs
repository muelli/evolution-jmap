// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Delegation levels for the planned Delegates page (ROADMAP item 164).
//!
//! `notes/SHARED-MAILBOXES-2026-10-30.md` section 5 recommends building
//! delegation as "sharing with a nicer picker": the evo-ews vocabulary
//! (None, Reviewer, Author, Editor, plus a free/busy-only tier for
//! calendars) mapped onto the `shareWith` rights `Calendar` and
//! `AddressBook` already model. This module holds that mapping, the
//! `shareWith` patch and client calls it drives, and the `Principal/query`
//! lookup for the picker (falling back to manual id entry when the server
//! refuses the query, as `stalwart-runner` does); the GTK page itself is a
//! later claim.

use std::collections::{BTreeMap, BTreeSet};

use jmap_client::{Client, Error};
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarRights};
use jmap_proto::contacts::{AddressBook, AddressBookRights};
use jmap_proto::error::method::{FORBIDDEN, UNKNOWN_METHOD};
use jmap_proto::mail::{Mailbox, MailboxRights, role};
use jmap_proto::principals::{Principal, PrincipalQueryFilter};
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

/// Apply `level` to `principal_id` on every one of `account_id`'s own
/// calendars (`Calendar/get`, `ids: null`, then one `Calendar/set` update per
/// calendar).
///
/// Section 5.5 of the design note grants a level across the account's own
/// collections, not one calendar at a time, the write-side counterpart of
/// how [`list_delegates`] already reads a principal's level back as the
/// highest found on any of them.
pub fn commit_calendar_delegation(
    client: &Client,
    account_id: &Id,
    principal_id: &Id,
    level: CalendarDelegationLevel,
) -> Result<(), Error> {
    for calendar in client.calendars(account_id)? {
        let Some(calendar_id) = calendar.id else {
            continue;
        };
        apply_calendar_delegation(client, account_id, &calendar_id, principal_id, level)?;
    }
    Ok(())
}

/// A delegation level offered for an address book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddressBookDelegationLevel {
    /// No access. Removes an existing share.
    None,
    /// Full read access to cards.
    Reviewer,
    /// Reviewer, plus create, update and destroy cards.
    ///
    /// RFC 9610 models one `mayWrite` right, not the separate own/all write
    /// rights `CalendarRights` has, so there is no further level between
    /// this and full access: an "Editor" level would grant nothing this one
    /// does not already grant, and so is not offered.
    Author,
}

impl AddressBookDelegationLevel {
    /// The `shareWith` entry for this level, or `None` to remove the share.
    pub fn to_rights(self) -> Option<AddressBookRights> {
        match self {
            Self::None => None,
            Self::Reviewer => Some(AddressBookRights::read_only()),
            Self::Author => Some(AddressBookRights::read_only().with_may_write(true)),
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
    /// this page did not necessarily create itself.
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

/// Apply `level` to `principal_id` on every one of `account_id`'s own
/// address books (`AddressBook/get`, `ids: null`, then one `AddressBook/set`
/// update per book). The write-side counterpart of how [`list_delegates`]
/// reads a principal's level back as the highest found on any of them.
pub fn commit_address_book_delegation(
    client: &Client,
    account_id: &Id,
    principal_id: &Id,
    level: AddressBookDelegationLevel,
) -> Result<(), Error> {
    for book in client.address_books(account_id)? {
        let Some(book_id) = book.id else {
            continue;
        };
        apply_address_book_delegation(client, account_id, &book_id, principal_id, level)?;
    }
    Ok(())
}

/// A delegation level offered for mail: the Inbox and its subfolders, see
/// `notes/DELEGATES-MAIL-ROW-DESIGN-2026-11-23.md` section 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MailDelegationLevel {
    /// No access. Removes an existing share.
    None,
    /// Full read access to messages.
    Reviewer,
    /// Reviewer, plus add messages and set seen and other keywords.
    Author,
    /// Author, plus remove messages.
    Editor,
}

impl MailDelegationLevel {
    /// The `shareWith` entry for this level, or `None` to remove the share.
    pub fn to_rights(self) -> Option<MailboxRights> {
        match self {
            Self::None => None,
            Self::Reviewer => Some(MailboxRights::read_only()),
            Self::Author => Some(
                MailboxRights::read_only()
                    .with_may_add_items(true)
                    .with_may_set_seen(true)
                    .with_may_set_keywords(true),
            ),
            Self::Editor => Some(
                MailboxRights::read_only()
                    .with_may_add_items(true)
                    .with_may_set_seen(true)
                    .with_may_set_keywords(true)
                    .with_may_remove_items(true),
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
    pub fn from_rights(rights: &MailboxRights) -> Self {
        if rights.may_remove_items == Some(true) {
            Self::Editor
        } else if rights.may_add_items == Some(true) {
            Self::Author
        } else if rights.may_read_items == Some(true) {
            Self::Reviewer
        } else {
            Self::None
        }
    }
}

/// The ids of the Inbox and every mailbox whose `parentId` chain reaches it
/// (design note `notes/DELEGATES-MAIL-ROW-DESIGN-2026-11-23.md` section 1:
/// "Inbox with its subfolders", not every mailbox of the account). Empty
/// when `mailboxes` has no mailbox with `role == "inbox"`.
fn inbox_subtree_ids(mailboxes: &[Mailbox]) -> BTreeSet<Id> {
    let Some(inbox_id) = mailboxes
        .iter()
        .find(|mailbox| mailbox.role.as_deref() == Some(role::INBOX))
        .and_then(|mailbox| mailbox.id.clone())
    else {
        return BTreeSet::new();
    };
    let by_id: BTreeMap<&Id, &Mailbox> = mailboxes
        .iter()
        .filter_map(|mailbox| mailbox.id.as_ref().map(|id| (id, mailbox)))
        .collect();
    mailboxes
        .iter()
        .filter_map(|mailbox| mailbox.id.clone())
        .filter(|id| *id == inbox_id || chain_reaches(&by_id, id, &inbox_id))
        .collect()
}

/// Whether `id`'s `parentId` chain reaches `target`, walking at most
/// [`jmap_mail_sync::folder::MAX_DEPTH`] ancestors so a `parentId` loop a
/// buggy server sends cannot hang this walk (same bound `FolderTree`
/// already uses for the same reason).
fn chain_reaches(by_id: &BTreeMap<&Id, &Mailbox>, id: &Id, target: &Id) -> bool {
    let mut current = by_id.get(id).and_then(|mailbox| mailbox.parent_id.clone());
    for _ in 0..jmap_mail_sync::folder::MAX_DEPTH {
        match current {
            Some(parent_id) if parent_id == *target => return true,
            Some(parent_id) => {
                current = by_id
                    .get(&parent_id)
                    .and_then(|mailbox| mailbox.parent_id.clone());
            }
            None => return false,
        }
    }
    false
}

/// Apply `level` to `principal_id` on mailbox `mailbox_id` (`Mailbox/set`
/// with a `shareWith` patch, draft-ietf-jmap-mail-sharing §2).
pub fn apply_mail_delegation(
    client: &Client,
    account_id: &Id,
    mailbox_id: &Id,
    principal_id: &Id,
    level: MailDelegationLevel,
) -> Result<(), Error> {
    client.mailbox_update(account_id, mailbox_id, level.share_with_patch(principal_id))
}

/// Apply `level` to `principal_id` on the Inbox and every mailbox whose
/// `parentId` chain reaches it (`Mailbox/get`, then one `Mailbox/set` update
/// per mailbox in that subtree). Unlike [`commit_calendar_delegation`] and
/// [`commit_address_book_delegation`], this does not grant on every mailbox
/// of the account: design note section 1 picks the Inbox subtree as the
/// default scope for "share my mail".
pub fn commit_mail_delegation(
    client: &Client,
    account_id: &Id,
    principal_id: &Id,
    level: MailDelegationLevel,
) -> Result<(), Error> {
    let mailboxes = client.mailbox_get(account_id)?.list;
    for mailbox_id in inbox_subtree_ids(&mailboxes) {
        apply_mail_delegation(client, account_id, &mailbox_id, principal_id, level)?;
    }
    Ok(())
}

/// One principal's calendar, address book and mail access, read back from
/// `shareWith`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    pub principal_id: Id,
    pub calendar_level: CalendarDelegationLevel,
    pub address_book_level: AddressBookDelegationLevel,
    pub mail_level: MailDelegationLevel,
}

/// Every principal named in any of `calendars`' or `address_books`'
/// `shareWith`, or in the Inbox subtree of `mailboxes` (design note section
/// 1), each with the highest level found for its type.
///
/// Section 5.5 of the design note grants a level across the account's own
/// collections, not one calendar or book at a time, so a principal shared on
/// more than one calendar (or book) shows the highest level found on any of
/// them, not a list of them. The Mail row (section 3) extends this the same
/// way, restricted to the Inbox and its subfolders: a mailbox outside that
/// subtree is not counted even if its own `shareWith` names a principal. A
/// principal shared on only one type gets `None` for the others.
pub fn list_delegates(
    calendars: &[Calendar],
    address_books: &[AddressBook],
    mailboxes: &[Mailbox],
) -> Vec<Delegate> {
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
    let inbox_subtree = inbox_subtree_ids(mailboxes);
    let mut mail_levels: BTreeMap<Id, MailDelegationLevel> = BTreeMap::new();
    for mailbox in mailboxes {
        let Some(mailbox_id) = &mailbox.id else {
            continue;
        };
        if !inbox_subtree.contains(mailbox_id) {
            continue;
        }
        for (principal_id, rights) in mailbox.share_with.iter().flatten() {
            let level = MailDelegationLevel::from_rights(rights);
            mail_levels
                .entry(principal_id.clone())
                .and_modify(|current| *current = (*current).max(level))
                .or_insert(level);
        }
    }
    let principals: BTreeSet<Id> = calendar_levels
        .keys()
        .chain(book_levels.keys())
        .chain(mail_levels.keys())
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
            mail_level: mail_levels
                .get(&principal_id)
                .copied()
                .unwrap_or(MailDelegationLevel::None),
            principal_id,
        })
        .collect()
}

/// Fetch every calendar, address book and mailbox of `account_id` and list
/// their delegates (`Calendar/get`, `AddressBook/get` and `Mailbox/get`,
/// `ids: null`).
pub fn account_delegates(client: &Client, account_id: &Id) -> Result<Vec<Delegate>, Error> {
    let calendars = client.calendars(account_id)?;
    let address_books = client.address_books(account_id)?;
    let mailboxes = client.mailbox_get(account_id)?.list;
    Ok(list_delegates(&calendars, &address_books, &mailboxes))
}

/// A display label for a principal, built from its `name` and `email`
/// (item 179(f): the Delegates list must not show only a raw principal id).
/// `"Name <email>"` when both are present, the one present alone when only
/// one is, `None` when neither, so the caller falls back to the raw id.
pub fn principal_label(name: &str, email: Option<&str>) -> Option<String> {
    let name = name.trim();
    let email = email.map(str::trim).filter(|email| !email.is_empty());
    match (name.is_empty(), email) {
        (false, Some(email)) => Some(format!("{name} <{email}>")),
        (false, None) => Some(name.to_string()),
        (true, Some(email)) => Some(email.to_string()),
        (true, None) => None,
    }
}

/// Best-effort name/email lookup for `ids` (`Principal/get`), for the
/// Delegates list. A server error, or an id it cannot resolve, just leaves
/// that id out of the map; the caller falls back to the raw id row by row,
/// so no lookup failure here can block the list from showing.
pub fn resolve_principal_labels(
    client: &Client,
    account_id: &Id,
    ids: &[Id],
) -> BTreeMap<Id, String> {
    let Ok(principals) = client.principal_get(account_id, ids) else {
        return BTreeMap::new();
    };
    principals
        .into_iter()
        .filter_map(|principal| {
            let id = principal.id.clone()?;
            let label = principal_label(&principal.name, principal.email.as_deref())?;
            Some((id, label))
        })
        .collect()
}

/// The result of looking up a principal to delegate to.
#[derive(Debug, Clone, PartialEq)]
pub enum PrincipalSearch {
    /// The server answered `Principal/query`; these principals matched
    /// `text` (possibly none).
    Found(Vec<Principal>),
    /// The server refuses `Principal/query` (Stalwart's default directory
    /// policy). The caller should fall back to a manual principal-id field.
    QueryRefused,
}

/// Look up principals matching `text` (`Principal/query` with a free-text
/// filter, then `Principal/get` on the matching ids), for the Delegates
/// page's picker. `text` is matched against name, email and other
/// properties, server-side.
///
/// When the server refuses the query with `forbidden` or does not implement
/// it at all, this returns `QueryRefused` rather than an error, so the page
/// can offer manual principal-id entry instead (the item 164 design note's
/// documented fallback for `stalwart-runner`, which disables directory
/// queries). Any other error still propagates.
pub fn search_principals(
    client: &Client,
    account_id: &Id,
    text: &str,
) -> Result<PrincipalSearch, Error> {
    let ids = match client.principal_query(account_id, PrincipalQueryFilter::default().text(text)) {
        Ok(ids) => ids,
        Err(Error::Method(method_error))
            if method_error.error_type == FORBIDDEN
                || method_error.error_type == UNKNOWN_METHOD =>
        {
            return Ok(PrincipalSearch::QueryRefused);
        }
        Err(error) => return Err(error),
    };
    if ids.is_empty() {
        return Ok(PrincipalSearch::Found(vec![]));
    }
    Ok(PrincipalSearch::Found(
        client.principal_get(account_id, &ids)?,
    ))
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
    fn every_two_address_book_levels_grant_different_rights() {
        // 179(g): a level this server cannot tell apart from another must
        // not be offered; each pair here must grant a different right set.
        let levels = [
            AddressBookDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
        ];
        for (at, a) in levels.iter().enumerate() {
            for b in &levels[at + 1..] {
                assert_ne!(
                    a.to_rights(),
                    b.to_rights(),
                    "{a:?} and {b:?} grant the same rights"
                );
            }
        }
        let rights = AddressBookDelegationLevel::Author.to_rights().unwrap();
        assert_eq!(rights.may_read, Some(true));
        assert!(rights.is_writable());
    }

    #[test]
    fn address_book_levels_never_grant_delete_or_share() {
        for level in [
            AddressBookDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(rights.may_delete, None);
            assert_eq!(rights.may_share, None);
        }
    }

    #[test]
    fn mail_none_removes_the_share() {
        assert_eq!(MailDelegationLevel::None.to_rights(), None);
    }

    #[test]
    fn mail_reviewer_is_read_only() {
        let rights = MailDelegationLevel::Reviewer.to_rights().unwrap();
        assert_eq!(rights.may_read_items, Some(true));
        assert_eq!(rights.may_add_items, None);
        assert_eq!(rights.may_set_seen, None);
        assert_eq!(rights.may_set_keywords, None);
        assert_eq!(rights.may_remove_items, None);
    }

    #[test]
    fn mail_author_can_add_and_flag_but_not_remove() {
        let rights = MailDelegationLevel::Author.to_rights().unwrap();
        assert_eq!(rights.may_read_items, Some(true));
        assert_eq!(rights.may_add_items, Some(true));
        assert_eq!(rights.may_set_seen, Some(true));
        assert_eq!(rights.may_set_keywords, Some(true));
        assert_eq!(rights.may_remove_items, None);
    }

    #[test]
    fn mail_editor_can_remove() {
        let rights = MailDelegationLevel::Editor.to_rights().unwrap();
        assert_eq!(rights.may_add_items, Some(true));
        assert_eq!(rights.may_remove_items, Some(true));
    }

    #[test]
    fn every_two_mail_levels_grant_different_rights() {
        let levels = [
            MailDelegationLevel::Reviewer,
            MailDelegationLevel::Author,
            MailDelegationLevel::Editor,
        ];
        for (at, a) in levels.iter().enumerate() {
            for b in &levels[at + 1..] {
                assert_ne!(
                    a.to_rights(),
                    b.to_rights(),
                    "{a:?} and {b:?} grant the same rights"
                );
            }
        }
    }

    #[test]
    fn mail_levels_never_grant_create_rename_delete_share_or_submit() {
        for level in [
            MailDelegationLevel::Reviewer,
            MailDelegationLevel::Author,
            MailDelegationLevel::Editor,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(rights.may_create_child, None);
            assert_eq!(rights.may_rename, None);
            assert_eq!(rights.may_delete, None);
            assert_eq!(rights.may_share, None);
            assert_eq!(rights.may_submit, None);
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
    fn mail_none_patch_removes_the_share() {
        let principal = Id::new("P-bob");
        let patch = MailDelegationLevel::None.share_with_patch(&principal);
        assert_eq!(patch, json!({"shareWith/P-bob": null}));
    }

    #[test]
    fn mail_author_patch_sets_read_add_seen_and_keywords() {
        let principal = Id::new("P-bob");
        let patch = MailDelegationLevel::Author.share_with_patch(&principal);
        assert_eq!(
            patch,
            json!({"shareWith/P-bob": {
                "mayReadItems": true,
                "mayAddItems": true,
                "maySetSeen": true,
                "maySetKeywords": true
            }})
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
    }

    #[test]
    fn no_address_book_right_at_all_reads_back_as_none() {
        assert_eq!(
            AddressBookDelegationLevel::from_rights(&AddressBookRights::default()),
            AddressBookDelegationLevel::None
        );
    }

    #[test]
    fn every_mail_level_reads_back_from_its_own_rights() {
        for level in [
            MailDelegationLevel::Reviewer,
            MailDelegationLevel::Author,
            MailDelegationLevel::Editor,
        ] {
            let rights = level.to_rights().unwrap();
            assert_eq!(MailDelegationLevel::from_rights(&rights), level);
        }
    }

    #[test]
    fn no_mail_right_at_all_reads_back_as_none() {
        assert_eq!(
            MailDelegationLevel::from_rights(&MailboxRights::default()),
            MailDelegationLevel::None
        );
    }

    #[test]
    fn inbox_subtree_includes_the_inbox_and_its_descendants() {
        let inbox = Mailbox::new("Inbox")
            .with_id("M-inbox")
            .with_role(role::INBOX);
        let child = Mailbox::new("Work")
            .with_id("M-child")
            .with_parent_id("M-inbox");
        let grandchild = Mailbox::new("Urgent")
            .with_id("M-grandchild")
            .with_parent_id("M-child");
        let ids = inbox_subtree_ids(&[inbox, child, grandchild]);
        assert_eq!(
            ids,
            BTreeSet::from([
                Id::new("M-inbox"),
                Id::new("M-child"),
                Id::new("M-grandchild"),
            ])
        );
    }

    #[test]
    fn inbox_subtree_excludes_mailboxes_outside_it() {
        let inbox = Mailbox::new("Inbox")
            .with_id("M-inbox")
            .with_role(role::INBOX);
        let sent = Mailbox::new("Sent").with_id("M-sent").with_role(role::SENT);
        let other_top_level = Mailbox::new("Lists").with_id("M-lists");
        let ids = inbox_subtree_ids(&[inbox, sent, other_top_level]);
        assert_eq!(ids, BTreeSet::from([Id::new("M-inbox")]));
    }

    #[test]
    fn inbox_subtree_is_empty_with_no_inbox() {
        let sent = Mailbox::new("Sent").with_id("M-sent").with_role(role::SENT);
        assert_eq!(inbox_subtree_ids(&[sent]), BTreeSet::new());
    }

    #[test]
    fn inbox_subtree_walk_does_not_hang_on_a_parent_id_loop() {
        // A buggy server's parentId cycle: M-a's parent is M-b, M-b's parent
        // is M-a. Neither reaches the real inbox, so neither is included.
        let inbox = Mailbox::new("Inbox")
            .with_id("M-inbox")
            .with_role(role::INBOX);
        let a = Mailbox::new("A").with_id("M-a").with_parent_id("M-b");
        let b = Mailbox::new("B").with_id("M-b").with_parent_id("M-a");
        let ids = inbox_subtree_ids(&[inbox, a, b]);
        assert_eq!(ids, BTreeSet::from([Id::new("M-inbox")]));
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

    /// A mailbox with `role` (`None` for an ordinary folder) and `shareWith`.
    fn mailbox_with_share(
        id: &str,
        role: Option<&str>,
        parent_id: Option<&str>,
        share_with: BTreeMap<Id, MailboxRights>,
    ) -> Mailbox {
        serde_json::from_value(json!({
            "id": id,
            "name": id,
            "role": role,
            "parentId": parent_id,
            "shareWith": share_with,
        }))
        .unwrap()
    }

    #[test]
    fn no_shared_calendars_books_or_mailboxes_list_no_delegates() {
        let calendar = Calendar::new("Work");
        let book = AddressBook::new("Contacts");
        assert_eq!(list_delegates(&[calendar], &[book], &[]), vec![]);
    }

    #[test]
    fn a_delegate_shared_on_only_one_type_gets_none_for_the_others() {
        let bob = Id::new("P-bob");
        let calendar = calendar_with_share(
            "Work",
            BTreeMap::from([(
                bob.clone(),
                CalendarDelegationLevel::Reviewer.to_rights().unwrap(),
            )]),
        );
        let delegates = list_delegates(&[calendar], &[], &[]);
        assert_eq!(
            delegates,
            vec![Delegate {
                principal_id: bob,
                calendar_level: CalendarDelegationLevel::Reviewer,
                address_book_level: AddressBookDelegationLevel::None,
                mail_level: MailDelegationLevel::None,
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
        let delegates = list_delegates(&[reviewed, edited], &[], &[]);
        assert_eq!(delegates.len(), 1);
        assert_eq!(delegates[0].calendar_level, CalendarDelegationLevel::Editor);
    }

    #[test]
    fn delegates_from_calendars_books_and_mail_merge_by_principal() {
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
        let inbox = mailbox_with_share(
            "M-inbox",
            Some(role::INBOX),
            None,
            BTreeMap::from([(
                carol.clone(),
                MailDelegationLevel::Editor.to_rights().unwrap(),
            )]),
        );
        let delegates = list_delegates(&[calendar], &[book], &[inbox]);
        assert_eq!(
            delegates,
            vec![
                Delegate {
                    principal_id: bob,
                    calendar_level: CalendarDelegationLevel::Author,
                    address_book_level: AddressBookDelegationLevel::Reviewer,
                    mail_level: MailDelegationLevel::None,
                },
                Delegate {
                    principal_id: carol,
                    calendar_level: CalendarDelegationLevel::None,
                    address_book_level: AddressBookDelegationLevel::Author,
                    mail_level: MailDelegationLevel::Editor,
                },
            ]
        );
    }

    #[test]
    fn a_delegate_shared_on_two_mail_folders_in_the_inbox_subtree_shows_the_higher_level() {
        let bob = Id::new("P-bob");
        let inbox = mailbox_with_share(
            "M-inbox",
            Some(role::INBOX),
            None,
            BTreeMap::from([(
                bob.clone(),
                MailDelegationLevel::Reviewer.to_rights().unwrap(),
            )]),
        );
        let child = mailbox_with_share(
            "M-child",
            None,
            Some("M-inbox"),
            BTreeMap::from([(bob, MailDelegationLevel::Editor.to_rights().unwrap())]),
        );
        let delegates = list_delegates(&[], &[], &[inbox, child]);
        assert_eq!(delegates.len(), 1);
        assert_eq!(delegates[0].mail_level, MailDelegationLevel::Editor);
    }

    #[test]
    fn a_mail_share_outside_the_inbox_subtree_is_not_counted() {
        let bob = Id::new("P-bob");
        let inbox = mailbox_with_share("M-inbox", Some(role::INBOX), None, BTreeMap::new());
        let lists = mailbox_with_share(
            "M-lists",
            None,
            None,
            BTreeMap::from([(bob, MailDelegationLevel::Editor.to_rights().unwrap())]),
        );
        let delegates = list_delegates(&[], &[], &[inbox, lists]);
        assert_eq!(delegates, vec![], "Lists sits outside the Inbox subtree");
    }

    #[test]
    fn principal_label_prefers_name_and_email_together() {
        assert_eq!(
            principal_label("Bob Jones", Some("bob@example.com")),
            Some("Bob Jones <bob@example.com>".to_string())
        );
    }

    #[test]
    fn principal_label_falls_back_to_email_alone_with_no_name() {
        assert_eq!(
            principal_label("", Some("bob@example.com")),
            Some("bob@example.com".to_string())
        );
    }

    #[test]
    fn principal_label_falls_back_to_name_alone_with_no_email() {
        assert_eq!(
            principal_label("Bob Jones", None),
            Some("Bob Jones".to_string())
        );
    }

    #[test]
    fn principal_label_is_none_with_neither_name_nor_email() {
        assert_eq!(principal_label("", None), None);
        assert_eq!(principal_label("  ", Some("  ")), None);
    }
}
