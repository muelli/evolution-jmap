// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generic `/set` implementation over a [`Store`], shared by data types that
//! need no per-create side effects beyond the store itself (contacts,
//! calendar events).

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{SetRequest, SetResponse};

use crate::patch::apply_patch;
use crate::state::Store;

/// Answer a standard `/changes` request from the store's changes log
/// (RFC 8620 §5.2). An id appears in at most one list: objects created and
/// destroyed inside the window appear in neither.
///
/// `page_size` is the server's own cap on how many ids one response may carry
/// ([`crate::MockServerBuilder::changes_page_size`]); the client's
/// `maxChanges` caps it further. Either way the answer is truncated at a state
/// boundary — `newState` has to be a state the client can ask again from, and
/// half of a transition is not one.
pub(crate) fn store_changes<T>(
    store: &Store<T>,
    request: jmap_proto::methods::ChangesRequest,
    page_size: Option<u64>,
) -> Result<jmap_proto::methods::ChangesResponse, MethodError> {
    let since: u64 = request.since_state.as_str().parse().map_err(|_| {
        MethodError::new("cannotCalculateChanges")
            .with_description("sinceState was not issued by this server")
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

    Ok(jmap_proto::methods::ChangesResponse {
        account_id: request.account_id,
        old_state: request.since_state,
        new_state: jmap_proto::State::new(window_end.to_string()),
        has_more_changes,
        created,
        updated,
        destroyed,
    })
}

/// How far past `since` this response reaches, and whether anything is left
/// beyond it.
///
/// The window ends at a state boundary, so it grows one whole transition at a
/// time. The first transition is served however large it is: a cap that could
/// withhold all of it would be a client that asks again from the same state
/// forever.
fn window<T>(store: &Store<T>, since: u64, cap: Option<u64>) -> (u64, bool) {
    let Some(cap) = cap else {
        return (store.state_counter(), false);
    };

    // The log in transition order: one entry per state, with how many objects
    // that transition touched.
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

/// Apply a standard `/set` request to `store`.
///
/// `prepare` validates a create and stamps server-set properties onto it
/// (it receives the freshly allocated id). The created-response map echoes
/// the full stored object — a superset of the server-set properties, which
/// clients may rely on.
///
/// Update and destroy are not validated: use [`simple_set_with_validation`]
/// for a data type that needs to refuse one of those too.
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

/// Same as [`simple_set`], but also validates an update against the object
/// both before and after the patch, and a destroy against the object about
/// to be removed, refusing either before it is ever applied to the store.
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

    Ok(SetResponse {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: store.state(),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}
