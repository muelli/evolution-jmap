// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The round trip of the Delegates page, over the [`crate::link`]
//! connection. Read-only: this page has no submit yet (ROADMAP item 164's
//! page skeleton).

use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CONTACTS};

use crate::delegation::{self, Delegate};
use crate::link::{self, AccountLink};

/// What the load found: the delegates, and whether the account's session
/// offers either capability `shareWith` delegation needs. A session with
/// neither is not an error; the page shows a status instead of a list.
pub struct Loaded {
    pub delegates: Vec<Delegate>,
    pub offered: bool,
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
        let calendars = match &calendars_account {
            Some(account_id) => client.calendars(account_id)?,
            None => Vec::new(),
        };
        let address_books = match &contacts_account {
            Some(account_id) => client.address_books(account_id)?,
            None => Vec::new(),
        };
        Ok(Loaded {
            delegates: delegation::list_delegates(&calendars, &address_books),
            offered,
        })
    })
    .map_err(|error| link::describe(&error))
}

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;

    use super::load;
    use crate::delegation::{
        AddressBookDelegationLevel, CalendarDelegationLevel, commit_address_book_delegation,
        commit_calendar_delegation,
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
}
