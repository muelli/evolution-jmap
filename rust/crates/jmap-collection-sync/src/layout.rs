// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which JMAP account serves which of Evolution's three services.
//!
//! The session document makes two different statements about capabilities:
//!
//! - `capabilities` is what the *server* implements. RFC 8620 §3.3 makes each
//!   request name the capabilities it uses. A server answers a `using` that it
//!   does not advertise with `unknownCapability`, which fails the whole
//!   request, not only one call.
//! - `accountCapabilities` is what *this account* offers. Methods operate on
//!   one account, so an account that does not list `…:jmap:mail` has no
//!   mailboxes, even if the server implements mail.
//!
//! Thus this module offers a service only when both statements include it.
//! With only the account's statement, each refresh of the child source fails
//! with `unknownCapability`. With only the server's statement, each refresh
//! fails with `accountNotSupportedByMethod`.
//!
//! ## Which account
//!
//! `primaryAccounts` is the server's answer to "which account holds this
//! user's mail" (RFC 8620 §2). Where it has an entry, the layout uses that
//! account if the account lists the capability. This is also true when it is
//! not the user's own account: a server that makes a shared account primary
//! does so deliberately.
//!
//! Where it has no entry for a capability, the layout uses the account only if
//! exactly one personal account (`isPersonal`) offers the capability. If two
//! accounts offer it, the layout uses neither. A wrong account is worse than
//! no account, because the user cannot see which JMAP account a folder comes
//! from.
//!
//! ## Sending
//!
//! `urn:ietf:params:jmap:submission` is a separate capability with a separate
//! `primaryAccounts` entry. A transport is necessary only when it resolves to
//! the *same* account as mail. An `EmailSubmission` names an `emailId`
//! (RFC 8621 §7), and an id is valid in one account only. Thus the protocol
//! cannot send through account B a message that is in account A. A different
//! submission account gives no transport.

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
    /// The account's name from the server (RFC 8620 §1.6.2): a display string,
    /// and the default name for the child source.
    pub name: String,
    /// The account's `isReadOnly`: all data in the account is read-only, not
    /// only one collection. Evolution can show a child of this account, but
    /// cannot create, change or delete anything in it.
    pub read_only: bool,
}

/// The mail account, and whether it can also send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailService {
    pub account: ServiceAccount,
    /// Whether `urn:ietf:params:jmap:submission` resolves to this same
    /// account, so that the account has a Camel *transport* with its store.
    /// False is a receive-only account, which is a usable account.
    pub can_send: bool,
}

/// The services that one JMAP login offers.
///
/// Each field is `None` when the login does not offer that service. It is not
/// `None` when the user switched the service off: [`Parts`](crate::Parts) holds
/// that choice.
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
    /// A login can authenticate and still have no service that this backend
    /// can use: for example, a server that implements only `…:jmap:core`.
    /// Tell the user this, instead of showing an empty account.
    pub fn is_empty(&self) -> bool {
        self.mail.is_none() && self.contacts.is_none() && self.calendars.is_none()
    }

    /// The account that holds this login's collections of `kind`, if any.
    pub fn account_for(&self, kind: ChildKind) -> Option<&ServiceAccount> {
        match kind {
            ChildKind::AddressBook => self.contacts.as_ref(),
            ChildKind::Calendar => self.calendars.as_ref(),
        }
    }

    /// Whether the login offers collections of `kind`. This is not whether the
    /// user wants them (see [`Parts`](crate::Parts)), and not whether the
    /// account holds any.
    pub fn serves(&self, kind: ChildKind) -> bool {
        self.account_for(kind).is_some()
    }
}

/// An account shared with this login, and which collection kinds it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedService {
    /// The owner's account. Its `name` tells the user who owns it: RFC 8620
    /// §1.6.2 gives "the email address representing the owner" as an example.
    pub account: ServiceAccount,
    pub contacts: bool,
    pub calendars: bool,
}

impl SharedService {
    /// Whether the account offers collections of `kind`.
    pub fn offers(&self, kind: ChildKind) -> bool {
        match kind {
            ChildKind::AddressBook => self.contacts,
            ChildKind::Calendar => self.calendars,
        }
    }
}

impl CollectionLayout {
    /// The accounts shared with this login that offer contacts or calendars,
    /// in the session's account id order.
    ///
    /// Only accounts with `isPersonal: false`: a second personal account is not
    /// a share, and [`CollectionLayout::from_session`] selects the personal
    /// account. A kind is offered only when the server and the account both
    /// list its capability, and the account is not the resolved primary account
    /// for it.
    pub fn shared_accounts(session: &Session) -> Vec<SharedService> {
        let offers = |id: &Id, capability: &str| {
            session.capabilities.contains_key(capability)
                && session
                    .accounts
                    .get(id)
                    .is_some_and(|account| account.account_capabilities.contains_key(capability))
                && session.resolve_primary_account(capability) != Some(id)
        };
        session
            .accounts
            .iter()
            .filter(|(_, account)| !account.is_personal)
            .filter_map(|(id, account)| {
                let shared = SharedService {
                    account: ServiceAccount {
                        id: id.clone(),
                        name: account.name.clone(),
                        read_only: account.is_read_only,
                    },
                    contacts: offers(id, CAPABILITY_CONTACTS),
                    calendars: offers(id, CAPABILITY_CALENDARS),
                };
                (shared.contacts || shared.calendars).then_some(shared)
            })
            .collect()
    }
}

