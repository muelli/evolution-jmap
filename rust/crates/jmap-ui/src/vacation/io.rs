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
