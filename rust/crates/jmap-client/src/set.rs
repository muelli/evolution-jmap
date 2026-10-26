// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers for splitting `/set` method calls across `maxObjectsInSet` (RFC 8620 Section 2).
//!
//! Each chunk carries the creation ids of the earlier chunks as the request's `createdIds`
//! (RFC 8620 Section 3.3), so the server resolves `#creationId` references itself. Only the
//! server knows which properties have the type `Id`, so the client never rewrites an object.
//! If a chunk boundary breaks a circular reference, splitting fails.

use std::collections::{BTreeMap, HashMap, HashSet};

use jmap_proto::Id;
use jmap_proto::error::SetError;
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::state::State;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::Error;

/// Turn an optional [`SetError`] rejection into an [`Error`].
///
/// Returns [`Error::Set`] when an error is present.
/// Returns [`Error::Protocol`] when the response omits both success and failure.
pub(crate) fn set_failure(set_error: Option<&SetError>) -> Error {
    match set_error {
        Some(set_error) => Error::Set(set_error.clone()),
        None => Error::Protocol("/set response reports neither success nor failure".to_owned()),
    }
}

/// Extract the created object for `creation_id` from a `/set` response.
///
/// Returns the created item when present in `created`.
/// Returns [`Error::Set`] when `not_created` rejects the creation id.
/// Returns [`Error::Protocol`] when the response mentions neither.
pub fn expect_created<T: Clone>(response: &SetResponse<T>, creation_id: &str) -> Result<T, Error> {
    if let Some(object) = response
        .created
        .as_ref()
        .and_then(|created| created.get(creation_id))
    {
        return Ok(object.clone());
    }
    Err(set_failure(
        response
            .not_created
            .as_ref()
            .and_then(|not_created| not_created.get(creation_id)),
    ))
}

/// Verify that `id` was updated in a `/set` response.
///
/// Returns `Ok(())` when `id` is present in `updated`.
/// Returns [`Error::Set`] when `not_updated` rejects the update.
/// Returns [`Error::Protocol`] when the response mentions neither.
pub fn expect_updated<T>(response: &SetResponse<T>, id: &Id) -> Result<(), Error> {
    if response
        .updated
        .as_ref()
        .is_some_and(|updated| updated.contains_key(id))
    {
        return Ok(());
    }
    if let Some(err) = response
        .not_updated
        .as_ref()
        .and_then(|not_updated| not_updated.get(id))
    {
        return Err(Error::Set(err.clone()));
    }
    Err(Error::Protocol(format!(
        "/set response reports neither success nor failure for {id}"
    )))
}

/// Verify that `id` was destroyed in a `/set` response.
///
/// Returns `Ok(())` when `id` is present in `destroyed`.
/// Returns [`Error::Set`] when `not_destroyed` rejects the destruction.
/// Returns [`Error::Protocol`] when the response mentions neither.
pub fn expect_destroyed<T>(response: &SetResponse<T>, id: &Id) -> Result<(), Error> {
    if response
        .destroyed
        .as_ref()
        .is_some_and(|destroyed| destroyed.contains(id))
    {
        return Ok(());
    }
    if let Some(err) = response
        .not_destroyed
        .as_ref()
        .and_then(|not_destroyed| not_destroyed.get(id))
    {
        return Err(Error::Set(err.clone()));
    }
    Err(Error::Protocol(format!(
        "/set response reports neither success nor failure for {id}"
    )))
}

/// Chunk definition containing partition slices for create, update, and destroy.
#[derive(Debug)]
struct SetChunk<T> {
    create: BTreeMap<String, T>,
    update: BTreeMap<Id, Value>,
    destroy: Vec<Id>,
}

impl<T> Default for SetChunk<T> {
    fn default() -> Self {
        Self {
            create: BTreeMap::new(),
            update: BTreeMap::new(),
            destroy: Vec::new(),
        }
    }
}

impl<T> SetChunk<T> {
    fn is_empty(&self) -> bool {
        self.create.is_empty() && self.update.is_empty() && self.destroy.is_empty()
    }
}

