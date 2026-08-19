// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal operations (RFC 9670): resolving an email/name to a principal
//! id and its capability bag — the shared floor for scheduling and
//! per-source sharing. See `docs/PRINCIPALS-DESIGN.md`.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::principals::{Principal, PrincipalQueryFilter};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_PRINCIPALS};

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS];

impl Client {
    /// Fetch all principals (`Principal/get` with `ids: null`).
    pub fn principals(&self, account_id: &Id) -> Result<Vec<Principal>, Error> {
        let arguments =
            self.single_call(USING, "Principal/get", &GetRequest::all(account_id.clone()))?;
        let response: GetResponse<Principal> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Resolve principals matching `filter` (`Principal/query`) — e.g. by
    /// email, to turn a meeting attendee's address into a principal id.
    pub fn principal_query(
        &self,
        account_id: &Id,
        filter: PrincipalQueryFilter,
    ) -> Result<Vec<Id>, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "Principal/query", &request)?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }
}
