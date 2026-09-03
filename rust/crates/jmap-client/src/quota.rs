// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Quota (RFC 9425): read-only mailbox/account usage and limits. There
//! is no `Quota/set` — the RFC defines quotas as server-computed.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse};
use jmap_proto::quota::Quota;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_QUOTA};

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_QUOTA];

impl Client {
    /// Fetch all quotas (`Quota/get` with `ids: null`).
    ///
    /// An empty list, with no request sent, when the account does not
    /// advertise `urn:ietf:params:jmap:quota` at all (Fastmail, for one):
    /// naming an unadvertised capability in `using` fails the whole request
    /// (RFC 8620 §3.6.1's `unknownCapability`) rather than answering "no
    /// quotas", and every caller here already treats an empty list as
    /// nothing to report.
    pub fn quotas(&self, account_id: &Id) -> Result<Vec<Quota>, Error> {
        let has_quota = self
            .session()
            .accounts
            .get(account_id)
            .is_some_and(|account| account.has_capability(CAPABILITY_QUOTA));
        if !has_quota {
            return Ok(Vec::new());
        }
        let arguments =
            self.single_call(USING, "Quota/get", &GetRequest::all(account_id.clone()))?;
        let response: GetResponse<Quota> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }
}
