// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal operations (RFC 9670).
//!
//! Resolves email addresses and names to principal identifiers and capabilities.

use jmap_proto::Id;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal, PrincipalQueryFilter,
    ShareNotification, ShareNotificationQueryFilter,
};
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE, CAPABILITY_PRINCIPALS};
use jmap_proto::state::UtcDate;

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS];

/// Availability queries require both principals and calendars capabilities.
const AVAILABILITY_USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS, CAPABILITY_CALENDARS];

impl Client {
    /// Fetch all principals (`Principal/get` with `ids: null`).
    pub fn principals(&self, account_id: &Id) -> Result<Vec<Principal>, Error> {
        let arguments =
            self.single_call(USING, "Principal/get", &GetRequest::all(account_id.clone()))?;
        let response: GetResponse<Principal> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Fetch principals by id (`Principal/get` with `ids`).
    ///
    /// Splits requests into chunks when IDs exceed `maxObjectsInGet`.
    /// When a server advertises an account ID for `currentUserPrincipalId`, `Principal/get` fails with `notFound`.
    /// In this case, the method falls back to resolving the owner principal by username or email.
    pub fn principal_get(&self, account_id: &Id, ids: &[Id]) -> Result<Vec<Principal>, Error> {
        let max_objects = self
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|&limit| limit > 0)
            .unwrap_or(ids.len().max(1));

        let mut fetched = Vec::with_capacity(ids.len());
        let mut not_found_list = Vec::new();
        let mut rest = ids;
        loop {
            let take = rest.len().min(max_objects);
            let (chunk, remaining) = rest.split_at(take);
            let request = GetRequest::ids(account_id.clone(), chunk.to_vec());
            let arguments = self.single_call(USING, "Principal/get", &request)?;
            let mut response: GetResponse<Principal> = serde_json::from_value(arguments)?;
            Self::keep_requested(&mut response.list, chunk, |principal| principal.id.as_ref());
            Self::keep_requested(&mut response.not_found, chunk, |id| Some(id));
            fetched.extend(response.list);
            not_found_list.extend(response.not_found);
            rest = remaining;
            if rest.is_empty() {
                break;
            }
        }

        if ids.len() == 1 && fetched.is_empty() && not_found_list.contains(&ids[0]) {
            let requested_id = &ids[0];
            let advertised_id = self
                .session()
                .accounts
                .get(account_id)
                .and_then(|acct| acct.account_capabilities.get(CAPABILITY_PRINCIPALS))
                .and_then(|cap| cap.get("currentUserPrincipalId"))
                .and_then(|id| id.as_str())
                .map(Id::from);

            let is_suspect_account_id =
                requested_id == account_id || advertised_id.as_ref() == Some(requested_id);

            if is_suspect_account_id {
                let resolved = self.resolve_owner_principal(account_id)?;
                if let Some(principal) = resolved {
                    return Ok(vec![principal]);
                }
            }
        }

        Ok(fetched)
    }

    /// Fetch a single principal by ID via `Principal/get`.
    fn fetch_principal(&self, account_id: &Id, id: &Id) -> Option<Principal> {
        let request = GetRequest::ids(account_id.clone(), [id.clone()]);
        let args = self.single_call(USING, "Principal/get", &request).ok()?;
        let resp = serde_json::from_value::<GetResponse<Principal>>(args).ok()?;
        resp.list.into_iter().next()
    }

