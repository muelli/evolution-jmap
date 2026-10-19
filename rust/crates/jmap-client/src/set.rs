// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers for splitting `/set` method calls across `maxObjectsInSet` (RFC 8620 Section 2).
//!
//! Splitting preserves creation-id back-references across chunk boundaries.
//! If a chunk boundary breaks a circular or unresolved reference, splitting fails.

use std::collections::{BTreeMap, HashMap, HashSet};

use jmap_proto::Id;
use jmap_proto::error::SetError;
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::state::State;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::Error;

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

/// Find creation-id references in a JSON value.
///
/// Collects IDs that match entries in `candidates`.
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

/// Replace creation-id references in a JSON value with resolved IDs.
pub(crate) fn substitute_creation_references(val: &mut Value, resolved: &HashMap<String, Id>) {
    match val {
        Value::String(s) => {
            if let Some(real_id) = s.strip_prefix('#').and_then(|target| resolved.get(target)) {
                *s = real_id.as_str().to_owned();
            }
        }
        Value::Object(map) => {
            let mut keys_to_replace = Vec::new();
            for key in map.keys() {
                if key.starts_with('#') || key.contains("/#") {
                    keys_to_replace.push(key.clone());
                }
            }
            for old_key in keys_to_replace {
                if let Some(value) = map.remove(&old_key) {
                    let mut new_key = old_key.clone();
                    for (target, real_id) in resolved {
                        let needle = format!("#{target}");
                        if new_key == needle {
                            new_key = real_id.as_str().to_owned();
                        } else {
                            new_key = new_key.replace(&needle, real_id.as_str());
                        }
                    }
                    map.insert(new_key, value);
                }
            }
            for v in map.values_mut() {
                substitute_creation_references(v, resolved);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                substitute_creation_references(item, resolved);
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

    let mut sccs = tarjan.sccs;

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

    // Tarjan produces reverse topological order. Reverse gives forward order.
    sccs.reverse();
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
    F: FnMut(&SetRequest<T>) -> Result<SetResponse<T>, Error>,
{
    let total_items = request.create.as_ref().map_or(0, |m| m.len())
        + request.update.as_ref().map_or(0, |m| m.len())
        + request.destroy.as_ref().map_or(0, |v| v.len());

    let limit = match limit {
        Some(limit) if limit > 0 => limit,
        _ => return execute_chunk(request),
    };

    if total_items <= limit {
        return execute_chunk(request);
    }

    let chunks = partition_set_request(request, limit)?;
    let candidates: HashSet<String> = request
        .create
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let mut resolved_ids: HashMap<String, Id> = HashMap::new();
    let mut failed_creates: HashSet<String> = HashSet::new();
    let mut current_state = request.if_in_state.clone();

    let mut merged = SetResponse::new(request.account_id.clone(), State::new(""));
    merged.new_state = None;

    for mut chunk in chunks {
        // Substitute resolved IDs in create objects.
        let mut ready_create = BTreeMap::new();
        for (cid, obj) in chunk.create {
            let mut val = serde_json::to_value(&obj)?;
            let refs = find_creation_references(&val, &candidates);

            // Check if object references a creation ID that failed.
            let failed_ref = refs.iter().find(|r| failed_creates.contains(*r));
            if let Some(failed_id) = failed_ref {
                failed_creates.insert(cid.clone());
                merged.not_created.get_or_insert_with(BTreeMap::new).insert(
                    cid,
                    SetError::new(jmap_proto::error::set::INVALID_PROPERTIES).with_description(
                        format!("creation reference #{failed_id} failed in earlier chunk"),
                    ),
                );
                continue;
            }

            // Check if object references an ID not yet resolved and not in current batch.
            let unresolved = refs
                .iter()
                .find(|r| !resolved_ids.contains_key(*r) && *r != &cid);
            if let Some(unresolved_id) = unresolved {
                return Err(Error::ChunkBoundaryReferenceBroken {
                    creation_id: unresolved_id.clone(),
                    reason: format!(
                        "creation reference #{unresolved_id} was not resolved before chunk boundary"
                    ),
                });
            }

            substitute_creation_references(&mut val, &resolved_ids);
            let updated_obj: T = serde_json::from_value(val)?;
            ready_create.insert(cid, updated_obj);
        }

        // Substitute resolved IDs in updates.
        let mut ready_update = BTreeMap::new();
        for (id, mut patch) in chunk.update {
            let refs = find_creation_references(&patch, &candidates);

            let failed_ref = refs.iter().find(|r| failed_creates.contains(*r));
            if let Some(failed_id) = failed_ref {
                merged.not_updated.get_or_insert_with(BTreeMap::new).insert(
                    id,
                    SetError::new(jmap_proto::error::set::INVALID_PROPERTIES).with_description(
                        format!("creation reference #{failed_id} failed in earlier chunk"),
                    ),
                );
                continue;
            }

            let unresolved = refs.iter().find(|r| !resolved_ids.contains_key(*r));
            if let Some(unresolved_id) = unresolved {
                return Err(Error::ChunkBoundaryReferenceBroken {
                    creation_id: unresolved_id.clone(),
                    reason: format!(
                        "creation reference #{unresolved_id} was not resolved before chunk boundary"
                    ),
                });
            }

            substitute_creation_references(&mut patch, &resolved_ids);
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

        let response = execute_chunk(&chunk_request)?;

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
                    resolved_ids.insert(cid.clone(), Id::new(id_str));
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

    Ok(merged)
}
