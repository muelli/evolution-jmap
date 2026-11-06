// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which other accounts share mail with this login.
//!
//! A session can list more than one account with `urn:ietf:params:jmap:mail`:
//! the user's own personal account, and zero or more accounts another user
//! shared mailboxes from (RFC 8620 §1.6.2, `isPersonal: false`). `MailSync`
//! is scoped to one account id. This module finds the other mail-capable
//! accounts a `CamelJmapStore` must also merge into its folder tree, the
//! same way `jmap_collection_sync::CollectionLayout::shared_accounts` finds
//! shared address books and calendars for the collection backend. That
//! function deliberately leaves a mail-only share out of scope, since the
//! collection backend never builds a mail child; this module is mail's own
//! equivalent, kept in `jmap-mail-sync` so the merge logic it feeds stays
//! next to the rest of this crate's account-scoped sync code.

use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_MAIL, Session};

/// An account that shares mail with this login, other than the account the
/// mail store itself is configured for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedMailAccount {
    /// The account id every `MailSync` call for this share carries.
    pub id: Id,
    /// The owner's display name (RFC 8620 §1.6.2), used to label the subtree.
    pub name: String,
    /// Whether the user may create, change or delete anything in this
    /// account. A mailbox's own `myRights` can be stricter than this; this
    /// flag alone does not decide whether one folder inside the account is
    /// writable.
    pub read_only: bool,
}

/// The accounts shared with this login that offer mail, other than the
/// account already resolved for mail itself.
///
/// Only accounts with `isPersonal: false`: a second personal account is not
/// a share. Mail is offered only when the server and the account both list
/// `urn:ietf:params:jmap:mail`, and the account is not the resolved primary
/// mail account. [`Session::resolve_primary_account`] already returns a
/// shared account there when the server deliberately makes one primary, so
/// excluding it here avoids listing that account twice.
pub fn shared_mail_accounts(session: &Session) -> Vec<SharedMailAccount> {
    if !session.capabilities.contains_key(CAPABILITY_MAIL) {
        return Vec::new();
    }
    let primary = session.resolve_primary_account(CAPABILITY_MAIL);
    session
        .accounts
        .iter()
        .filter(|(id, account)| {
            !account.is_personal && account.has_capability(CAPABILITY_MAIL) && Some(*id) != primary
        })
        .map(|(id, account)| SharedMailAccount {
            id: id.clone(),
            name: account.name.clone(),
            read_only: account.is_read_only,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use jmap_proto::State;
    use jmap_proto::session::Account;
    use serde_json::json;

    fn session() -> Session {
        Session::new(
            "vera",
            "https://example.test/api",
            "https://example.test/download/{accountId}/{blobId}/{name}?type={type}",
            "https://example.test/upload/{accountId}",
            State::new("s1"),
        )
        .with_capability(CAPABILITY_MAIL, json!({}))
    }

    fn mail_account(name: &str) -> Account {
        Account::new(name).with_capability(CAPABILITY_MAIL, json!({}))
    }

    #[test]
    fn a_shared_mail_account_is_listed() {
        let session = session()
            .with_account("a", mail_account("Vera").is_personal(true))
            .with_account("b", mail_account("bob@example.com").is_read_only(true));

        assert_eq!(
            shared_mail_accounts(&session),
            [SharedMailAccount {
                id: Id::new("b"),
                name: "bob@example.com".to_owned(),
                read_only: true,
            }]
        );
    }

    #[test]
    fn a_personal_account_is_never_a_shared_one() {
        let session = session()
            .with_account("a", mail_account("Vera").is_personal(true))
            .with_account("b", mail_account("Vera at work").is_personal(true));

        assert_eq!(shared_mail_accounts(&session), []);
    }

    #[test]
    fn an_account_not_offering_mail_is_not_shared() {
        let session = session().with_account("b", Account::new("bob@example.com"));

        assert_eq!(shared_mail_accounts(&session), []);
    }

    #[test]
    fn the_server_must_offer_mail_too() {
        let session = Session::new(
            "vera",
            "https://example.test/api",
            "https://example.test/download/{accountId}/{blobId}/{name}?type={type}",
            "https://example.test/upload/{accountId}",
            State::new("s1"),
        )
        .with_account("b", mail_account("bob@example.com"));

        assert_eq!(
            shared_mail_accounts(&session),
            [],
            "the server never lists the mail capability at all"
        );
    }

    #[test]
    fn a_shared_account_promoted_to_primary_is_not_listed_twice() {
        let session = session()
            .with_account("s", mail_account("Support"))
            .with_primary_account(CAPABILITY_MAIL, "s");

        assert_eq!(
            shared_mail_accounts(&session),
            [],
            "it is already the layout's own mail account, not a share"
        );
    }
}
