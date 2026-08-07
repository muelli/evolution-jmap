// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental sync via `/changes` (RFC 8620 §5.2) — the primitive the
//! Evolution Data Server meta-backends' `get_changes_sync` will drive.

use jmap_proto::methods::{ChangesRequest, ChangesResponse};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
};
use jmap_proto::{Id, State};

use crate::client::Client;
use crate::error::Error;

impl Client {
    /// Generic `Foo/changes` call. `type_name` is a JMAP data type such as
    /// `Email` or `ContactCard`.
    pub fn changes(
        &self,
        account_id: &Id,
        type_name: &str,
        since_state: &State,
    ) -> Result<ChangesResponse, Error> {
        let capability = match type_name {
            "Mailbox" | "Email" => CAPABILITY_MAIL,
            "AddressBook" | "ContactCard" => CAPABILITY_CONTACTS,
            "Calendar" | "CalendarEvent" => CAPABILITY_CALENDARS,
            other => {
                return Err(Error::Protocol(format!(
                    "no known capability for data type {other}"
                )));
            }
        };
        let request = ChangesRequest {
            account_id: account_id.clone(),
            since_state: since_state.clone(),
            max_changes: None,
        };
        let arguments = self.single_call(
            &[CAPABILITY_CORE, capability],
            &format!("{type_name}/changes"),
            &request,
        )?;
        Ok(serde_json::from_value(arguments)?)
    }
}
