// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal operations (RFC 9670): resolving an email/name to a principal
//! id and its capability bag — the shared floor for scheduling and
//! per-source sharing. See `docs/PRINCIPALS-DESIGN.md`.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal, PrincipalQueryFilter,
};
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE, CAPABILITY_PRINCIPALS};
use jmap_proto::state::UtcDate;

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS];

/// `getAvailability` is a calendars-draft extension on a principals object,
/// so its `using` set must name both capabilities (design §4.2).
const AVAILABILITY_USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS, CAPABILITY_CALENDARS];

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

    /// `Principal/getAvailability` (draft-ietf-jmap-calendars §2.2): the
    /// busy periods `principal_id` has between `utc_start` (inclusive) and
    /// `utc_end` (exclusive), e.g. to render an attendee's free/busy in a
    /// meeting scheduler.
    pub fn get_availability(
        &self,
        account_id: &Id,
        principal_id: &Id,
        utc_start: impl Into<UtcDate>,
        utc_end: impl Into<UtcDate>,
        show_details: bool,
    ) -> Result<Vec<BusyPeriod>, Error> {
        let mut request = GetAvailabilityRequest::new(
            account_id.clone(),
            principal_id.clone(),
            utc_start,
            utc_end,
        );
        if show_details {
            request = request.show_details();
        }
        let arguments =
            self.single_call(AVAILABILITY_USING, "Principal/getAvailability", &request)?;
        let response: GetAvailabilityResponse = serde_json::from_value(arguments)?;
        Ok(response.list)
    }
}
