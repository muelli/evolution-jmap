// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared `/set` and `/changes` helpers for [`Store`]-backed JMAP types.
//! Use these helpers when store-level create, update, and destroy logic is enough.

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{SetRequest, SetResponse};

use crate::patch::apply_patch;
use crate::state::Store;

/// Serve `/changes` from the store log, as RFC 8620 §5.2 defines.
///
/// Report each id in at most one list.
/// Hide objects that were created and destroyed in the same served window.
///
/// Apply `page_size` and client `maxChanges` at transition boundaries.
/// Keep the returned `newState` reusable.
///
/// `unrecognized_state_is_invalid_arguments` selects parse error mapping for
/// `sinceState`: RFC-style `cannotCalculateChanges` or Stalwart-style
/// `invalidArguments`.
pub(crate) fn store_changes<T>(
    store: &Store<T>,
    request: jmap_proto::methods::ChangesRequest,
    page_size: Option<u64>,
    unrecognized_state_is_invalid_arguments: bool,
) -> Result<jmap_proto::methods::ChangesResponse, MethodError> {
    let since: u64 = request.since_state.as_str().parse().map_err(|_| {
        if unrecognized_state_is_invalid_arguments {
            MethodError::new(error::method::INVALID_ARGUMENTS)
                .with_description("invalid JMAP State")
        } else {
            MethodError::new("cannotCalculateChanges")
                .with_description("sinceState was not issued by this server")
        }
    })?;
    let cap = [page_size, request.max_changes].into_iter().flatten().min();
    let (window_end, has_more_changes) = window(store, since, cap);

    #[derive(Default)]
    struct Disposition {
        created: bool,
        updated: bool,
        destroyed: bool,
    }
    let mut by_id: BTreeMap<Id, Disposition> = BTreeMap::new();
    for change in store
        .changes_since(since)
        .filter(|change| change.state <= window_end)
    {
        let disposition = by_id.entry(change.id.clone()).or_default();
        match change.kind {
            crate::state::ChangeKind::Created => disposition.created = true,
            crate::state::ChangeKind::Updated => disposition.updated = true,
            crate::state::ChangeKind::Destroyed => disposition.destroyed = true,
        }
    }

    let mut created = Vec::new();
    let mut updated = Vec::new();
    let mut destroyed = Vec::new();
    for (id, disposition) in by_id {
        match (disposition.created, disposition.destroyed) {
            (true, true) => {} // never visible to this client
            (true, false) => created.push(id),
            (false, true) => destroyed.push(id),
            (false, false) if disposition.updated => updated.push(id),
            (false, false) => {}
        }
    }

    Ok(jmap_proto::methods::ChangesResponse::new(
        request.account_id,
        request.since_state,
        jmap_proto::State::new(window_end.to_string()),
    )
    .with_created(created)
    .with_updated(updated)
    .with_destroyed(destroyed)
    .has_more_changes(has_more_changes))
}

/// Return the last included state and whether more changes remain.
///
/// Grow windows by whole transitions.
/// Always include the first transition, so tiny caps still advance `sinceState`.
fn window<T>(store: &Store<T>, since: u64, cap: Option<u64>) -> (u64, bool) {
    let Some(cap) = cap else {
        return (store.state_counter(), false);
    };

    // Keep one entry per transition state with its changed object count.
    let mut transitions: Vec<(u64, u64)> = Vec::new();
    for change in store.changes_since(since) {
        match transitions.last_mut() {
            Some((state, count)) if *state == change.state => *count += 1,
            _ => transitions.push((change.state, 1)),
        }
    }

    let mut end = store.state_counter();
    let mut taken: u64 = 0;
    for (index, (state, count)) in transitions.iter().enumerate() {
        if index > 0 && taken + count > cap {
            return (transitions[index - 1].0, true);
        }
        taken += count;
        end = *state;
    }
    (end, false)
}

/// Apply `/set` to `store`.
///
/// `prepare` validates each create and can stamp server-set properties with
/// the allocated id.
/// Accept update and destroy entries as-is.
///
/// Use [`simple_set_with_validation`] when update or destroy need checks.
pub(crate) fn simple_set<T>(
    store: &mut Store<T>,
    request: SetRequest<T>,
    prepare: impl FnMut(&Id, &mut T) -> Result<(), SetError>,
) -> Result<SetResponse<T>, MethodError>
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    simple_set_with_validation(store, request, prepare, |_, _, _| Ok(()), |_, _| Ok(()))
}

