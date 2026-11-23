// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The round trip of the Delegates page, over the [`crate::link`]
//! connection. [`load`] reads the list; [`revoke`] and [`grant`] are the
//! page's writes (ROADMAP item 164's Revoke and Add buttons): each sets
//! both delegation levels for one principal, then lists again. [`revoke`]
//! is [`grant`] with both levels at `None`.

use std::collections::BTreeMap;

use jmap_backend_core::i18n::translate;
use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_MAIL, CAPABILITY_PRINCIPALS,
};

use crate::delegation::{
    self, AddressBookDelegationLevel, CalendarDelegationLevel, Delegate, MailDelegationLevel,
    PrincipalSearch,
};
use crate::link::{self, AccountLink};

/// What the load found: the delegates, whether the account's session offers
/// either capability `shareWith` delegation needs, and a best-effort
/// principal id to display label map (item 179(f)). A session with neither
/// capability is not an error; the page shows a status instead of a list. A
/// principal id missing from `labels` (no principals capability, or the
/// server could not resolve it) falls back to showing the raw id.
pub struct Loaded {
    pub delegates: Vec<Delegate>,
    pub offered: bool,
    pub labels: BTreeMap<Id, String>,
}

/// Best-effort name/email labels for `delegates`' principal ids, on the
/// session's principals capability account. Returns an empty map when the
/// session offers no such account or there is nothing to resolve.
fn resolve_labels(client: &Client, delegates: &[Delegate]) -> BTreeMap<Id, String> {
    if delegates.is_empty() {
        return BTreeMap::new();
    }
    let Some(account_id) = client
        .session()
        .resolve_primary_account(CAPABILITY_PRINCIPALS)
        .cloned()
    else {
        return BTreeMap::new();
    };
    let ids: Vec<Id> = delegates.iter().map(|d| d.principal_id.clone()).collect();
    delegation::resolve_principal_labels(client, &account_id, &ids)
}

/// The resolved capability accounts, and the delegate list built from them.
fn list(
    client: &Client,
    calendars_account: &Option<Id>,
    contacts_account: &Option<Id>,
    mail_account: &Option<Id>,
) -> Result<Vec<Delegate>, jmap_client::Error> {
    let calendars = match calendars_account {
        Some(account_id) => client.calendars(account_id)?,
        None => Vec::new(),
    };
    let address_books = match contacts_account {
        Some(account_id) => client.address_books(account_id)?,
        None => Vec::new(),
    };
    let mailboxes = match mail_account {
        Some(account_id) => client.mailbox_get(account_id)?.list,
        None => Vec::new(),
    };
    Ok(delegation::list_delegates(
        &calendars,
        &address_books,
        &mailboxes,
    ))
}

/// Lists every delegate the account's own calendars, address books and
/// Inbox subtree name in `shareWith` (`Calendar/get`, `AddressBook/get` and
/// `Mailbox/get`, `ids: null`, on whichever of the three capability
/// accounts the session resolves; usually the same account for all three).
pub fn load(link: &AccountLink) -> Result<Loaded, String> {
    link.call(|client| {
        let session = client.session();
        let calendars_account = session
            .resolve_primary_account(CAPABILITY_CALENDARS)
            .cloned();
        let contacts_account = session
            .resolve_primary_account(CAPABILITY_CONTACTS)
            .cloned();
        let mail_account = session.resolve_primary_account(CAPABILITY_MAIL).cloned();
        let offered =
            calendars_account.is_some() || contacts_account.is_some() || mail_account.is_some();
        let delegates = list(client, &calendars_account, &contacts_account, &mail_account)?;
        let labels = resolve_labels(client, &delegates);
        Ok(Loaded {
            delegates,
            offered,
            labels,
        })
    })
    .map_err(|error| link::describe(&error))
}

