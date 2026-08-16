// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which JMAP account serves which of Evolution's three services.
//!
//! The session document has two statements about capabilities and they are not
//! the same statement:
//!
//! - `capabilities` is what the *server* implements. RFC 8620 §3.3 has every
//!   request name the capabilities it uses, and a server answers a `using` it
//!   does not advertise with `unknownCapability` — which fails the whole
//!   request, not the one call.
//! - `accountCapabilities` is what *this account* offers. The methods are
//!   per-account, so an account that does not list `…:jmap:mail` has no
//!   mailboxes to list however thoroughly the server implements mail.
//!
//! So a service is offered here only when both say so. Trusting either alone
//! produces a child source that cannot work: the account's word alone gives a
//! mail folder whose every refresh is an `unknownCapability`, and the server's
//! word alone gives one whose every refresh is an `accountNotFound`.
//!
//! ## Which account
//!
//! `primaryAccounts` is the server's own answer to "which account is this
//! user's mail" (RFC 8620 §2), and it is taken as given wherever it has an
//! entry — including when it names an account that is not the user's own,
//! because a server that designates a shared account as primary has said
//! something deliberate.
//!
//! Where it has no entry for a capability, the account is inferred, but only
//! from a position where there is nothing to guess: exactly one account
//! belonging to this user (`isPersonal`) offers the capability. Two of them and
//! the answer is none — an account fanned out to the wrong mailbox is worse
//! than one that reports it could not tell, because the user cannot see which
//! JMAP account a folder came from and would have to be told by us.
//!
//! ## Sending
//!
//! `urn:ietf:params:jmap:submission` is its own capability with its own
//! `primaryAccounts` entry, but a transport is only warranted when it resolves
//! to the *same* account as mail. An `EmailSubmission` names an `emailId`
//! (RFC 8621 §7) and ids are scoped to an account, so submitting through
//! account B a message uploaded into account A is not a thing the protocol can
//! express. A different submission account is therefore not a second transport
//! to offer — it is no transport at all.

use jmap_proto::Id;
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_MAIL, CAPABILITY_SUBMISSION, Session,
};

use crate::children::ChildKind;

/// The JMAP account behind one of Evolution's services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccount {
    /// The `accountId` every method call for this service carries.
    pub id: Id,
    /// The account's name as the server states it (RFC 8620 §1.6.2) — a
    /// display string, and the obvious default name for the child source.
    pub name: String,
    /// The account's `isReadOnly`: the whole data set is read-only, not one
    /// collection in it. A child made from this may be shown, but nothing in it
    /// may be created, changed or deleted.
    pub read_only: bool,
}

/// The mail account, and whether it can also send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailService {
    pub account: ServiceAccount,
    /// Whether the account also offers `urn:ietf:params:jmap:submission`, i.e.
    /// whether the collection warrants a Camel *transport* beside its store.
    /// False is a receive-only account, which is a usable account.
    pub can_send: bool,
}

/// The children one JMAP login warrants.
///
/// Each field is `None` when the login offers that service at all — not when
/// the user switched it off. Whether a source the user disabled is created is
/// EDS's business (`ESourceCollection:mail-enabled` and its two siblings); what
/// is here is what *exists* to enable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionLayout {
    pub mail: Option<MailService>,
    pub contacts: Option<ServiceAccount>,
    pub calendars: Option<ServiceAccount>,
}

impl CollectionLayout {
    /// Reads the session document for what this login offers.
    pub fn from_session(session: &Session) -> Self {
        let mail = service(session, CAPABILITY_MAIL).map(|account| {
            let can_send = service(session, CAPABILITY_SUBMISSION)
                .is_some_and(|submission| submission.id == account.id);
            MailService { account, can_send }
        });

        Self {
            mail,
            contacts: service(session, CAPABILITY_CONTACTS),
            calendars: service(session, CAPABILITY_CALENDARS),
        }
    }

    /// Whether the login offers nothing at all.
    ///
    /// A login can authenticate successfully and still have no service this
    /// backend can use — a server implementing only `…:jmap:core`, or an
    /// account with every capability stripped. That is a sentence for the user,
    /// not an empty account tree to leave them puzzling over.
    pub fn is_empty(&self) -> bool {
        self.mail.is_none() && self.contacts.is_none() && self.calendars.is_none()
    }

