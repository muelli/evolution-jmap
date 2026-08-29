// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The standard `/get`, `/set`, `/query`, and `/changes` method argument and
//! response shapes shared by every JMAP data type (RFC 8620 §5).
//!
//! These are generic over the object (`T`) or filter (`F`) type; both the
//! client and the mock server reuse them for every domain.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SetError;
use crate::id::Id;
use crate::request::ResultReference;
use crate::state::State;

/// `Foo/get` arguments (RFC 8620 §5.1). `ids: None` means "all objects".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    /// Back-reference form of `ids` (serialized as `#ids`, RFC 8620 §3.7).
    #[serde(rename = "#ids", default, skip_serializing_if = "Option::is_none")]
    pub ids_ref: Option<ResultReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl GetRequest {
    pub fn all(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            ids: None,
            ids_ref: None,
            properties: None,
        }
    }

    pub fn ids(account_id: impl Into<Id>, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            ids: Some(ids.into_iter().map(Into::into).collect()),
            ..Self::all(account_id)
        }
    }
}

/// `Foo/get` response (RFC 8620 §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResponse<T> {
    pub account_id: Id,
    pub state: State,
    #[serde(default)]
    pub list: Vec<T>,
    #[serde(default)]
    pub not_found: Vec<Id>,
}

/// `Foo/set` arguments (RFC 8620 §5.3).
///
/// `update` values are `PatchObject`s — maps of (JSON-pointer-ish) paths to
/// new values — so they stay untyped here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
pub struct SetRequest<T> {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<State>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<BTreeMap<String, T>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<BTreeMap<Id, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<Vec<Id>>,
}

impl<T> SetRequest<T> {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            create: None,
            update: None,
            destroy: None,
        }
    }

    pub fn create(mut self, creation_id: impl Into<String>, object: T) -> Self {
        self.create
            .get_or_insert_with(BTreeMap::new)
            .insert(creation_id.into(), object);
        self
    }

    pub fn update(mut self, id: impl Into<Id>, patch: Value) -> Self {
        self.update
            .get_or_insert_with(BTreeMap::new)
            .insert(id.into(), patch);
        self
    }

    pub fn destroy(mut self, id: impl Into<Id>) -> Self {
        self.destroy.get_or_insert_with(Vec::new).push(id.into());
        self
    }

    pub fn if_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }
}

/// `Foo/set` response (RFC 8620 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
pub struct SetResponse<T> {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<State>,
    pub new_state: State,
    /// Server-set properties (at minimum `id`) per creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, T>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<BTreeMap<Id, Option<T>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroyed: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_updated: Option<BTreeMap<Id, SetError>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_destroyed: Option<BTreeMap<Id, SetError>>,
}

/// `Foo/query` arguments (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "F: Serialize", deserialize = "F: serde::Deserialize<'de>"))]
pub struct QueryRequest<F> {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    #[serde(default, skip_serializing_if = "is_default_position")]
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub calculate_total: bool,
}

impl<F> QueryRequest<F> {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            filter: None,
            sort: None,
            position: 0,
            anchor: None,
            anchor_offset: None,
            limit: None,
            calculate_total: false,
        }
    }

    pub fn filter(mut self, filter: F) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn sort(mut self, sort: impl IntoIterator<Item = Comparator>) -> Self {
        self.sort = Some(sort.into_iter().collect());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Id>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn anchor_offset(mut self, offset: i64) -> Self {
        self.anchor_offset = Some(offset);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }
}

fn is_default_position(position: &i64) -> bool {
    *position == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_true() -> bool {
    true
}

/// A sort key for `/query` (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparator {
    pub property: String,
    #[serde(default = "default_true")]
    pub is_ascending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
}

impl Comparator {
    pub fn ascending(property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            is_ascending: true,
            collation: None,
        }
    }

    pub fn descending(property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            is_ascending: false,
            collation: None,
        }
    }

    pub fn with_collation(mut self, collation: impl Into<String>) -> Self {
        self.collation = Some(collation.into());
        self
    }
}