/// Run [`simple_set`] with update and destroy validation before mutation.
pub(crate) fn simple_set_with_validation<T>(
    store: &mut Store<T>,
    request: SetRequest<T>,
    mut prepare: impl FnMut(&Id, &mut T) -> Result<(), SetError>,
    mut validate_update: impl FnMut(&Id, &T, &T) -> Result<(), SetError>,
    mut validate_destroy: impl FnMut(&Id, &T) -> Result<(), SetError>,
) -> Result<SetResponse<T>, MethodError>
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    let old_state = store.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut created: BTreeMap<String, T> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, T)> = Vec::new();
    for (creation_id, mut object) in request.create.unwrap_or_default() {
        let id = store.alloc_id();
        match prepare(&id, &mut object) {
            Ok(()) => {
                created.insert(creation_id, object.clone());
                to_create.push((id, object));
            }
            Err(set_error) => {
                not_created.insert(creation_id, set_error);
            }
        }
    }

    let mut updated: BTreeMap<Id, Option<T>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, T)> = Vec::new();
    for (id, patch) in request.update.unwrap_or_default() {
        let Some(existing) = store.get(&id) else {
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
        let outcome = apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<T>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            })
            .and_then(|patched| {
                validate_update(&id, existing, &patched)?;
                Ok(patched)
            });
        match outcome {
            Ok(patched) => to_update.push((id, patched)),
            Err(set_error) => {
                not_updated.insert(id, set_error);
            }
        }
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    store.transaction(|transaction| {
        for (id, object) in to_create {
            transaction.create(id, object);
        }
        for (id, object) in to_update {
            transaction.update(&id, object);
            updated.insert(id, None);
        }
        for id in request.destroy.unwrap_or_default() {
            match transaction.get(&id) {
                None => {
                    not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
                }
                Some(existing) => match validate_destroy(&id, existing) {
                    Ok(()) => {
                        transaction.destroy(&id);
                        destroyed.push(id);
                    }
                    Err(set_error) => {
                        not_destroyed.insert(id, set_error);
                    }
                },
            }
        }
    });

    let mut response =
        SetResponse::new(request.account_id, store.state()).with_old_state(old_state);
    if !created.is_empty() {
        response = response.with_created(created);
    }
    if !updated.is_empty() {
        response = response.with_updated(updated);
    }
    if !destroyed.is_empty() {
        response = response.with_destroyed(destroyed);
    }
    if !not_created.is_empty() {
        response = response.with_not_created(not_created);
    }
    if !not_updated.is_empty() {
        response = response.with_not_updated(not_updated);
    }
    if !not_destroyed.is_empty() {
        response = response.with_not_destroyed(not_destroyed);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::simple_set_with_validation;
    use crate::state::Store;
    use jmap_proto::error;
    use jmap_proto::methods::SetRequest;
    use serde_json::Value;

    #[test]
    fn simple_set_with_validation_rejects_stale_if_in_state() {
        let mut store: Store<Value> = Store::new("obj-");

        let response = simple_set_with_validation(
            &mut store,
            SetRequest::new("account").if_in_state("stale-state"),
            |_, _| Ok(()),
            |_, _, _| Ok(()),
            |_, _| Ok(()),
        );

        let error = response.expect_err("stale ifInState should fail");
        assert_eq!(error.error_type, error::method::STATE_MISMATCH);
    }

    #[test]
    fn simple_set_with_validation_accepts_matching_if_in_state() {
        let mut store: Store<Value> = Store::new("obj-");
        let current_state = store.state();

        let response = simple_set_with_validation(
            &mut store,
            SetRequest::new("account").if_in_state(current_state.clone()),
            |_, _| Ok(()),
            |_, _, _| Ok(()),
            |_, _| Ok(()),
        )
        .expect("matching ifInState should succeed");

        assert_eq!(response.old_state, Some(current_state.clone()));
        assert_eq!(response.new_state, Some(current_state));
    }

    #[test]
    fn simple_set_with_validation_omits_empty_updated_and_destroyed() {
        let mut store: Store<Value> = Store::new("obj-");

        let response = simple_set_with_validation(
            &mut store,
            SetRequest::new("account"),
            |_, _| Ok(()),
            |_, _, _| Ok(()),
            |_, _| Ok(()),
        )
        .expect("no-op set should succeed");

        assert_eq!(response.updated, None);
        assert_eq!(response.destroyed, None);
    }
}