/// The account serving `capability`, if this login has one.
///
/// [`Session::resolve_primary_account`] selects the account (also for
/// [`jmap_client::Client::primary_account`]): the server must list the
/// capability, then `primaryAccounts` or the sole personal account offering it
/// gives the id, and that account must list the capability. This function
/// adds the name and the read-only flag.
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
        // The account lists all four; the server advertises only mail. A
        // `using` with contacts gets unknownCapability, so there is no address
        // book and no calendar.
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
        // Contacts has no primary entry and one personal account, so it
        // resolves. Only the mail entry fails, not the whole document.
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
        // The server's primary entry for contacts still decides.
        assert_eq!(layout.contacts.unwrap().id, Id::new("b"));
    }

    #[test]
    fn a_shared_account_is_not_inferred_but_is_honoured_when_named() {
        let mut shared = account("Support", ALL);
        shared["isPersonal"] = json!(false);
        let accounts = json!({"personal": account("Vera", ALL), "shared": shared});

        // Two accounts offer mail, but only one is personal, so it serves.
        let inferred = CollectionLayout::from_session(&session(ALL, accounts.clone(), json!({})));
        assert_eq!(inferred.mail.unwrap().account.id, Id::new("personal"));

        // When the server names a shared account as primary, it serves.
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
    fn a_shared_account_offering_calendars_and_contacts_is_listed_with_its_owner() {
        let mut shared = account("bob@example.com", ALL);
        shared["isPersonal"] = json!(false);
        shared["isReadOnly"] = json!(true);
        let accounts = json!({"a": account("Vera", ALL), "b": shared});

        assert_eq!(
            CollectionLayout::shared_accounts(&session(ALL, accounts, json!({}))),
            [SharedService {
                account: ServiceAccount {
                    id: Id::new("b"),
                    name: "bob@example.com".to_owned(),
                    read_only: true,
                },
                contacts: true,
                calendars: true,
            }]
        );
    }

    #[test]
    fn a_personal_account_is_never_a_shared_one() {
        // Two personal accounts and no primary entry: the layout uses neither,
        // and neither is a share.
        let accounts = json!({"a": account("Vera", ALL), "b": account("Vera at work", ALL)});
        assert_eq!(
            CollectionLayout::shared_accounts(&session(ALL, accounts, json!({}))),
            []
        );
    }

    #[test]
    fn a_shared_kind_needs_both_the_server_and_the_account_capability() {
        let mut calendars_only = account("bob@example.com", &[CAPABILITY_CALENDARS]);
        calendars_only["isPersonal"] = json!(false);
        let mut mail_only = account("carol@example.com", &[CAPABILITY_MAIL]);
        mail_only["isPersonal"] = json!(false);
        let accounts = json!({"a": account("Vera", ALL), "b": calendars_only, "c": mail_only});

        let shared = CollectionLayout::shared_accounts(&session(ALL, accounts.clone(), json!({})));
        assert_eq!(shared.len(), 1, "a mail-only share is out of scope");
        assert_eq!(shared[0].account.id, Id::new("b"));
        assert!(shared[0].offers(ChildKind::Calendar));
        assert!(!shared[0].offers(ChildKind::AddressBook));

        // The server does not implement calendars, so each calendar request
        // would get unknownCapability.
        assert_eq!(
            CollectionLayout::shared_accounts(&session(
                &[CAPABILITY_MAIL, CAPABILITY_CONTACTS],
                accounts,
                json!({})
            )),
            []
        );
    }

    #[test]
    fn a_shared_account_named_as_primary_is_not_listed_twice() {
        let mut shared = account("Support", ALL);
        shared["isPersonal"] = json!(false);
        let accounts = json!({"a": account("Vera", ALL), "s": shared});

        let named = session(ALL, accounts, json!({CAPABILITY_CALENDARS: "s"}));
        assert_eq!(
            CollectionLayout::from_session(&named).calendars.unwrap().id,
            Id::new("s")
        );
        let shared = CollectionLayout::shared_accounts(&named);
        assert_eq!(shared.len(), 1);
        assert!(
            !shared[0].calendars,
            "its calendars are already the layout's own"
        );
        assert!(shared[0].contacts);
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
