// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal operations (RFC 9670): resolving an email/name to a principal
//! id and its capability bag — the shared floor for scheduling and
//! per-source sharing. See `docs/PRINCIPALS-DESIGN.md`.

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

    /// Fetch principals by id (`Principal/get` with `ids`).
    ///
    /// When a single ID is requested and matches either `account_id` or the
    /// advertised `currentUserPrincipalId` in the session document, but the server
    /// reports it as `notFound` (Stalwart Finding 18: `currentUserPrincipalId`
    /// advertises the account ID instead of the principal ID), this method
    /// automatically falls back to resolving the account owner's true principal
    /// by username or email.
    pub fn principal_get(&self, account_id: &Id, ids: &[Id]) -> Result<Vec<Principal>, Error> {
        let request = GetRequest::ids(account_id.clone(), ids.to_vec());
        let arguments = self.single_call(USING, "Principal/get", &request)?;
        let response: GetResponse<Principal> = serde_json::from_value(arguments)?;

        if ids.len() == 1 && response.list.is_empty() && response.not_found.contains(&ids[0]) {
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

        Ok(response.list)
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
    /// Reads `currentUserPrincipalId` from the account's `urn:ietf:params:jmap:principals`
    /// capability and queries `Principal/get`. When the server advertises its account
    /// identifier instead of a principal identifier (Stalwart Finding 18), `Principal/get`
    /// returns `notFound` for that ID. In that case, this method falls back to querying the
    /// user's principal by username or email via `Principal/query` and matching against
    /// [`principals`](Self::principals).
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
    /// Returns the true principal identifier corresponding to the account owner,
    /// resolving past Stalwart's `currentUserPrincipalId` account-id mismatch
    /// (Finding 18).
    pub fn current_user_principal_id(&self, account_id: &Id) -> Result<Option<Id>, Error> {
        Ok(self.current_user_principal(account_id)?.and_then(|p| p.id))
    }

    /// Helper to resolve the owner principal when currentUserPrincipalId is missing or
    /// points to the account ID (Finding 18 fallback).
    ///
    /// `Principal/query`'s own "text"/"email" filters match by substring
    /// containment (RFC 9670 Section 2.4.1), so a server with more than one
    /// principal sharing a substring with `username` can return several
    /// candidate ids. Every candidate is checked with
    /// [`principal_identifies_username`] before being accepted; the earlier
    /// shape here returned whichever one `Principal/get` answered first with
    /// no check at all, so a decoy principal that merely contained
    /// `username` as a substring and happened to sort first would be
    /// returned as if it were the account owner.
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

    /// Resolve `filter` and return the first candidate whose own email or
    /// name actually identifies `username`, not merely the first one the
    /// query happened to return.
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

    /// Fetch all `ShareNotification`s visible to this credential (RFC 9670
    /// §4) — one appears for each `shareWith` grant, widen, narrow, or
    /// revoke a caller was the recipient of.
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

/// Whether `principal` is the one `username` (the session's own login
/// identity) actually names, checked by exact identity rather than the
/// substring containment `Principal/query`'s own filters use: `username`
/// equals the principal's email outright, or — when `username` carries no
/// `@` and is plausibly a bare login name rather than an address — equals
/// the local part of its email, or equals its name outright. RFC 9670
/// defines no `isPersonal` property on `Principal` at all (Section 2 lists
/// exactly `id`/`type`/`name`/`description`/`email`/`timeZone`/
/// `capabilities`/`accounts`); no server this crate talks to, mock or real,
/// has ever been seen to set one, so a prior revision of this check that
/// also accepted any candidate with `is_personal == Some(true)` could never
/// actually fire and is not reinstated here.
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