/// Find the creation ids that a JSON value can refer to.
///
/// The result can contain false matches, for example a title equal to `#c1`. Use it only to
/// order and group creates, never to change the value.
pub(crate) fn find_creation_references(
    val: &Value,
    candidates: &HashSet<String>,
) -> HashSet<String> {
    let mut refs = HashSet::new();
    collect_creation_references(val, candidates, &mut refs);
    refs
}

fn collect_creation_references(
    val: &Value,
    candidates: &HashSet<String>,
    refs: &mut HashSet<String>,
) {
    match val {
        Value::String(s) => {
            if let Some(target) = s.strip_prefix('#').filter(|t| candidates.contains(*t)) {
                refs.insert(target.to_owned());
            }
        }
        Value::Object(map) => {
            for (key, v) in map {
                for segment in key.split('/') {
                    if let Some(target) = segment
                        .strip_prefix('#')
                        .filter(|t| candidates.contains(*t))
                    {
                        refs.insert(target.to_owned());
                    }
                }
                collect_creation_references(v, candidates, refs);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_creation_references(item, candidates, refs);
            }
        }
        _ => {}
    }
}

struct Tarjan<'a> {
    dependencies: &'a HashMap<String, HashSet<String>>,
    index: usize,
    indices: HashMap<String, usize>,
    lowlink: HashMap<String, usize>,
    on_stack: HashSet<String>,
    stack: Vec<String>,
    sccs: Vec<Vec<String>>,
}

impl<'a> Tarjan<'a> {
    fn new(dependencies: &'a HashMap<String, HashSet<String>>) -> Self {
        Self {
            dependencies,
            index: 0,
            indices: HashMap::new(),
            lowlink: HashMap::new(),
            on_stack: HashSet::new(),
            stack: Vec::new(),
            sccs: Vec::new(),
        }
    }

    fn strongconnect(&mut self, node: &str) {
        self.indices.insert(node.to_owned(), self.index);
        self.lowlink.insert(node.to_owned(), self.index);
        self.index += 1;
        self.stack.push(node.to_owned());
        self.on_stack.insert(node.to_owned());

        if let Some(deps) = self.dependencies.get(node) {
            for dep in deps {
                if !self.indices.contains_key(dep) {
                    self.strongconnect(dep);
                    let dep_low = self.lowlink[dep];
                    let node_low = self.lowlink.get_mut(node).unwrap();
                    *node_low = (*node_low).min(dep_low);
                } else if self.on_stack.contains(dep) {
                    let dep_index = self.indices[dep];
                    let node_low = self.lowlink.get_mut(node).unwrap();
                    *node_low = (*node_low).min(dep_index);
                }
            }
        }

        if self.lowlink[node] == self.indices[node] {
            let mut scc = Vec::new();
            while let Some(w) = self.stack.pop() {
                self.on_stack.remove(&w);
                scc.push(w.clone());
                if w == node {
                    break;
                }
            }
            self.sccs.push(scc);
        }
    }
}

/// Topologically sort strongly connected components in the creation graph.
fn sort_creation_components(
    nodes: &[String],
    dependencies: &HashMap<String, HashSet<String>>,
    limit: usize,
) -> Result<Vec<Vec<String>>, Error> {
    let mut tarjan = Tarjan::new(dependencies);

    for node in nodes {
        if !tarjan.indices.contains_key(node) {
            tarjan.strongconnect(node);
        }
    }

    let sccs = tarjan.sccs;

    // Verify no component exceeds the chunk limit.
    for scc in &sccs {
        if scc.len() > limit {
            return Err(Error::ChunkBoundaryReferenceBroken {
                creation_id: scc[0].clone(),
                reason: format!(
                    "dependency cycle of size {} exceeds maxObjectsInSet limit {}",
                    scc.len(),
                    limit
                ),
            });
        }
    }

    // Tarjan emits a component after the components it refers to, so the order is already correct.
    Ok(sccs)
}