/// Sets `principal_id`'s calendar, address book and mail access to
/// `calendar_level`, `address_book_level` and `mail_level` (on every one of
/// the account's own calendars and address books, and the Inbox subtree for
/// mail, the write side of [`crate::delegation::list_delegates`]'s per-type
/// highest-level read), then lists again so the page shows the result
/// without a second round trip.
pub fn grant(
    link: &AccountLink,
    principal_id: &Id,
    calendar_level: CalendarDelegationLevel,
    address_book_level: AddressBookDelegationLevel,
    mail_level: MailDelegationLevel,
) -> Result<Loaded, String> {
    link.call(|client| {
        let session = client.session();
        let calendars_account = session
            .resolve_primary_account(CAPABILITY_CALENDARS)
            .cloned();
        let contacts_account = session
            .resolve_primary_account(CAPABILITY_CONTACTS)
            .cloned();
        let mail_account = session.resolve_primary_account(CAPABILITY_MAIL).cloned();
        if let Some(account_id) = &calendars_account {
            delegation::commit_calendar_delegation(
                client,
                account_id,
                principal_id,
                calendar_level,
            )?;
        }
        if let Some(account_id) = &contacts_account {
            delegation::commit_address_book_delegation(
                client,
                account_id,
                principal_id,
                address_book_level,
            )?;
        }
        if let Some(account_id) = &mail_account {
            delegation::commit_mail_delegation(client, account_id, principal_id, mail_level)?;
        }
        let offered =
            calendars_account.is_some() || contacts_account.is_some() || mail_account.is_some();
        let delegates = list(client, &calendars_account, &contacts_account, &mail_account)?;
        let labels = resolve_labels(client, &delegates);
        Ok(Loaded {
            delegates,
            offered,
            labels,
        })
    })
    .map_err(|error| link::describe(&error))
}

/// Clears `principal_id`'s calendar, address book and mail access. [`grant`]
/// with every level at `None`.
pub fn revoke(link: &AccountLink, principal_id: &Id) -> Result<Loaded, String> {
    grant(
        link,
        principal_id,
        CalendarDelegationLevel::None,
        AddressBookDelegationLevel::None,
        MailDelegationLevel::None,
    )
}

/// Looks up principals matching `text` for the Add dialog's picker
/// (`crate::delegation::search_principals`, on the account the session
/// resolves for `urn:ietf:params:jmap:principals`). A session with no such
/// account answers the same `QueryRefused` fallback as a server that
/// refuses the query, so the dialog always has one fallback to show.
pub fn search(link: &AccountLink, text: &str) -> Result<PrincipalSearch, String> {
    link.call(|client| {
        let session = client.session();
        let Some(account_id) = session
            .resolve_primary_account(CAPABILITY_PRINCIPALS)
            .cloned()
        else {
            return Ok(PrincipalSearch::QueryRefused);
        };
        delegation::search_principals(client, &account_id, text)
    })
    .map_err(|error| link::describe(&error))
}

/// Resolves what the Add dialog's principal field holds to a principal id
/// (item 185(b)). The field used to take only a principal id; typing an
/// address into it failed the write with the server's "Invalid account id".
/// Anything with no `@` is already an id, the manual-entry fallback item 164
/// shipped with; anything with one is looked up by [`search`] and resolved
/// to the one principal whose email matches it exactly.
pub fn resolve_principal(link: &AccountLink, text: &str) -> Result<Id, String> {
    if !text.contains('@') {
        return Ok(Id::new(text));
    }
    match search(link, text)? {
        PrincipalSearch::QueryRefused => Err(translate(
            c"this server cannot look up an address; use Find to search for the principal",
        )),
        PrincipalSearch::Found(principals) => pick_by_email(principals, text),
    }
}