/// `Foo/query` response (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub account_id: Id,
    pub query_state: State,
    #[serde(default)]
    pub can_calculate_changes: bool,
    #[serde(default)]
    pub position: u64,
    #[serde(default)]
    pub ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// `Foo/changes` arguments (RFC 8620 §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesRequest {
    pub account_id: Id,
    pub since_state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
}

/// Response of a binary upload (RFC 8620 §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResponse {
    pub account_id: Id,
    pub blob_id: Id,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size: u64,
}

/// `Foo/changes` response (RFC 8620 §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResponse {
    pub account_id: Id,
    pub old_state: State,
    pub new_state: State,
    #[serde(default)]
    pub has_more_changes: bool,
    #[serde(default)]
    pub created: Vec<Id>,
    #[serde(default)]
    pub updated: Vec<Id>,
    #[serde(default)]
    pub destroyed: Vec<Id>,
}

/// Standard boolean filter operators (RFC 8620 §5.5).
pub mod filter_operator {
    pub const AND: &str = "AND";
    pub const OR: &str = "OR";
    pub const NOT: &str = "NOT";
}

/// `Foo/queryChanges` arguments (RFC 8620 §5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "F: Serialize", deserialize = "F: serde::Deserialize<'de>"))]
pub struct QueryChangesRequest<F> {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    pub since_query_state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_id: Option<Id>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub calculate_total: bool,
}

impl<F> QueryChangesRequest<F> {
    pub fn new(account_id: impl Into<Id>, since_query_state: impl Into<State>) -> Self {
        Self {
            account_id: account_id.into(),
            filter: None,
            sort: None,
            since_query_state: since_query_state.into(),
            max_changes: None,
            up_to_id: None,
            calculate_total: false,
        }
    }

    pub fn filter(mut self, filter: F) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn sort(mut self, sort: impl IntoIterator<Item = Comparator>) -> Self {
        self.sort = Some(sort.into_iter().collect());
        self
    }

    pub fn max_changes(mut self, max_changes: u64) -> Self {
        self.max_changes = Some(max_changes);
        self
    }

    pub fn up_to_id(mut self, id: impl Into<Id>) -> Self {
        self.up_to_id = Some(id.into());
        self
    }

    pub fn calculate_total(mut self) -> Self {
        self.calculate_total = true;
        self
    }
}

/// An item added to query results in `/queryChanges` (RFC 8620 §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedItem {
    pub id: Id,
    pub index: u64,
}

impl AddedItem {
    pub fn new(id: impl Into<Id>, index: u64) -> Self {
        Self {
            id: id.into(),
            index,
        }
    }
}

/// `Foo/queryChanges` response (RFC 8620 §5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChangesResponse {
    pub account_id: Id,
    pub old_query_state: State,
    pub new_query_state: State,
    #[serde(default)]
    pub added: Vec<AddedItem>,
    #[serde(default)]
    pub removed: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// `Foo/copy` arguments (RFC 8620 §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
pub struct CopyRequest<T> {
    pub from_account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_from_in_state: Option<State>,
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<State>,
    pub create: BTreeMap<String, T>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_success_destroy_original: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy_from_if_in_state: Option<State>,
}

impl<T> CopyRequest<T> {
    pub fn new(from_account_id: impl Into<Id>, account_id: impl Into<Id>) -> Self {
        Self {
            from_account_id: from_account_id.into(),
            if_from_in_state: None,
            account_id: account_id.into(),
            if_in_state: None,
            create: BTreeMap::new(),
            on_success_destroy_original: false,
            destroy_from_if_in_state: None,
        }
    }

    pub fn if_from_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_from_in_state = Some(state.into());
        self
    }

    pub fn if_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }

    pub fn copy_object(mut self, creation_id: impl Into<String>, object: T) -> Self {
        self.create.insert(creation_id.into(), object);
        self
    }

    pub fn on_success_destroy_original(mut self) -> Self {
        self.on_success_destroy_original = true;
        self
    }

    pub fn destroy_from_if_in_state(mut self, state: impl Into<State>) -> Self {
        self.destroy_from_if_in_state = Some(state.into());
        self
    }
}

/// `Foo/copy` response (RFC 8620 §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
pub struct CopyResponse<T> {
    pub from_account_id: Id,
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<State>,
    pub new_state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, T>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
}
