// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Sieve (RFC 9661): `SieveScript/get`. `/set`, `/query` and
//! `/validate` are separate increments — Evolution's filters UI is not
//! wired to any of this yet.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};
use jmap_proto::sieve::SieveScript;

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_SIEVE];

impl Client {
    /// Fetch all Sieve scripts (`SieveScript/get` with `ids: null`).
    pub fn sieve_scripts(&self, account_id: &Id) -> Result<Vec<SieveScript>, Error> {
        let arguments = self.single_call(
            USING,
            "SieveScript/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<SieveScript> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }
}
