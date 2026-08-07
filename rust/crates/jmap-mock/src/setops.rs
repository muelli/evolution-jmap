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
pub(crate) fn store_changes<T>(
    store: &Store<T>,
    request: jmap_proto::methods::ChangesRequest,
) -> Result<jmap_proto::methods::ChangesResponse, MethodError> {
    let since: u64 = request.since_state.as_str().parse().map_err(|_| {
        MethodError::new("cannotCalculateChanges")
            .with_description("sinceState was not issued by this server")
    })?;

    #[derive(Default)]
    struct Disposition {
        created: bool,
        updated: bool,
        destroyed: bool,
    }
    let mut by_id: BTreeMap<Id, Disposition> = BTreeMap::new();
    for change in store.changes_since(since) {
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
        new_state: store.state(),
        has_more_changes: false,
        created,
        updated,
        destroyed,
    })
}

/// Apply a standard `/set` request to `store`.
///
/// `prepare` validates a create and stamps server-set properties onto it
/// (it receives the freshly allocated id). The created-response map echoes
/// the full stored object — a superset of the server-set properties, which
/// clients may rely on.
pub(crate) fn simple_set<T>(
    store: &mut Store<T>,
    request: SetRequest<T>,
    mut prepare: impl FnMut(&Id, &mut T) -> Result<(), SetError>,
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
        match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<T>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
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
            if transaction.destroy(&id) {
                destroyed.push(id);
            } else {
                not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
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