/// Picks the one principal of `principals` whose email matches `email`
/// exactly, case-insensitively. Fails clearly on zero or on more than one
/// match, rather than guessing.
fn pick_by_email(
    principals: Vec<jmap_proto::principals::Principal>,
    email: &str,
) -> Result<Id, String> {
    let mut matches = principals.into_iter().filter(|principal| {
        principal
            .email
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(email))
    });
    let Some(first) = matches.next() else {
        return Err(translate(
            c"no principal has this address; use Find to search by name",
        ));
    };
    if matches.next().is_some() {
        return Err(translate(
            c"more than one principal has this address; use Find to pick one",
        ));
    }
    first
        .id
        .ok_or_else(|| translate(c"the server did not send an id for this principal"))
}

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;
    use jmap_proto::principals::Principal;

    use super::{grant, load, resolve_principal, revoke, search};
    use crate::delegation::{
        AddressBookDelegationLevel, CalendarDelegationLevel, MailDelegationLevel, PrincipalSearch,
        commit_address_book_delegation, commit_calendar_delegation, commit_mail_delegation,
    };
    use crate::link::AccountLink;
    use crate::session_cache::AccountFeatures;

    fn link_to(server: &MockServer) -> AccountLink {
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features = AccountFeatures::from_session(&client.session()).unwrap();
        AccountLink::without_refresh(client, features)
    }

    #[test]
    fn load_lists_no_delegates_on_a_fresh_account() {
        let server = MockServer::builder().start();
        let loaded = load(&link_to(&server)).unwrap();
        assert!(loaded.offered);
        assert_eq!(loaded.delegates, vec![]);
    }

    #[test]
    fn load_lists_a_delegate_committed_across_the_accounts_own_collections() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
            account.seed_address_book("Contacts", false);
        }
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Author)
            .unwrap();
        commit_address_book_delegation(
            &alice,
            &account_id,
            &bob,
            AddressBookDelegationLevel::Reviewer,
        )
        .unwrap();

        let loaded = load(&link_to(&server)).unwrap();

        assert_eq!(loaded.delegates.len(), 1);
        assert_eq!(loaded.delegates[0].principal_id, bob);
        assert_eq!(
            loaded.delegates[0].calendar_level,
            CalendarDelegationLevel::Author
        );
        assert_eq!(
            loaded.delegates[0].address_book_level,
            AddressBookDelegationLevel::Reviewer
        );
    }

    #[test]
    fn load_resolves_a_delegates_name_and_email_from_the_server() {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let bob = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
            account.seed_principal(Principal::new("Bob Jones").with_email("bob@example.com"))
        };
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Author)
            .unwrap();

        let loaded = load(&link_to(&server)).unwrap();

        assert_eq!(
            loaded.labels.get(&bob).map(String::as_str),
            Some("Bob Jones <bob@example.com>")
        );
    }

    #[test]
    fn load_leaves_an_unresolvable_principal_out_of_the_labels_map() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
        }
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Author)
            .unwrap();

        let loaded = load(&link_to(&server)).unwrap();

        assert_eq!(loaded.delegates.len(), 1, "the delegate itself still lists");
        assert!(
            !loaded.labels.contains_key(&bob),
            "no principal record exists for this id, so no label"
        );
    }

    #[test]
    fn load_resolves_a_mail_share_on_the_inbox_subtree() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
        }
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_mail_delegation(&alice, &account_id, &bob, MailDelegationLevel::Author).unwrap();

        let loaded = load(&link_to(&server)).unwrap();

        assert_eq!(loaded.delegates.len(), 1);
        assert_eq!(loaded.delegates[0].principal_id, bob);
        assert_eq!(loaded.delegates[0].mail_level, MailDelegationLevel::Author);
    }

    #[test]
    fn load_is_offered_from_mail_alone_with_neither_calendars_nor_contacts() {
        let server = MockServer::builder()
            .without_capability(jmap_proto::session::CAPABILITY_CALENDARS)
            .without_capability(jmap_proto::session::CAPABILITY_CONTACTS)
            .start();

        let loaded = load(&link_to(&server)).unwrap();

        assert!(
            loaded.offered,
            "mail delegation alone still offers the page"
        );
        assert_eq!(loaded.delegates, vec![]);
    }

    #[test]
    fn revoke_clears_both_levels_and_the_delegate_drops_from_the_list() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
            account.seed_address_book("Contacts", false);
        }
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Editor)
            .unwrap();
        commit_address_book_delegation(
            &alice,
            &account_id,
            &bob,
            AddressBookDelegationLevel::Author,
        )
        .unwrap();
        assert_eq!(load(&link_to(&server)).unwrap().delegates.len(), 1);

        let loaded = revoke(&link_to(&server), &bob).unwrap();

        assert_eq!(loaded.delegates, vec![], "bob should no longer be listed");
        assert_eq!(load(&link_to(&server)).unwrap().delegates, vec![]);
    }

    #[test]
    fn grant_adds_a_delegate_with_the_chosen_levels() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
            account.seed_address_book("Contacts", false);
        }
        assert_eq!(load(&link_to(&server)).unwrap().delegates, vec![]);

        let loaded = grant(
            &link_to(&server),
            &bob,
            CalendarDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
            MailDelegationLevel::None,
        )
        .unwrap();

        assert_eq!(loaded.delegates.len(), 1);
        assert_eq!(loaded.delegates[0].principal_id, bob);
        assert_eq!(
            loaded.delegates[0].calendar_level,
            CalendarDelegationLevel::Reviewer
        );
        assert_eq!(
            loaded.delegates[0].address_book_level,
            AddressBookDelegationLevel::Author
        );
        assert_eq!(load(&link_to(&server)).unwrap().delegates.len(), 1);
    }

    #[test]
    fn grant_writes_a_mail_share_on_the_inbox_subtree() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
        }
        assert_eq!(load(&link_to(&server)).unwrap().delegates, vec![]);

        let loaded = grant(
            &link_to(&server),
            &bob,
            CalendarDelegationLevel::None,
            AddressBookDelegationLevel::None,
            MailDelegationLevel::Author,
        )
        .unwrap();

        assert_eq!(loaded.delegates.len(), 1);
        assert_eq!(loaded.delegates[0].principal_id, bob);
        assert_eq!(loaded.delegates[0].mail_level, MailDelegationLevel::Author);
        assert_eq!(
            load(&link_to(&server)).unwrap().delegates[0].mail_level,
            MailDelegationLevel::Author
        );
    }

    #[test]
    fn grant_changes_an_existing_delegates_levels() {
        let bob = Id::new("P-bob");
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
        }
        let alice = Client::connect(server.origin(), Credentials::none()).unwrap();
        commit_calendar_delegation(&alice, &account_id, &bob, CalendarDelegationLevel::Reviewer)
            .unwrap();

        let loaded = grant(
            &link_to(&server),
            &bob,
            CalendarDelegationLevel::Editor,
            AddressBookDelegationLevel::None,
            MailDelegationLevel::None,
        )
        .unwrap();

        assert_eq!(
            loaded.delegates[0].calendar_level,
            CalendarDelegationLevel::Editor
        );
    }

    #[test]
    fn revoking_an_unknown_principal_is_not_an_error() {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Work", false);
        }
        let loaded = revoke(&link_to(&server), &Id::new("P-nobody")).unwrap();
        assert_eq!(loaded.delegates, vec![]);
    }

    #[test]
    fn search_finds_a_seeded_principal_by_name() {
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&server.account_id())
                .unwrap()
                .seed_principal(Principal::new("Bob Jones").with_email("bob@example.com"));
        }

        let found = search(&link_to(&server), "Bob").unwrap();

        let PrincipalSearch::Found(principals) = found else {
            panic!("expected Found, got {found:?}");
        };
        assert_eq!(principals.len(), 1);
        assert_eq!(principals[0].name, "Bob Jones");
    }

    #[test]
    fn search_falls_back_when_the_server_refuses_the_query() {
        let server = MockServer::builder().disable_principal_query().start();

        let found = search(&link_to(&server), "anything").unwrap();

        assert_eq!(found, PrincipalSearch::QueryRefused);
    }

    #[test]
    fn resolve_principal_leaves_a_plain_id_unchanged() {
        let server = MockServer::builder().start();

        let resolved = resolve_principal(&link_to(&server), "P-bob").unwrap();

        assert_eq!(resolved, Id::new("P-bob"));
    }

    #[test]
    fn resolve_principal_looks_up_a_single_matching_address() {
        let server = MockServer::builder().start();
        let bob = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&server.account_id())
                .unwrap()
                .seed_principal(Principal::new("Bob Jones").with_email("bob@example.com"))
        };

        let resolved = resolve_principal(&link_to(&server), "bob@example.com").unwrap();

        assert_eq!(resolved, bob);
    }

    #[test]
    fn pick_by_email_matches_case_insensitively() {
        let bob = Id::new("P-bob");
        let principals = vec![
            Principal::new("Bob Jones")
                .with_email("bob@example.com")
                .with_id(bob.clone()),
        ];

        let resolved = super::pick_by_email(principals, "Bob@Example.com").unwrap();

        assert_eq!(resolved, bob);
    }

    #[test]
    fn resolve_principal_fails_with_no_match() {
        let server = MockServer::builder().start();

        let error = resolve_principal(&link_to(&server), "nobody@example.com").unwrap_err();

        assert!(error.contains("no principal"), "{error}");
    }

    #[test]
    fn resolve_principal_fails_with_more_than_one_match() {
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            account.seed_principal(Principal::new("Bob Jones").with_email("bob@example.com"));
            account.seed_principal(Principal::new("Bob Smith").with_email("bob@example.com"));
        }

        let error = resolve_principal(&link_to(&server), "bob@example.com").unwrap_err();

        assert!(error.contains("more than one"), "{error}");
    }

    #[test]
    fn resolve_principal_fails_clearly_when_the_server_refuses_the_query() {
        let server = MockServer::builder().disable_principal_query().start();

        let error = resolve_principal(&link_to(&server), "bob@example.com").unwrap_err();

        assert!(error.contains("Find"), "{error}");
    }
}
