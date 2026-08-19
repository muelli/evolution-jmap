// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Principal methods (`Principal/get`, `Principal/query`, RFC 9670) and
//! principal seeding helpers.

use jmap_proto::Id;
use jmap_proto::error::MethodError;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::principals::{Principal, PrincipalQueryFilter};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::AccountState;

pub fn principal_get(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .principals
                .iter()
                .map(|(_, principal)| principal.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.principals.get(id) {
                    Some(principal) => list.push(principal.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.principals.state(),
        list,
        not_found,
    })
}

pub fn principal_query(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: QueryRequest<PrincipalQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .principals
        .iter()
        .filter(|(_, principal)| principal_matches(principal, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .principals
        .iter()
        .filter(|(_, principal)| principal_matches(principal, &filter))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.principals.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

fn principal_matches(principal: &Principal, filter: &PrincipalQueryFilter) -> bool {
    if let Some(name) = &filter.name
        && !principal.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(email) = &filter.email
        && principal.email.as_deref() != Some(email.as_str())
    {
        return false;
    }
    if let Some(text) = &filter.text {
        let matches_name = principal.name.contains(text.as_str());
        let matches_email = principal
            .email
            .as_deref()
            .is_some_and(|email| email.contains(text.as_str()));
        if !(matches_name || matches_email) {
            return false;
        }
    }
    true
}

impl AccountState {
    /// Seed a principal; returns its id. Does not bump state.
    pub fn seed_principal(&mut self, principal: Principal) -> Id {
        let id = self.principals.alloc_id();
        let principal = Principal {
            id: Some(id.clone()),
            ..principal
        };
        self.principals.seed_with_id(id.clone(), principal);
        id
    }

    /// Seed a principal and make it the account's `currentUserPrincipalId`
    /// (RFC 9670 §2.5) — the common case, since most tests only need one
    /// principal representing the account owner. Tests that need more than
    /// one (e.g. an attendee to resolve via `Principal/query`) call
    /// [`Self::seed_principal`] for the rest.
    pub fn seed_current_user_principal(&mut self, principal: Principal) -> Id {
        let id = self.seed_principal(principal);
        self.current_user_principal_id = Some(id.clone());
        id
    }
}
