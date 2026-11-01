// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal operations (RFC 9670).
//!
//! Resolves email addresses and names to principal identifiers and capabilities.

use jmap_proto::methods::{
    ChangesResponse, GetRequest, GetResponse, QueryChangesRequest, QueryChangesResponse,
    QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal, PrincipalQueryFilter,
    ShareNotification, ShareNotificationQueryFilter,
};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CORE, CAPABILITY_PRINCIPALS,
    CAPABILITY_PRINCIPALS_AVAILABILITY,
};
use jmap_proto::state::UtcDate;
use jmap_proto::{Id, State};

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS];

/// Availability queries require principals, calendars, and principals availability capabilities.
const AVAILABILITY_USING: &[&str] = &[
    CAPABILITY_CORE,
    CAPABILITY_PRINCIPALS,
    CAPABILITY_CALENDARS,
    CAPABILITY_PRINCIPALS_AVAILABILITY,
];

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
    /// Splits requests into chunks when IDs exceed `maxSizeRequest` or `maxObjectsInGet`.
    /// When a server advertises an account ID for `currentUserPrincipalId`, `Principal/get` fails with `notFound`.
    /// In this case, the method falls back to resolving the owner principal by username or email.
    pub fn principal_get(&self, account_id: &Id, ids: &[Id]) -> Result<Vec<Principal>, Error> {
        let response = self.get_chunked(
            USING,
            "Principal/get",
            account_id,
            ids,
            None,
            |principal: &Principal| principal.id.as_ref(),
        )?;
        let fetched = response.list;
        let not_found_list = response.not_found;

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

    /// Request incremental changes for principals (`Principal/changes`, RFC 9670 §2.2).
    pub fn principal_changes(
        &self,
        account_id: &Id,
        since_state: &State,
    ) -> Result<ChangesResponse, Error> {
        self.changes(account_id, "Principal", since_state)
    }

    /// Modify principals (`Principal/set`, RFC 9670 §2.3).
    pub fn principal_set(
        &self,
        request: &SetRequest<Principal>,
    ) -> Result<SetResponse<Principal>, Error> {
        let arguments = self.single_call(USING, "Principal/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Query incremental changes for principals (`Principal/queryChanges`, RFC 9670 §2.5).
    pub fn principal_query_changes(
        &self,
        request: &QueryChangesRequest<PrincipalQueryFilter>,
    ) -> Result<QueryChangesResponse, Error> {
        let arguments = self.single_call(USING, "Principal/queryChanges", request)?;
        Ok(serde_json::from_value(arguments)?)
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

    /// Fetch share notifications by identifier (`ShareNotification/get` with `ids`, RFC 9670 §3.1).
    pub fn share_notification_get(
        &self,
        account_id: &Id,
        ids: &[Id],
    ) -> Result<Vec<ShareNotification>, Error> {
        let request = GetRequest::ids(account_id.clone(), ids.to_vec());
        let arguments = self.single_call(USING, "ShareNotification/get", &request)?;
        let response: GetResponse<ShareNotification> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Request incremental changes for share notifications (`ShareNotification/changes`, RFC 9670 §3.2).
    pub fn share_notification_changes(
        &self,
        account_id: &Id,
        since_state: &State,
    ) -> Result<ChangesResponse, Error> {
        self.changes(account_id, "ShareNotification", since_state)
    }

    /// Dismiss or modify share notifications (`ShareNotification/set`, RFC 9670 §3.3).
    pub fn share_notification_set(
        &self,
        request: &SetRequest<ShareNotification>,
    ) -> Result<SetResponse<ShareNotification>, Error> {
        let arguments = self.single_call(USING, "ShareNotification/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Dismiss a share notification by destroying it (`ShareNotification/set`, RFC 9670 §3.3).
    pub fn share_notification_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let mut request = SetRequest::new(account_id.clone());
        request.destroy = Some(vec![id.clone()]);
        let response = self.share_notification_set(&request)?;
        if response.destroyed.as_ref().is_some_and(|d| d.contains(id)) {
            Ok(())
        } else if let Some(error) = response.not_destroyed.as_ref().and_then(|nd| nd.get(id)) {
            Err(Error::Set(error.clone()))
        } else {
            Ok(())
        }
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

    /// Query incremental changes for share notifications (`ShareNotification/queryChanges`, RFC 9670 §3.5).
    pub fn share_notification_query_changes(
        &self,
        request: &QueryChangesRequest<ShareNotificationQueryFilter>,
    ) -> Result<QueryChangesResponse, Error> {
        let arguments = self.single_call(USING, "ShareNotification/queryChanges", request)?;
        Ok(serde_json::from_value(arguments)?)
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