    /// The account whose collections of `kind` this login's children come from,
    /// if it has one.
    pub fn account_for(&self, kind: ChildKind) -> Option<&ServiceAccount> {
        match kind {
            ChildKind::AddressBook => self.contacts.as_ref(),
            ChildKind::Calendar => self.calendars.as_ref(),
        }
    }

    /// Whether the login offers collections of `kind` at all — which is a
    /// different question from whether the user wants them (see
    /// [`Parts`](crate::Parts)) and from whether the account holds any.
    pub fn serves(&self, kind: ChildKind) -> bool {
        self.account_for(kind).is_some()
    }
}

/// The account serving `capability`, if this login has one.
///
/// The inference (server capability, then `primaryAccounts`, then the sole
/// personal account offering it) lives on
/// [`Session::resolve_primary_account`] itself now, shared with
/// [`jmap_client::Client::primary_account`] — this just attaches the name and
/// read-only bit the child source needs.
fn service(session: &Session, capability: &str) -> Option<ServiceAccount> {
    let id = session.resolve_primary_account(capability)?.clone();
    let account = session.accounts.get(&id)?;
    Some(ServiceAccount {
        id,
        name: account.name.clone(),
        read_only: account.is_read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::{Value, json};

    /// A session document with the given server capabilities, accounts and
    /// `primaryAccounts` map, and the required properties around them filled
    /// with something plausible.
    fn session(capabilities: &[&str], accounts: Value, primary_accounts: Value) -> Session {
        let capabilities: serde_json::Map<String, Value> = capabilities
            .iter()
            .map(|urn| ((*urn).to_owned(), json!({})))
            .collect();
        serde_json::from_value(json!({
            "capabilities": capabilities,
            "accounts": accounts,
            "primaryAccounts": primary_accounts,
            "username": "vera@example.com",
            "apiUrl": "https://jmap.example.com/jmap",
            "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}",
            "uploadUrl": "https://jmap.example.com/upload/{accountId}",
            "eventSourceUrl": "https://jmap.example.com/eventsource",
            "state": "s0",
        }))
        .expect("a session document jmap-proto can read")
    }

    fn account(name: &str, capabilities: &[&str]) -> Value {
        json!({
            "name": name,
            "isPersonal": true,
            "isReadOnly": false,
            "accountCapabilities": capabilities
                .iter()
                .map(|urn| ((*urn).to_owned(), json!({})))
                .collect::<serde_json::Map<String, Value>>(),
        })
    }

    const ALL: &[&str] = &[
        CAPABILITY_MAIL,
        CAPABILITY_SUBMISSION,
        CAPABILITY_CONTACTS,
        CAPABILITY_CALENDARS,
    ];

    #[test]
    fn a_capability_the_server_does_not_implement_is_not_offered_however_the_account_answers() {
        // The account claims all four; the server advertises only mail. A
        // `using` naming contacts would be answered with unknownCapability, so
        // there is no address book here — and no calendar either.
        let layout = CollectionLayout::from_session(&session(
            &[CAPABILITY_MAIL],
            json!({"a": account("Vera", ALL)}),
            json!({CAPABILITY_MAIL: "a", CAPABILITY_CONTACTS: "a", CAPABILITY_CALENDARS: "a"}),
        ));

        assert_eq!(layout.mail.as_ref().unwrap().account.id, Id::new("a"));
        assert!(!layout.mail.as_ref().unwrap().can_send);
        assert_eq!(layout.contacts, None);
        assert_eq!(layout.calendars, None);
    }

    #[test]
    fn a_capability_the_account_does_not_offer_is_not_offered_however_the_server_answers() {
        let layout = CollectionLayout::from_session(&session(
            ALL,
            json!({"a": account("Vera", &[CAPABILITY_MAIL, CAPABILITY_SUBMISSION])}),
            json!({CAPABILITY_MAIL: "a", CAPABILITY_SUBMISSION: "a", CAPABILITY_CONTACTS: "a"}),
        ));

        assert!(layout.mail.as_ref().unwrap().can_send);
        assert_eq!(layout.contacts, None);
    }

    #[test]
    fn a_primary_account_that_is_not_in_the_document_offers_nothing() {
        let layout = CollectionLayout::from_session(&session(
            ALL,
            json!({"a": account("Vera", ALL)}),
            json!({CAPABILITY_MAIL: "gone"}),
        ));

        assert_eq!(layout.mail, None);
        // The other two have no primary entry and one personal candidate, so
        // they are found the other way — the point being that the mail failure
        // is about mail's own entry, not about the document being unusable.
        assert_eq!(layout.contacts.unwrap().id, Id::new("a"));
    }

    #[test]
    fn the_sole_personal_account_offering_a_capability_serves_it_unnamed() {
        let layout = CollectionLayout::from_session(&session(
            ALL,
            json!({"a": account("Vera", ALL)}),
            json!({}),
        ));

        let mail = layout.mail.unwrap();
        assert_eq!(mail.account.id, Id::new("a"));
        assert_eq!(mail.account.name, "Vera");
        assert!(mail.can_send);
        assert_eq!(layout.contacts.unwrap().id, Id::new("a"));
        assert_eq!(layout.calendars.unwrap().id, Id::new("a"));
    }

    #[test]
    fn two_personal_accounts_offering_a_capability_and_no_primary_entry_is_not_guessed_at() {
        let layout = CollectionLayout::from_session(&session(
            ALL,
            json!({"a": account("Vera", ALL), "b": account("Vera at work", ALL)}),
            json!({CAPABILITY_CONTACTS: "b"}),
        ));

        assert_eq!(
            layout.mail, None,
            "nothing says which mailbox is the user's"
        );
        assert_eq!(layout.calendars, None);
        // Named, so not guessed at: the server's own entry still decides.
        assert_eq!(layout.contacts.unwrap().id, Id::new("b"));
    }

    #[test]
    fn a_shared_account_is_not_inferred_but_is_honoured_when_named() {
        let mut shared = account("Support", ALL);
        shared["isPersonal"] = json!(false);
        let accounts = json!({"personal": account("Vera", ALL), "shared": shared});

        // Two accounts offer mail; only one of them is the user's, so there is
        // nothing to choose between and it serves.
        let inferred = CollectionLayout::from_session(&session(ALL, accounts.clone(), json!({})));
        assert_eq!(inferred.mail.unwrap().account.id, Id::new("personal"));

        // Named as primary, a shared account is what the server says it is.
        let named = CollectionLayout::from_session(&session(
            ALL,
            accounts,
            json!({CAPABILITY_MAIL: "shared", CAPABILITY_SUBMISSION: "shared"}),
        ));
        let mail = named.mail.unwrap();
        assert_eq!(mail.account.id, Id::new("shared"));
        assert!(mail.can_send);
    }

    #[test]
    fn submission_in_another_account_is_no_transport_at_all() {
        let layout = CollectionLayout::from_session(&session(
            ALL,
            json!({"a": account("Vera", ALL), "b": account("Vera at work", ALL)}),
            json!({CAPABILITY_MAIL: "a", CAPABILITY_SUBMISSION: "b"}),
        ));

        let mail = layout.mail.unwrap();
        assert_eq!(mail.account.id, Id::new("a"));
        assert!(
            !mail.can_send,
            "an EmailSubmission cannot name an Email in another account"
        );
    }

    #[test]
    fn a_read_only_account_is_reported_as_one() {
        let mut locked = account("Archive", ALL);
        locked["isReadOnly"] = json!(true);
        let layout = CollectionLayout::from_session(&session(ALL, json!({"a": locked}), json!({})));

        assert!(layout.mail.unwrap().account.read_only);
        assert!(layout.contacts.unwrap().read_only);
    }

    #[test]
    fn a_login_with_nothing_behind_it_is_empty_rather_than_three_broken_children() {
        let layout = CollectionLayout::from_session(&session(
            &[],
            json!({"a": account("Vera", &[])}),
            json!({}),
        ));

        assert!(layout.is_empty());
        assert!(
            !CollectionLayout::from_session(&session(
                ALL,
                json!({"a": account("Vera", ALL)}),
                json!({})
            ))
            .is_empty()
        );
    }
}
