// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The round trip of the Delegates page, over the [`crate::link`]
//! connection. [`load`] reads the list; [`revoke`] and [`grant`] are the
//! page's writes (ROADMAP item 164's Revoke and Add buttons): each sets
//! both delegation levels for one principal, then lists again. [`revoke`]
//! is [`grant`] with both levels at `None`.

use jmap_client::Client;
use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_PRINCIPALS};

use crate::delegation::{
    self, AddressBookDelegationLevel, CalendarDelegationLevel, Delegate, PrincipalSearch,
};
use crate::link::{self, AccountLink};

/// What the load found: the delegates, and whether the account's session
/// offers either capability `shareWith` delegation needs. A session with
/// neither is not an error; the page shows a status instead of a list.
pub struct Loaded {
    pub delegates: Vec<Delegate>,
    pub offered: bool,
}

/// The resolved capability accounts, and the delegate list built from them.
fn list(
    client: &Client,
    calendars_account: &Option<Id>,
    contacts_account: &Option<Id>,
) -> Result<Vec<Delegate>, jmap_client::Error> {
    let calendars = match calendars_account {
        Some(account_id) => client.calendars(account_id)?,
        None => Vec::new(),
    };
    let address_books = match contacts_account {
        Some(account_id) => client.address_books(account_id)?,
        None => Vec::new(),
    };
    Ok(delegation::list_delegates(&calendars, &address_books))
}

/// Lists every delegate the account's own calendars and address books name
/// in `shareWith` (`Calendar/get` and `AddressBook/get`, `ids: null`, on
/// whichever of the two capability accounts the session resolves; usually
/// the same account for both).
pub fn load(link: &AccountLink) -> Result<Loaded, String> {
    link.call(|client| {
        let session = client.session();
        let calendars_account = session
            .resolve_primary_account(CAPABILITY_CALENDARS)
            .cloned();
        let contacts_account = session
            .resolve_primary_account(CAPABILITY_CONTACTS)
            .cloned();
        let offered = calendars_account.is_some() || contacts_account.is_some();
        let delegates = list(client, &calendars_account, &contacts_account)?;
        Ok(Loaded { delegates, offered })
    })
    .map_err(|error| link::describe(&error))
}

/// Sets `principal_id`'s calendar and address book access to `calendar_level`
/// and `address_book_level` (on every one of the account's own calendars and
/// address books, the write side of [`crate::delegation::list_delegates`]'s
/// per-type highest-level read), then lists again so the page shows the
/// result without a second round trip.
pub fn grant(
    link: &AccountLink,
    principal_id: &Id,
    calendar_level: CalendarDelegationLevel,
    address_book_level: AddressBookDelegationLevel,
) -> Result<Loaded, String> {
    link.call(|client| {
        let session = client.session();
        let calendars_account = session
            .resolve_primary_account(CAPABILITY_CALENDARS)
            .cloned();
        let contacts_account = session
            .resolve_primary_account(CAPABILITY_CONTACTS)
            .cloned();
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
        let offered = calendars_account.is_some() || contacts_account.is_some();
        let delegates = list(client, &calendars_account, &contacts_account)?;
        Ok(Loaded { delegates, offered })
    })
    .map_err(|error| link::describe(&error))
}

/// Clears `principal_id`'s calendar and address book access. [`grant`] with
/// both levels at `None`.
pub fn revoke(link: &AccountLink, principal_id: &Id) -> Result<Loaded, String> {
    grant(
        link,
        principal_id,
        CalendarDelegationLevel::None,
        AddressBookDelegationLevel::None,
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

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;
    use jmap_proto::principals::Principal;

    use super::{grant, load, revoke, search};
    use crate::delegation::{
        AddressBookDelegationLevel, CalendarDelegationLevel, PrincipalSearch,
        commit_address_book_delegation, commit_calendar_delegation,
    };
    use crate::link::AccountLink;
    use crate::session_cache::AccountFeatures;

    fn link_to(server: &MockServer) -> AccountLink {
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features = AccountFeatures::from_session(client.session()).unwrap();
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
}