/// Split a [`SetRequest`] into chunks bounded by `limit`.
fn partition_set_request<T: Clone + Serialize>(
    request: &SetRequest<T>,
    limit: usize,
) -> Result<Vec<SetChunk<T>>, Error> {
    let create_map = request.create.as_ref();
    let candidates: HashSet<String> = create_map
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    // Map dependencies for every create item.
    let mut dependencies: HashMap<String, HashSet<String>> = HashMap::new();
    let mut create_nodes = Vec::new();
    if let Some(creates) = create_map {
        for (cid, obj) in creates {
            create_nodes.push(cid.clone());
            let val = serde_json::to_value(obj)?;
            let mut refs = find_creation_references(&val, &candidates);
            refs.remove(cid);
            dependencies.insert(cid.clone(), refs);
        }
    }

    let sccs = sort_creation_components(&create_nodes, &dependencies, limit)?;

    let mut chunks: Vec<SetChunk<T>> = Vec::new();
    let mut current = SetChunk::default();
    let mut current_count = 0usize;

    // Pack create components into chunks.
    if let Some(creates) = create_map {
        for scc in sccs {
            if current_count + scc.len() > limit && current_count > 0 {
                chunks.push(std::mem::take(&mut current));
                current_count = 0;
            }
            for cid in scc {
                if let Some(item) = creates.get(&cid) {
                    current.create.insert(cid, item.clone());
                    current_count += 1;
                }
            }
        }
    }

    // Pack update items into chunks.
    if let Some(updates) = &request.update {
        for (id, patch) in updates {
            if current_count >= limit {
                chunks.push(std::mem::take(&mut current));
                current_count = 0;
            }
            current.update.insert(id.clone(), patch.clone());
            current_count += 1;
        }
    }

    // Pack destroy items into chunks.
    if let Some(destroys) = &request.destroy {
        for id in destroys {
            if current_count >= limit {
                chunks.push(std::mem::take(&mut current));
                current_count = 0;
            }
            current.destroy.push(id.clone());
            current_count += 1;
        }
    }

    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }

    Ok(chunks)
}

/// Execute a `/set` request, chunking across `limit` when necessary.
pub(crate) fn execute_set_chunked<T, F>(
    request: &SetRequest<T>,
    limit: Option<usize>,
    mut execute_chunk: F,
) -> Result<SetResponse<T>, Error>
where
    T: Clone + Serialize + DeserializeOwned,
    F: FnMut(&SetRequest<T>, &BTreeMap<Id, Id>) -> Result<SetResponse<T>, Error>,
{
    let total_items = request.create.as_ref().map_or(0, |m| m.len())
        + request.update.as_ref().map_or(0, |m| m.len())
        + request.destroy.as_ref().map_or(0, |v| v.len());

    let limit = match limit {
        Some(limit) if limit > 0 => limit,
        _ => return execute_chunk(request, &BTreeMap::new()),
    };

    if total_items <= limit {
        return execute_chunk(request, &BTreeMap::new());
    }

    let chunks = partition_set_request(request, limit)?;
    let mut merged = SetResponse::new(request.account_id.clone(), State::new(""));
    merged.new_state = None;
    let mut applied = false;
    match send_chunks(request, chunks, &mut merged, &mut applied, execute_chunk) {
        Ok(()) => Ok(merged),
        Err(cause) if applied => Err(Error::SetPartiallyApplied {
            committed: Box::new(serde_json::from_value(serde_json::to_value(&merged)?)?),
            cause: Box::new(cause),
        }),
        Err(cause) => Err(cause),
    }
}

