// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Quota/*` methods (RFC 9425). Read-only: the RFC defines no `Quota/set`.

use jmap_proto::error::MethodError;
use jmap_proto::methods::{GetRequest, GetResponse};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::ServerState;

/// `Quota/get` (RFC 9425 §3): an `ids: null` request returns every quota the
/// account has; a named id that does not exist is `notFound`, the same as
/// every other `/get` method.
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
