// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The vacation page's two round trips, over the [`crate::link`] connection.
//! Blocking; worker threads only.

use jmap_proto::mail::VacationResponse;
use serde_json::Value;

use crate::link::{self, AccountLink};

/// The account's current autoresponder (`VacationResponse/get`).
pub fn load(link: &AccountLink) -> Result<VacationResponse, String> {
    link.call(|client| client.vacation_response_get(&link.features.account_id))
        .map_err(|error| link::describe(&error))
}

/// Write the page's state back (`VacationResponse/set` update).
pub fn save(link: &AccountLink, patch: Value) -> Result<(), String> {
    link.call(|client| client.vacation_response_update(&link.features.account_id, patch.clone()))
        .map(|_| ())
        .map_err(|error| link::describe(&error))
}