/// Send `chunks` in order and merge each response into `merged`.
///
/// Set `applied` when the server answers the first chunk. Stop at the first error.
fn send_chunks<T, F>(
    request: &SetRequest<T>,
    chunks: Vec<SetChunk<T>>,
    merged: &mut SetResponse<T>,
    applied: &mut bool,
    mut execute_chunk: F,
) -> Result<(), Error>
where
    T: Clone + Serialize + DeserializeOwned,
    F: FnMut(&SetRequest<T>, &BTreeMap<Id, Id>) -> Result<SetResponse<T>, Error>,
{
    let candidates: HashSet<String> = request
        .create
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let mut created_ids: BTreeMap<Id, Id> = BTreeMap::new();
    let mut failed_creates: HashSet<String> = HashSet::new();
    let mut current_state = request.if_in_state.clone();

    for mut chunk in chunks {
        let create_refs: HashMap<String, HashSet<String>> = chunk
            .create
            .iter()
            .map(|(cid, obj)| {
                Ok((
                    cid.clone(),
                    find_creation_references(&serde_json::to_value(obj)?, &candidates),
                ))
            })
            .collect::<Result<_, Error>>()?;

        // Propagate failed creations across intra-chunk references before filtering.
        loop {
            let mut progressed = false;
            for cid in chunk.create.keys() {
                if !failed_creates.contains(cid)
                    && let Some(refs) = create_refs.get(cid)
                    && let Some(failed_id) = refs.iter().find(|r| failed_creates.contains(*r))
                {
                    let failed_id = failed_id.clone();
                    failed_creates.insert(cid.clone());
                    merged.not_created.get_or_insert_with(BTreeMap::new).insert(
                        cid.clone(),
                        SetError::new(jmap_proto::error::set::INVALID_PROPERTIES).with_description(
                            format!("creation reference #{failed_id} failed in earlier chunk"),
                        ),
                    );
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        let in_chunk: HashSet<String> = chunk
            .create
            .keys()
            .filter(|cid| !failed_creates.contains(*cid))
            .cloned()
            .collect();
        let known =
            |r: &String| created_ids.contains_key(&Id::new(r.as_str())) || in_chunk.contains(r);

        let mut ready_create = BTreeMap::new();
        for (cid, obj) in chunk.create {
            if failed_creates.contains(&cid) {
                continue;
            }
            let refs = &create_refs[&cid];
            if let Some(unresolved_id) = refs.iter().find(|r| !known(r)) {
                return Err(unresolved(unresolved_id));
            }
            ready_create.insert(cid, obj);
        }

        let mut ready_update = BTreeMap::new();
        for (id, patch) in chunk.update {
            let refs = find_creation_references(&patch, &candidates);

            if let Some(failed_id) = refs.iter().find(|r| failed_creates.contains(*r)) {
                merged.not_updated.get_or_insert_with(BTreeMap::new).insert(
                    id,
                    SetError::new(jmap_proto::error::set::INVALID_PROPERTIES).with_description(
                        format!("creation reference #{failed_id} failed in earlier chunk"),
                    ),
                );
                continue;
            }

            if let Some(unresolved_id) = refs.iter().find(|r| !known(r)) {
                return Err(unresolved(unresolved_id));
            }
            ready_update.insert(id, patch);
        }

        chunk.create = ready_create;
        chunk.update = ready_update;

        if chunk.is_empty() {
            continue;
        }

        let mut chunk_request = SetRequest::new(request.account_id.clone());
        chunk_request.if_in_state = current_state.clone();
        if !chunk.create.is_empty() {
            chunk_request.create = Some(chunk.create);
        }
        if !chunk.update.is_empty() {
            chunk_request.update = Some(chunk.update);
        }
        if !chunk.destroy.is_empty() {
            chunk_request.destroy = Some(chunk.destroy);
        }

        let response = execute_chunk(&chunk_request, &created_ids)?;
        *applied = true;

        if merged.old_state.is_none() {
            merged.old_state = response.old_state;
        }
        if response.new_state.is_some() {
            current_state = response.new_state.clone();
            merged.new_state = response.new_state;
        }

        if let Some(created) = response.created {
            for (cid, obj) in created {
                let obj_val = serde_json::to_value(&obj)?;
                if let Some(id_str) = obj_val.get("id").and_then(Value::as_str) {
                    created_ids.insert(Id::new(cid.as_str()), Id::new(id_str));
                }
                merged
                    .created
                    .get_or_insert_with(BTreeMap::new)
                    .insert(cid, obj);
            }
        }

        if let Some(not_created) = response.not_created {
            for (cid, err) in not_created {
                failed_creates.insert(cid.clone());
                merged
                    .not_created
                    .get_or_insert_with(BTreeMap::new)
                    .insert(cid, err);
            }
        }

        if let Some(updated) = response.updated {
            merged
                .updated
                .get_or_insert_with(BTreeMap::new)
                .extend(updated);
        }

        if let Some(not_updated) = response.not_updated {
            merged
                .not_updated
                .get_or_insert_with(BTreeMap::new)
                .extend(not_updated);
        }

        if let Some(destroyed) = response.destroyed {
            merged
                .destroyed
                .get_or_insert_with(Vec::new)
                .extend(destroyed);
        }

        if let Some(not_destroyed) = response.not_destroyed {
            merged
                .not_destroyed
                .get_or_insert_with(BTreeMap::new)
                .extend(not_destroyed);
        }
    }

    Ok(())
}

fn unresolved(creation_id: &str) -> Error {
    Error::ChunkBoundaryReferenceBroken {
        creation_id: creation_id.to_owned(),
        reason: format!("creation reference #{creation_id} was not resolved before chunk boundary"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One chunk the fake server received, with the `createdIds` map it carried.
    type Sent = (SetRequest<Value>, BTreeMap<Id, Id>);

    /// Run `request` against a fake server that assigns `id-<creationId>` to every create.
    ///
    /// Return the response and each chunk with the `createdIds` map it carried.
    fn run(
        request: &SetRequest<Value>,
        limit: usize,
    ) -> (Result<SetResponse<Value>, Error>, Vec<Sent>) {
        let mut sent = Vec::new();
        let result = execute_set_chunked(request, Some(limit), |chunk, created_ids| {
            sent.push((chunk.clone(), created_ids.clone()));
            Ok(answer(chunk))
        });
        (result, sent)
    }

    fn answer(chunk: &SetRequest<Value>) -> SetResponse<Value> {
        let mut response = SetResponse::new(chunk.account_id.clone(), State::new("s"));
        response.created = chunk.create.as_ref().map(|create| {
            create
                .keys()
                .map(|cid| (cid.clone(), json!({ "id": format!("id-{cid}") })))
                .collect()
        });
        response.updated = chunk
            .update
            .as_ref()
            .map(|update| update.keys().map(|id| (id.clone(), None)).collect());
        response.destroyed = chunk.destroy.clone();
        response
    }

    fn sent_create<'a>(sent: &'a [Sent], cid: &str) -> &'a Value {
        sent.iter()
            .find_map(|(chunk, _)| chunk.create.as_ref()?.get(cid))
            .expect("create sent")
    }

    #[test]
    fn a_later_chunk_carries_the_earlier_creation_ids_for_the_server() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "parent" }))
            .create("c1", json!({ "name": "child", "parentId": "#c0" }));
        let (result, sent) = run(&request, 1);
        result.unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].1.is_empty());
        assert_eq!(
            sent[1].1,
            BTreeMap::from([(Id::new("c0"), Id::new("id-c0"))])
        );
        // The server resolves the reference from `createdIds`; the client sends it unchanged.
        assert_eq!(sent_create(&sent, "c1")["parentId"], "#c0");
    }

    #[test]
    fn a_chain_of_references_is_sent_targets_first() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "parentId": "#c1" }))
            .create("c1", json!({ "parentId": "#c2" }))
            .create("c2", json!({ "name": "root" }));
        let (result, sent) = run(&request, 1);
        assert_eq!(result.unwrap().created.unwrap().len(), 3);
        let order: Vec<&String> = sent
            .iter()
            .flat_map(|(chunk, _)| chunk.create.as_ref().unwrap().keys())
            .collect();
        assert_eq!(order, ["c2", "c1", "c0"]);
        assert_eq!(sent[2].1.len(), 2);
    }

    #[test]
    fn a_plain_string_equal_to_a_creation_id_is_sent_unchanged() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "title": "first" }))
            .create("c1", json!({ "title": "#c0", "tags": ["#c0"] }));
        let (result, sent) = run(&request, 1);
        result.unwrap();
        assert_eq!(
            sent_create(&sent, "c1"),
            &json!({ "title": "#c0", "tags": ["#c0"] })
        );
    }

    #[test]
    fn a_patch_key_that_starts_with_a_shorter_creation_id_is_sent_unchanged() {
        let request = SetRequest::new("a")
            .create("c1", json!({ "name": "one" }))
            .update(
                "m",
                json!({ "keywords/#c10": true, "mailboxIds/#c1": true }),
            );
        let (result, sent) = run(&request, 1);
        result.unwrap();
        let (chunk, created_ids) = &sent[1];
        assert_eq!(
            chunk.update.as_ref().unwrap()[&Id::new("m")],
            json!({ "keywords/#c10": true, "mailboxIds/#c1": true })
        );
        assert_eq!(created_ids[&Id::new("c1")], Id::new("id-c1"));
    }

    #[test]
    fn a_reference_to_a_create_in_the_same_chunk_is_sent() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "parent" }))
            .create("c1", json!({ "name": "child", "parentId": "#c0" }))
            .destroy("old");
        let (result, sent) = run(&request, 2);
        let response = result.unwrap();
        assert_eq!(response.created.unwrap().len(), 2);
        assert_eq!(sent[0].0.create.as_ref().unwrap().len(), 2);
        assert_eq!(sent_create(&sent, "c1")["parentId"], "#c0");
    }

    #[test]
    fn a_failing_later_chunk_keeps_the_results_of_the_applied_chunks() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "zero" }))
            .create("c1", json!({ "name": "one" }))
            .destroy("old");
        let mut calls = 0;
        let result = execute_set_chunked(&request, Some(1), |chunk, _| {
            calls += 1;
            if calls == 2 {
                return Err(Error::Transport("connection reset".into()));
            }
            Ok(answer(chunk))
        });
        assert_eq!(calls, 2, "no chunk is sent after a failure");
        let Err(Error::SetPartiallyApplied { committed, cause }) = result else {
            panic!("expected SetPartiallyApplied, got {result:?}");
        };
        assert!(matches!(*cause, Error::Transport(_)));
        let created = committed.created.expect("created map");
        assert_eq!(created.keys().collect::<Vec<_>>(), ["c0"]);
        assert_eq!(created["c0"]["id"], "id-c0");
        assert_eq!(committed.new_state, Some(State::new("s")));
    }

    #[test]
    fn a_failing_first_chunk_returns_the_plain_error() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "zero" }))
            .create("c1", json!({ "name": "one" }));
        let result = execute_set_chunked(&request, Some(1), |_: &SetRequest<Value>, _| {
            Err(Error::Transport("connection refused".into()))
        });
        assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
    }

    #[test]
    fn a_create_referencing_a_same_chunk_create_whose_prerequisite_failed_is_not_sent() {
        // Chunk 0 (limit 2) holds "a0" and "b". "b" fails on the server.
        // Chunk 1 holds "c" (references "#d") and "d" (references "#b").
        // Because "c" sorts before "d" in the BTreeMap, "c" must still see that "d" failed.
        let request = SetRequest::new("a")
            .create("a0", json!({ "name": "root-ok" }))
            .create("b", json!({ "name": "root-fail" }))
            .create("d", json!({ "parentId": "#b" }))
            .create("c", json!({ "parentId": "#d" }));

        let mut sent = Vec::new();
        let result = execute_set_chunked(&request, Some(2), |chunk, created_ids| {
            sent.push((chunk.clone(), created_ids.clone()));
            let mut resp = SetResponse::new(chunk.account_id.clone(), State::new("s"));
            let mut created = BTreeMap::new();
            let mut not_created = BTreeMap::new();
            if let Some(creates) = &chunk.create {
                for cid in creates.keys() {
                    if cid == "b" {
                        not_created.insert(
                            cid.clone(),
                            SetError::new(jmap_proto::error::set::INVALID_PROPERTIES),
                        );
                    } else {
                        created.insert(cid.clone(), json!({ "id": format!("id-{cid}") }));
                    }
                }
            }
            if !created.is_empty() {
                resp.created = Some(created);
            }
            if !not_created.is_empty() {
                resp.not_created = Some(not_created);
            }
            Ok(resp)
        });

        let response = result.expect("set succeeds with partial not_created");
        assert_eq!(
            sent.len(),
            1,
            "second chunk must be skipped completely when both 'd' and 'c' fail locally"
        );
        let not_created = response.not_created.expect("not_created map");
        assert!(not_created.contains_key("b"));
        assert!(not_created.contains_key("d"));
        assert!(not_created.contains_key("c"));
    }

    #[test]
    fn a_creation_reference_inside_an_array_is_discovered_and_ordered() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "tags": ["#c1"] }))
            .create("c1", json!({ "name": "tag" }));
        let (result, sent) = run(&request, 1);
        result.unwrap();
        assert_eq!(sent.len(), 2);
        let order: Vec<&String> = sent
            .iter()
            .flat_map(|(chunk, _)| chunk.create.as_ref().unwrap().keys())
            .collect();
        assert_eq!(order, ["c1", "c0"]);
        assert_eq!(sent[1].1[&Id::new("c1")], Id::new("id-c1"));
    }

    #[test]
    fn a_dependency_cycle_exceeding_limit_returns_error() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "parentId": "#c1" }))
            .create("c1", json!({ "parentId": "#c0" }));
        let (result, _) = run(&request, 1);
        let Err(Error::ChunkBoundaryReferenceBroken {
            creation_id,
            reason,
        }) = result
        else {
            panic!("expected ChunkBoundaryReferenceBroken, got {result:?}");
        };
        assert!(
            reason.contains("dependency cycle of size 2 exceeds maxObjectsInSet limit 1"),
            "got reason: {reason}"
        );
        assert!(creation_id == "c0" || creation_id == "c1");
    }

    #[test]
    fn a_dependency_cycle_within_limit_succeeds_in_one_chunk() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "parentId": "#c1" }))
            .create("c1", json!({ "parentId": "#c0" }));
        let (result, sent) = run(&request, 2);
        result.unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.create.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn mixed_create_update_destroy_counts_all_items_against_limit() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "zero" }))
            .update("u0", json!({ "name": "one" }))
            .destroy("d0");
        let (result, sent) = run(&request, 2);
        result.unwrap();
        assert_eq!(sent.len(), 2);
    }

    #[test]
    fn limit_of_zero_sends_the_request_unchunked() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "zero" }))
            .create("c1", json!({ "name": "one" }));
        let (result, sent) = run(&request, 0);
        result.unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.create.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn updates_and_destroys_split_across_chunk_limit() {
        let request = SetRequest::new("a")
            .update("u0", json!({ "name": "zero" }))
            .update("u1", json!({ "name": "one" }))
            .update("u2", json!({ "name": "two" }));
        let (result, sent) = run(&request, 2);
        result.unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0.update.as_ref().unwrap().len(), 2);
        assert_eq!(sent[1].0.update.as_ref().unwrap().len(), 1);

        let request = SetRequest::new("a")
            .destroy("d0")
            .destroy("d1")
            .destroy("d2");
        let (result, sent) = run(&request, 2);
        result.unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0.destroy.as_ref().unwrap().len(), 2);
        assert_eq!(sent[1].0.destroy.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn an_update_referencing_a_failed_create_fails_with_invalid_properties() {
        let request = SetRequest::new("a")
            .create("c0", json!({ "name": "bad" }))
            .update("u1", json!({ "parentId": "#c0" }));
        let mut calls = 0;
        let result = execute_set_chunked(&request, Some(1), |chunk, _| {
            calls += 1;
            let mut resp = SetResponse::new(chunk.account_id.clone(), State::new("s"));
            if calls == 1 {
                let mut not_created = BTreeMap::new();
                not_created.insert(
                    "c0".to_string(),
                    SetError::new(jmap_proto::error::set::INVALID_PROPERTIES),
                );
                resp.not_created = Some(not_created);
            }
            Ok(resp)
        });
        let response = result.unwrap();
        assert_eq!(
            calls, 1,
            "update chunk should be skipped because all updates failed prerequisites"
        );
        let not_updated = response.not_updated.expect("not_updated map");
        assert!(not_updated.contains_key(&Id::new("u1")));
    }

    #[test]
    fn an_empty_set_request_sends_one_empty_chunk() {
        let request = SetRequest::new("a");
        let (result, sent) = run(&request, 1);
        result.unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].0.create.is_none());
        assert!(sent[0].0.update.is_none());
        assert!(sent[0].0.destroy.is_none());
    }
}
