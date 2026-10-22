// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implement `Quota/*` methods from RFC 9425.
//! Keep this module read only.
//! RFC 9425 defines no `Quota/set` method.

use jmap_proto::Id;
use jmap_proto::error::MethodError;
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::quota::{Quota, QuotaQueryFilter};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::ServerState;

/// Handle `Quota/get` as RFC 9425 §3 defines.
/// Return all quotas when `ids` is `null`.
/// Report unknown ids in `notFound`.
pub fn quota_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(account.quotas.iter().map(|(_, quota)| quota.clone())),
        Some(ids) => {
            for id in ids {
                match account.quotas.get(id) {
                    Some(quota) => list.push(quota.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.quotas.state(),
        list,
        not_found,
    })
}

/// Handle `Quota/query` as RFC 9425 §4.4 defines.
pub fn quota_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<QuotaQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .quotas
        .iter()
        .filter(|(_, quota)| quota_matches(quota, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .quotas
        .iter()
        .filter(|(_, quota)| quota_matches(quota, &filter))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.quotas.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

/// Apply RFC 9425 §4.4 filter rules.
/// Match `name` by substring.
/// Match `type` against one member in `types`.
/// Match `scope` and `resourceType` exactly.
fn quota_matches(quota: &Quota, filter: &QuotaQueryFilter) -> bool {
    if let Some(name) = &filter.name
        && !quota.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(resource_type) = &filter.resource_type
        && quota.resource_type != *resource_type
    {
        return false;
    }
    if let Some(scope) = &filter.scope
        && quota.scope != *scope
    {
        return false;
    }
    if let Some(r#type) = &filter.r#type
        && !quota.types.iter().any(|t| t == r#type)
    {
        return false;
    }
    true
}
