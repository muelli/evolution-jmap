// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `SieveScript/get`, `/set`, `/query` and `/validate` (RFC 9661).

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetResponse};
use jmap_proto::sieve::{
    SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest, SieveScriptValidateRequest,
    SieveScriptValidateResponse, sieve_set_error,
};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::patch::apply_patch;
use crate::state::{AccountState, ServerState};

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

/// `SieveScript/set` (RFC 9661 §2.4). A script's `id` and `isActive` are
/// both server-set: `isActive` only ever changes through
/// `onSuccessActivateScript`, never a direct create or update, and
/// destroying the currently active script is `sieveIsActive` until it is
/// deactivated first. Script names are unique per account, so a duplicate
/// is `alreadyExists`, the same as every other `/set` method that has names.
pub fn sieve_script_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SieveScriptSetRequest = parse_arguments(arguments)?;
    let SieveScriptSetRequest {
        set,
        on_success_activate_script,
    } = request;
    let account_id = set.account_id.clone();
    let account = account_mut(state, &account_id)?;

    let old_state = account.sieve_scripts.state();
    if let Some(expected) = &set.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut names: BTreeMap<Id, String> = account
        .sieve_scripts
        .iter()
        .map(|(id, script)| (id.clone(), script.name.clone()))
        .collect();
    let active_id = account
        .sieve_scripts
        .iter()
        .find(|(_, script)| script.is_active)
        .map(|(id, _)| id.clone());

    let mut created: BTreeMap<String, SieveScript> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, SieveScript)> = Vec::new();
    let mut created_here: BTreeMap<String, Id> = BTreeMap::new();
    for (creation_id, mut script) in set.create.unwrap_or_default() {
        if script.id.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("id is set by the server and must not be given in a create"),
            );
            continue;
        }
        if script.is_active {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isActive is set by the server; activate via onSuccessActivateScript",
                ),
            );
            continue;
        }
        if names.values().any(|name| name == &script.name) {
            not_created.insert(creation_id, SetError::new(error::set::ALREADY_EXISTS));
            continue;
        }
        let id = account.sieve_scripts.alloc_id();
        script.id = Some(id.clone());
        names.insert(id.clone(), script.name.clone());
        created_here.insert(creation_id.clone(), id.clone());
        created.insert(creation_id, script.clone());
        to_create.push((id, script));
    }

    let mut updated: BTreeMap<Id, Option<SieveScript>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, SieveScript)> = Vec::new();
    for (id, patch) in set.update.unwrap_or_default() {
        let Some(existing) = account.sieve_scripts.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };
        let mut value = serde_json::to_value(existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        let patched = match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<SieveScript>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
            Ok(patched) => patched,
            Err(set_error) => {
                not_updated.insert(id, set_error);
                continue;
            }
        };
        if patched.id.as_ref() != Some(&id) {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description("id is immutable"),
            );
            continue;
        }
        if patched.is_active != existing.is_active {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isActive is set by the server; activate via onSuccessActivateScript",
                ),
            );
            continue;
        }
        if patched.name != existing.name
            && names
                .iter()
                .any(|(other, name)| other != &id && name == &patched.name)
        {
            not_updated.insert(id, SetError::new(error::set::ALREADY_EXISTS));
            continue;
        }
        names.insert(id.clone(), patched.name.clone());
        to_update.push((id, patched));
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in set.destroy.unwrap_or_default() {
        if !account.sieve_scripts.contains(&id) {
            not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
        } else if active_id.as_ref() == Some(&id) {
            not_destroyed.insert(id, SetError::new(sieve_set_error::SIEVE_IS_ACTIVE));
        } else {
            destroyed.push(id);
        }
    }

    account.sieve_scripts.transaction(|transaction| {
        for (id, script) in to_create {
            transaction.create(id, script);
        }
        for (id, script) in to_update {
            transaction.update(&id, script);
            updated.insert(id, None);
        }
        for id in &destroyed {
            transaction.destroy(id);
        }
    });

    if let Some(target) = on_success_activate_script {
        let resolved = match target {
            Value::Null => None,
            Value::String(reference) => {
                let id = match reference.strip_prefix('#') {
                    Some(creation_id) => created_here.get(creation_id).cloned(),
                    None => Some(Id::new(reference.clone())),
                };
                match id {
                    Some(id) if account.sieve_scripts.contains(&id) => Some(id),
                    _ => {
                        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                            .with_description("onSuccessActivateScript names an unknown script"));
                    }
                }
            }
            _ => {
                return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                    .with_description("onSuccessActivateScript must be a string or null"));
            }
        };
        activate(account, resolved.as_ref());
    }

    to_result(&SetResponse {
        account_id,
        old_state: Some(old_state),
        new_state: account.sieve_scripts.state(),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// `SieveScript/query` (RFC 9661 §2.5): filters by `name` (substring match,
/// the same idiom `principal_query` uses) and/or `isActive`.
pub fn sieve_script_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<SieveScriptQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .sieve_scripts
        .iter()
        .filter(|(_, script)| sieve_script_matches(script, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .sieve_scripts
        .iter()
        .filter(|(_, script)| sieve_script_matches(script, &filter))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.sieve_scripts.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

fn sieve_script_matches(script: &SieveScript, filter: &SieveScriptQueryFilter) -> bool {
    if let Some(name) = &filter.name
        && !script.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(is_active) = filter.is_active
        && script.is_active != is_active
    {
        return false;
    }
    true
}

/// `SieveScript/validate` (RFC 9661 section 2.6): the request must name
/// exactly one of `id`, `blobId` or `content` as the script's source. This
/// mock has no real Sieve parser, the same deliberate limitation
/// `SieveScript/set` already has (it never produces `invalidSieve`), so a
/// source that resolves is always `isValid: true`; only argument shape and
/// blob/id resolution are checked, the same `invalidArguments` case
/// `SieveScript/set`'s `onSuccessActivateScript` already uses for an unknown
/// reference.
pub fn sieve_script_validate(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: SieveScriptValidateRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let sources = [
        request.id.is_some(),
        request.blob_id.is_some(),
        request.content.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if sources != 1 {
        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
            .with_description("SieveScript/validate takes exactly one of id, blobId or content"));
    }

    let blob_id = if let Some(id) = &request.id {
        let Some(script) = account.sieve_scripts.get(id) else {
            return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                .with_description("id names an unknown SieveScript"));
        };
        Some(script.blob_id.clone())
    } else {
        request.blob_id.clone()
    };
    if let Some(blob_id) = blob_id
        && !account.blobs.contains_key(&blob_id)
    {
        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
            .with_description("blobId is not a known blob"));
    }

    to_result(&SieveScriptValidateResponse::valid(
        request.account_id.clone(),
    ))
}

/// Make `target` the one active script, deactivating whatever else was
/// active (RFC 9661 §2.4 `onSuccessActivateScript`); `None` deactivates
/// everything.
fn activate(account: &mut AccountState, target: Option<&Id>) {
    let ids: Vec<Id> = account
        .sieve_scripts
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    account.sieve_scripts.transaction(|transaction| {
        for id in &ids {
            let Some(script) = transaction.get(id) else {
                continue;
            };
            let should_be_active = target == Some(id);
            if script.is_active != should_be_active {
                let mut updated = script.clone();
                updated.is_active = should_be_active;
                transaction.update(id, updated);
            }
        }
    });
}
