// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two round trips of the vacation page, over the [`crate::link`]
//! connection. The calls block, so use them on worker threads only.

use std::ffi::CStr;

use jmap_backend_core::i18n::{N_, translate};
use jmap_client::expect_updated;
use jmap_proto::mail::VacationResponse;
use jmap_proto::methods::SetRequest;
use jmap_proto::{Id, State};
use serde_json::Value;

use crate::link::{self, AccountLink};

/// The autoresponder as the server holds it, with the `state` of the
/// answer.
pub struct Loaded {
    pub response: VacationResponse,
    /// The `state` of the `VacationResponse/get` answer. A save sends it as
    /// `ifInState`, so that it does not overwrite a change of another
    /// program.
    pub state: State,
}

/// Gets the current autoresponder of the account (`VacationResponse/get`).
pub fn load(link: &AccountLink) -> Result<Loaded, String> {
    link.call(|client| client.vacation_response_get_with_state(&link.features.account_id))
        .map(|(response, state)| Loaded { response, state })
        .map_err(|error| link::describe(&error))
}

/// The answer to a save after another program changed the autoresponder.
pub const SETTINGS_CHANGED: &CStr = N_(
    c"Another program changed the autoresponder after this page loaded it. Close the account editor, open it again and make the change again.",
);

/// Writes the page state to the server (`VacationResponse/set` update),
/// guarded by `if_in_state` (RFC 8620 Section 5.3) when the page loaded one.
pub fn save(link: &AccountLink, patch: Value, if_in_state: Option<State>) -> Result<(), String> {
    let account = link.features.account_id.clone();
    let id: Id = "singleton".into();
    let mut request = SetRequest::<VacationResponse>::new(account).update(id.clone(), patch);
    if let Some(state) = if_in_state {
        request = request.if_in_state(state);
    }
    link.call(|client| client.vacation_response_set(&request))
        .map_err(|error| refused(&error))
        .and_then(|response| expect_updated(&response, &id).map_err(|error| refused(&error)))
}

/// The message for a refused `VacationResponse/set`.
fn refused(error: &jmap_client::Error) -> String {
    if error.is_state_mismatch() {
        translate(SETTINGS_CHANGED)
    } else {
        link::describe(error)
    }
}

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;
    use jmap_proto::mail::VacationResponse;
    use serde_json::json;

    use super::{SETTINGS_CHANGED, load, save, translate};
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

        assert!(loaded.response.is_enabled);
        assert_eq!(loaded.response.subject.as_deref(), Some("Away"));
    }

    #[test]
    fn save_writes_the_patch_to_the_server() {
        let server = MockServer::builder().start();

        let patch = json!({"isEnabled": true, "subject": "Away"});
        assert_eq!(save(&link_to(&server), patch, None), Ok(()));

        let stored = singleton(&server);
        assert!(stored.is_enabled);
        assert_eq!(stored.subject.as_deref(), Some("Away"));
    }

    #[test]
    fn save_reports_a_patch_that_the_server_refuses() {
        let server = MockServer::builder().start();

        assert!(save(&link_to(&server), json!({"isEnabled": "yes"}), None).is_err());
        assert!(!singleton(&server).is_enabled);
    }

    /// Writes a patch without a state check, as a second program (another
    /// mail client, the server's web interface) does.
    fn edit_elsewhere(link: &AccountLink, patch: serde_json::Value) {
        save(link, patch, None).unwrap();
    }

    #[test]
    fn save_does_not_overwrite_a_change_made_elsewhere_after_the_load() {
        let server = MockServer::builder().start();
        let link = link_to(&server);
        let loaded = load(&link).unwrap();

        edit_elsewhere(&link, json!({"subject": "Away until Monday"}));
        let error = save(
            &link,
            json!({"isEnabled": true, "subject": "Away"}),
            Some(loaded.state),
        )
        .unwrap_err();

        assert_eq!(error, translate(SETTINGS_CHANGED));
        let stored = singleton(&server);
        assert_eq!(stored.subject.as_deref(), Some("Away until Monday"));
        assert!(!stored.is_enabled, "the refused save wrote nothing");
    }

    #[test]
    fn a_save_with_no_known_state_still_writes() {
        let server = MockServer::builder().start();
        let link = link_to(&server);

        assert_eq!(save(&link, json!({"isEnabled": true}), None), Ok(()));

        assert!(singleton(&server).is_enabled);
    }
}
