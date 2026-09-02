// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `SieveScript/get` (RFC 9661). `/set`, `/query` and `/validate` are
//! separate increments.

use jmap_proto::error::MethodError;
use jmap_proto::methods::{GetRequest, GetResponse};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::ServerState;

/// `SieveScript/get` (RFC 9661 §2.3): an `ids: null` request returns every
/// script the account has, which is none on a fresh account since scripts
/// are client-created; a named id that does not exist is `notFound`, the
/// same as every other `/get` method.
pub fn sieve_script_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .sieve_scripts
                .iter()
                .map(|(_, script)| script.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.sieve_scripts.get(id) {
                    Some(script) => list.push(script.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.sieve_scripts.state(),
        list,
        not_found,
    })
}
