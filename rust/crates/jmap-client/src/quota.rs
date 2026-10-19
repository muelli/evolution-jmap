// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Quota operations (RFC 9425).
//!
//! Provides read-only access to mailbox and account usage and limits.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::quota::{Quota, QuotaQueryFilter};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_QUOTA};

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_QUOTA];

impl Client {
    /// Fetch all quotas (`Quota/get` with `ids: null`).
    ///
    /// Returns an empty list without sending requests if the account lacks quota capabilities.
    pub fn quotas(&self, account_id: &Id) -> Result<Vec<Quota>, Error> {
        if !self.has_quota_capability(account_id) {
            return Ok(Vec::new());
        }
        let arguments =
            self.single_call(USING, "Quota/get", &GetRequest::all(account_id.clone()))?;
        let response: GetResponse<Quota> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Query matching quota IDs (`Quota/query`, RFC 9425 Section 4.4).
    ///
    /// Returns an empty list if the account lacks quota capabilities.
    pub fn quota_query(&self, account_id: &Id, filter: QuotaQueryFilter) -> Result<Vec<Id>, Error> {
        if !self.has_quota_capability(account_id) {
            return Ok(Vec::new());
        }
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "Quota/query", &request)?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }

    fn has_quota_capability(&self, account_id: &Id) -> bool {
        self.session()
            .accounts
            .get(account_id)
            .is_some_and(|account| account.has_capability(CAPABILITY_QUOTA))
    }
}
