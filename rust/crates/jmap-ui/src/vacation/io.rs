// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two round trips of the vacation page, over the [`crate::link`]
//! connection. The calls block, so use them on worker threads only.

use jmap_proto::mail::VacationResponse;
use serde_json::Value;

use crate::link::{self, AccountLink};

/// Gets the current autoresponder of the account (`VacationResponse/get`).
pub fn load(link: &AccountLink) -> Result<VacationResponse, String> {
    link.call(|client| client.vacation_response_get(&link.features.account_id))
        .map_err(|error| link::describe(&error))
}

/// Writes the page state to the server (`VacationResponse/set` update).
pub fn save(link: &AccountLink, patch: Value) -> Result<(), String> {
    link.call(|client| client.vacation_response_update(&link.features.account_id, patch.clone()))
        .map(|_| ())
        .map_err(|error| link::describe(&error))
}

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;
    use jmap_proto::mail::VacationResponse;
    use serde_json::json;

    use super::{load, save};
    use crate::link::AccountLink;
    use crate::session_cache::AccountFeatures;

    fn link_to(server: &MockServer) -> AccountLink {
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features = AccountFeatures::from_session(client.session()).unwrap();
        AccountLink::without_refresh(client, features)
    }

    /// The singleton of the account on `server`.
    fn singleton(server: &MockServer) -> VacationResponse {
        let state = server.state();
        let state = state.lock().unwrap();
        let account = state.account(&server.account_id()).unwrap();
        account
            .vacation_response
            .get(&Id::new("singleton"))
            .unwrap()
            .clone()
    }

    #[test]
    fn load_returns_the_autoresponder_of_the_server() {
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            let vacation = account
                .vacation_response
                .get_mut(&Id::new("singleton"))
                .unwrap();
            vacation.is_enabled = true;
            vacation.subject = Some("Away".into());
        }

        let loaded = load(&link_to(&server)).unwrap();

        assert!(loaded.is_enabled);
        assert_eq!(loaded.subject.as_deref(), Some("Away"));
    }

    #[test]
    fn save_writes_the_patch_to_the_server() {
        let server = MockServer::builder().start();

        let patch = json!({"isEnabled": true, "subject": "Away"});
        assert_eq!(save(&link_to(&server), patch), Ok(()));

        let stored = singleton(&server);
        assert!(stored.is_enabled);
        assert_eq!(stored.subject.as_deref(), Some("Away"));
    }

    #[test]
    fn save_reports_a_patch_that_the_server_refuses() {
        let server = MockServer::builder().start();

        assert!(save(&link_to(&server), json!({"isEnabled": "yes"})).is_err());
        assert!(!singleton(&server).is_enabled);
    }
}