    /// Fetch the current user's principal object (RFC 9670 Section 2).
    ///
    /// Reads `currentUserPrincipalId` from the account capability and queries `Principal/get`.
    /// If the server advertises an account ID instead, `Principal/get` returns `notFound`.
    /// In that case, this method resolves the owner principal by username or email.
    pub fn current_user_principal(&self, account_id: &Id) -> Result<Option<Principal>, Error> {
        if !self
            .session()
            .accounts
            .get(account_id)
            .is_some_and(|a| a.has_capability(CAPABILITY_PRINCIPALS))
        {
            return Ok(None);
        }

        let advertised_id = self
            .session()
            .accounts
            .get(account_id)
            .and_then(|acct| acct.account_capabilities.get(CAPABILITY_PRINCIPALS))
            .and_then(|cap| cap.get("currentUserPrincipalId"))
            .and_then(|id| id.as_str())
            .map(Id::from);

        if let Some(ref id) = advertised_id {
            let principals = self.principal_get(account_id, std::slice::from_ref(id))?;
            if let Some(p) = principals.into_iter().next() {
                return Ok(Some(p));
            }
        }

        self.resolve_owner_principal(account_id)
    }

    /// Resolve the current user's principal identifier (RFC 9670 Section 2).
    ///
    /// Resolves past servers that incorrectly advertise an account ID for `currentUserPrincipalId`.
    pub fn current_user_principal_id(&self, account_id: &Id) -> Result<Option<Id>, Error> {
        Ok(self.current_user_principal(account_id)?.and_then(|p| p.id))
    }

    /// Resolve the owner principal when `currentUserPrincipalId` is missing or invalid.
    ///
    /// Query filters match by substring, so candidates are verified with [`principal_identifies_username`].
    fn resolve_owner_principal(&self, account_id: &Id) -> Result<Option<Principal>, Error> {
        let username = &self.session().username;
        if !username.is_empty() {
            let filter = if username.contains('@') {
                PrincipalQueryFilter::email(username)
            } else {
                PrincipalQueryFilter::default().text(username)
            };
            if let Some(principal) = self.find_verified_principal(account_id, filter, username) {
                return Ok(Some(principal));
            }

            let alt_filter = if username.contains('@') {
                PrincipalQueryFilter::default().text(username)
            } else {
                PrincipalQueryFilter::email(username)
            };
            if let Some(principal) = self.find_verified_principal(account_id, alt_filter, username)
            {
                return Ok(Some(principal));
            }
        }

        if let Ok(all) = self.principals(account_id) {
            let matched = all
                .into_iter()
                .find(|p| principal_identifies_username(p, username));
            if let Some(principal) = matched {
                return Ok(Some(principal));
            }
        }

        Ok(None)
    }

    /// Return the first candidate principal that matches `username` exactly.
    fn find_verified_principal(
        &self,
        account_id: &Id,
        filter: PrincipalQueryFilter,
        username: &str,
    ) -> Option<Principal> {
        let ids = self.principal_query(account_id, filter).ok()?;
        ids.into_iter()
            .filter_map(|id| self.fetch_principal(account_id, &id))
            .find(|p| principal_identifies_username(p, username))
    }

    /// Query matching principal identifiers (`Principal/query`).
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

    /// Fetch all share notifications visible to this account (RFC 9670 Section 4).
    pub fn share_notifications(&self, account_id: &Id) -> Result<Vec<ShareNotification>, Error> {
        let arguments = self.single_call(
            USING,
            "ShareNotification/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<ShareNotification> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Resolve `ShareNotification` ids matching `filter`
    /// (`ShareNotification/query`, RFC 9670 §4).
    pub fn share_notification_query(
        &self,
        account_id: &Id,
        filter: ShareNotificationQueryFilter,
    ) -> Result<Vec<Id>, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "ShareNotification/query", &request)?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }

    /// Fetch principal availability (`Principal/getAvailability`, draft-ietf-jmap-calendars Section 2.2).
    ///
    /// Returns busy periods between `utc_start` (inclusive) and `utc_end` (exclusive).
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

/// Return true if `principal` matches `username` exactly.
///
/// Compares against the principal email, username local part, or display name.
fn principal_identifies_username(principal: &Principal, username: &str) -> bool {
    if username.is_empty() {
        return false;
    }
    if principal.email.as_deref() == Some(username) {
        return true;
    }
    if !username.contains('@')
        && let Some(email) = &principal.email
        && email.split('@').next() == Some(username)
    {
        return true;
    }
    principal.name == username
}
