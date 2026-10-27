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
#[non_exhaustive]
pub struct GetRequest {
    /// The account to read objects from.
    pub account_id: Id,
    /// The ids to fetch, or `None` for every object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    /// Back-reference form of `ids` (serialized as `#ids`, RFC 8620 §3.7).
    #[serde(rename = "#ids", default, skip_serializing_if = "Option::is_none")]
    pub ids_ref: Option<ResultReference>,
    /// The properties to return for each object, or `None` for the default set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

impl GetRequest {
    /// Returns every object the account holds.
    pub fn all(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            ids: None,
            ids_ref: None,
            properties: None,
        }
    }

    /// Returns only the given ids.
    pub fn ids(account_id: impl Into<Id>, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            ids: Some(ids.into_iter().map(Into::into).collect()),
            ..Self::all(account_id)
        }
    }

    /// Sets the properties to return.
    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    /// Sets a back-reference form of `ids`.
    pub fn ids_ref(mut self, ids_ref: impl Into<ResultReference>) -> Self {
        self.ids_ref = Some(ids_ref.into());
        self
    }
}

/// `Foo/get` response (RFC 8620 §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetResponse<T> {
    /// The account the objects came from.
    pub account_id: Id,
    /// The state string of the account's object list at the time of this response.
    pub state: State,
    /// The requested objects, in the order the server returns them.
    #[serde(default)]
    pub list: Vec<T>,
    /// Ids from the request `ids` that do not exist.
    #[serde(default)]
    pub not_found: Vec<Id>,
}

impl<T> GetResponse<T> {
    /// Creates a response with no missing ids.
    pub fn new(
        account_id: impl Into<Id>,
        state: impl Into<State>,
        list: impl IntoIterator<Item = T>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            state: state.into(),
            list: list.into_iter().collect(),
            not_found: Vec::new(),
        }
    }

    /// Sets the ids from the request that do not exist.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = not_found.into_iter().map(Into::into).collect();
        self
    }
}

/// `Foo/set` arguments (RFC 8620 §5.3).
///
/// `update` values are `PatchObject`s: maps of paths to new values, kept untyped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct SetRequest<T> {
    /// The account to apply the changes to.
    pub account_id: Id,
    /// Rejects the request unless the account is in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<State>,
    /// New objects to create, keyed by a client-chosen creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<BTreeMap<String, T>>,
    /// Patches to apply to existing objects, keyed by object id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<BTreeMap<Id, Value>>,
    /// Ids of objects to delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<Vec<Id>>,
}

impl<T> SetRequest<T> {
    /// Creates an empty request with no changes.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            create: None,
            update: None,
            destroy: None,
        }
    }

    /// Adds one object to create under the given creation id.
    pub fn create(mut self, creation_id: impl Into<String>, object: T) -> Self {
        self.create
            .get_or_insert_with(BTreeMap::new)
            .insert(creation_id.into(), object);
        self
    }

    /// Sets every object to create.
    pub fn with_create(mut self, create: BTreeMap<String, T>) -> Self {
        self.create = Some(create);
        self
    }

    /// Adds one patch for the given object id.
    pub fn update(mut self, id: impl Into<Id>, patch: Value) -> Self {
        self.update
            .get_or_insert_with(BTreeMap::new)
            .insert(id.into(), patch);
        self
    }

    /// Sets every patch to apply.
    pub fn with_update(mut self, update: BTreeMap<Id, Value>) -> Self {
        self.update = Some(update);
        self
    }

    /// Adds one object id to delete.
    pub fn destroy(mut self, id: impl Into<Id>) -> Self {
        self.destroy.get_or_insert_with(Vec::new).push(id.into());
        self
    }

    /// Sets every object id to delete.
    pub fn with_destroy(mut self, destroy: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.destroy = Some(destroy.into_iter().map(Into::into).collect());
        self
    }

    /// Rejects the request unless the account is in this state.
    pub fn if_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }
}

/// `Foo/set` response (RFC 8620 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct SetResponse<T> {
    /// The account the changes applied to.
    pub account_id: Id,
    /// The state string before this request, if the server tracked one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<State>,
    /// The state `Foo/get` will now return. Nullable for servers omitting state on singletons.
    #[serde(default)]
    pub new_state: Option<State>,
    /// Server-set properties (at minimum `id`) per creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, T>>,
    /// Server-set properties per updated object id, or `None` where nothing changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<BTreeMap<Id, Option<T>>>,
    /// Ids of objects the server destroyed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroyed: Option<Vec<Id>>,
    /// Creation ids that failed, with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
    /// Object ids that failed to update, with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_updated: Option<BTreeMap<Id, SetError>>,
    /// Object ids that failed to destroy, with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_destroyed: Option<BTreeMap<Id, SetError>>,
}

impl<T> SetResponse<T> {
    /// Creates a response with no changes recorded yet.
    pub fn new(account_id: impl Into<Id>, new_state: impl Into<State>) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: None,
            new_state: Some(new_state.into()),
            created: None,
            updated: None,
            destroyed: None,
            not_created: None,
            not_updated: None,
            not_destroyed: None,
        }
    }

    /// Sets the state string from before this request.
    pub fn with_old_state(mut self, old_state: impl Into<State>) -> Self {
        self.old_state = Some(old_state.into());
        self
    }

    /// Sets the created objects.
    pub fn with_created(mut self, created: BTreeMap<String, T>) -> Self {
        self.created = Some(created);
        self
    }

    /// Sets the updated objects.
    pub fn with_updated(mut self, updated: BTreeMap<Id, Option<T>>) -> Self {
        self.updated = Some(updated);
        self
    }

    /// Sets the destroyed object ids.
    pub fn with_destroyed(mut self, destroyed: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.destroyed = Some(destroyed.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the creation ids that failed.
    pub fn with_not_created(mut self, not_created: BTreeMap<String, SetError>) -> Self {
        self.not_created = Some(not_created);
        self
    }

    /// Sets the object ids that failed to update.
    pub fn with_not_updated(mut self, not_updated: BTreeMap<Id, SetError>) -> Self {
        self.not_updated = Some(not_updated);
        self
    }

    /// Sets the object ids that failed to destroy.
    pub fn with_not_destroyed(mut self, not_destroyed: BTreeMap<Id, SetError>) -> Self {
        self.not_destroyed = Some(not_destroyed);
        self
    }
}

/// `Foo/query` arguments (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "F: Serialize", deserialize = "F: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct QueryRequest<F> {
    /// The account to query.
    pub account_id: Id,
    /// Keeps only objects that match this filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<F>,
    /// Orders the results by these comparators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    /// The zero-based index of the first result to return.
    #[serde(default, skip_serializing_if = "is_default_position")]
    pub position: i64,
    /// Finds `position` relative to this object id instead of the start of the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Id>,
    /// Shifts the starting position relative to `anchor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_offset: Option<i64>,
    /// The maximum number of ids to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Asks the server to also return the total number of matching objects.
    #[serde(default, skip_serializing_if = "is_false")]
    pub calculate_total: bool,
    /// Collapses each email thread to a single matching message (RFC 8621 §4.4.1).
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapse_threads: bool,
}

impl<F> QueryRequest<F> {
    /// Creates a query with no filter, sort, or limit.
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
            collapse_threads: false,
        }
    }

    /// Sets the filter.
    pub fn filter(mut self, filter: F) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Sets the sort order.
    pub fn sort(mut self, sort: impl IntoIterator<Item = Comparator>) -> Self {
        self.sort = Some(sort.into_iter().collect());
        self
    }

    /// Sets the starting position.
    pub fn position(mut self, position: i64) -> Self {
        self.position = position;
        self
    }

    /// Sets the anchor object id.
    pub fn anchor(mut self, anchor: impl Into<Id>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    /// Sets the offset from the anchor.
    pub fn anchor_offset(mut self, offset: i64) -> Self {
        self.anchor_offset = Some(offset);
        self
    }

    /// Sets the maximum number of ids to return.
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Asks the server to also return the total count.
    pub fn calculate_total(mut self) -> Self {
        self.calculate_total = true;
        self
    }

    /// Collapses each thread to one matching message.
    pub fn collapse_threads(mut self) -> Self {
        self.collapse_threads = true;
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
#[non_exhaustive]
pub struct Comparator {
    /// The object property to sort by.
    pub property: String,
    /// Sorts ascending when `true`, descending when `false`.
    #[serde(default = "default_true")]
    pub is_ascending: bool,
    /// The collation to use for string comparison, or `None` for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
}

impl Comparator {
    /// Creates a comparator that sorts ascending by the given property.
    pub fn ascending(property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            is_ascending: true,
            collation: None,
        }
    }

    /// Creates a comparator that sorts descending by the given property.
    pub fn descending(property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            is_ascending: false,
            collation: None,
        }
    }

    /// Sets the collation to use.
    pub fn with_collation(mut self, collation: impl Into<String>) -> Self {
        self.collation = Some(collation.into());
        self
    }
}

/// A `Foo/query` filter (RFC 8620 §5.5).
/// Represents a leaf condition of type `F` or an untagged boolean combination of filters.
/// Stays exhaustive: an untagged two-case type fixed by the RFC, not a type a server extends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Filter<F> {
    /// A boolean combination of other filters.
    Operator(FilterOperator<F>),
    /// A single leaf condition of type `F`.
    Condition(F),
}

/// RFC 8620 §5.5 `FilterOperator`. `operator` is one of the `operator`
/// module constants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FilterOperator<F> {
    /// One of the `operator` module constants.
    pub operator: String,
    /// The filters to combine with `operator`.
    pub conditions: Vec<Filter<F>>,
}

/// `FilterOperator::operator` values (RFC 8620 §5.5).
pub mod operator {
    /// Matches when every condition matches.
    pub const AND: &str = "AND";
    /// Matches when at least one condition matches.
    pub const OR: &str = "OR";
    /// Matches when no condition matches.
    pub const NOT: &str = "NOT";
}

impl<F> Filter<F> {
    /// Wraps a leaf condition as a filter.
    pub fn condition(condition: F) -> Self {
        Filter::Condition(condition)
    }

    /// Combines filters so every one of them must match.
    pub fn and(conditions: impl IntoIterator<Item = Filter<F>>) -> Self {
        Filter::Operator(FilterOperator {
            operator: operator::AND.to_string(),
            conditions: conditions.into_iter().collect(),
        })
    }

    /// Combines filters so at least one of them must match.
    pub fn or(conditions: impl IntoIterator<Item = Filter<F>>) -> Self {
        Filter::Operator(FilterOperator {
            operator: operator::OR.to_string(),
            conditions: conditions.into_iter().collect(),
        })
    }

    /// Negates a combination of filters.
    pub fn not(conditions: impl IntoIterator<Item = Filter<F>>) -> Self {
        Filter::Operator(FilterOperator {
            operator: operator::NOT.to_string(),
            conditions: conditions.into_iter().collect(),
        })
    }
}

impl<F> From<F> for Filter<F> {
    fn from(condition: F) -> Self {
        Filter::Condition(condition)
    }
}

/// `Foo/query` response (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QueryResponse {
    /// The account that was queried.
    pub account_id: Id,
    /// The state string of this query's result list.
    pub query_state: State,
    /// Whether the server can later return a `/queryChanges` for this query.
    #[serde(default)]
    pub can_calculate_changes: bool,
    /// The zero-based index of the first id in `ids`.
    #[serde(default)]
    pub position: u64,
    /// The matching object ids, in result order.
    #[serde(default)]
    pub ids: Vec<Id>,
    /// The total number of matching objects, if requested and known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The limit the server applied, if lower than the one requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

impl QueryResponse {
    /// Creates a response with no total and no limit set.
    pub fn new(
        account_id: impl Into<Id>,
        query_state: impl Into<State>,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            query_state: query_state.into(),
            can_calculate_changes: false,
            position: 0,
            ids: ids.into_iter().map(Into::into).collect(),
            total: None,
            limit: None,
        }
    }

    /// Sets the total number of matching objects.
    pub fn with_total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }

    /// Sets the limit the server applied.
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sets the position of the first returned id.
    pub fn with_position(mut self, position: u64) -> Self {
        self.position = position;
        self
    }

    /// Sets whether the server can calculate later changes to this query.
    pub fn can_calculate_changes(mut self, can: bool) -> Self {
        self.can_calculate_changes = can;
        self
    }
}

/// `Foo/changes` arguments (RFC 8620 §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ChangesRequest {
    /// The account to check for changes.
    pub account_id: Id,
    /// Returns changes after this state.
    pub since_state: State,
    /// The maximum number of ids to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
}

impl ChangesRequest {
    /// Creates a request with no limit on the number of changes.
    pub fn new(account_id: impl Into<Id>, since_state: impl Into<State>) -> Self {
        Self {
            account_id: account_id.into(),
            since_state: since_state.into(),
            max_changes: None,
        }
    }

    /// Sets the maximum number of changes to return.
    pub fn max_changes(mut self, max_changes: u64) -> Self {
        self.max_changes = Some(max_changes);
        self
    }
}

/// Response of a binary upload (RFC 8620 §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UploadResponse {
    /// The account the blob was uploaded to.
    pub account_id: Id,
    /// The id the server assigned to the uploaded blob.
    pub blob_id: Id,
    /// The content type the server recorded for the blob, if any.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The size of the uploaded blob, in bytes.
    pub size: u64,
}

impl UploadResponse {
    /// Creates a response with no recorded content type.
    pub fn new(account_id: impl Into<Id>, blob_id: impl Into<Id>, size: u64) -> Self {
        Self {
            account_id: account_id.into(),
            blob_id: blob_id.into(),
            content_type: None,
            size,
        }
    }

    /// Sets the recorded content type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

/// `Core/echo` method arguments and response (RFC 8620 §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
#[non_exhaustive]
pub struct Echo<T = Value> {
    /// The value the server echoes back unchanged.
    pub payload: T,
}

impl<T> Echo<T> {
    /// Wraps a payload to echo.
    pub fn new(payload: T) -> Self {
        Self { payload }
    }
}

/// `Foo/changes` response (RFC 8620 §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ChangesResponse {
    /// The account that was checked.
    pub account_id: Id,
    /// The state passed in as `since_state`.
    pub old_state: State,
    /// The state the account is in now.
    pub new_state: State,
    /// Whether more changes exist beyond `max_changes`.
    #[serde(default)]
    pub has_more_changes: bool,
    /// Ids of objects created since `old_state`.
    #[serde(default)]
    pub created: Vec<Id>,
    /// Ids of objects updated since `old_state`.
    #[serde(default)]
    pub updated: Vec<Id>,
    /// Ids of objects destroyed since `old_state`.
    #[serde(default)]
    pub destroyed: Vec<Id>,
}

impl ChangesResponse {
    /// Creates a response with no changes recorded yet.
    pub fn new(
        account_id: impl Into<Id>,
        old_state: impl Into<State>,
        new_state: impl Into<State>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: old_state.into(),
            new_state: new_state.into(),
            has_more_changes: false,
            created: Vec::new(),
            updated: Vec::new(),
            destroyed: Vec::new(),
        }
    }

    /// Sets the created object ids.
    pub fn with_created(mut self, created: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.created = created.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the updated object ids.
    pub fn with_updated(mut self, updated: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.updated = updated.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the destroyed object ids.
    pub fn with_destroyed(mut self, destroyed: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.destroyed = destroyed.into_iter().map(Into::into).collect();
        self
    }

    /// Sets whether more changes exist beyond the ones returned.
    pub fn has_more_changes(mut self, has_more: bool) -> Self {
        self.has_more_changes = has_more;
        self
    }
}

/// Standard boolean filter operators (RFC 8620 §5.5).
pub mod filter_operator {
    /// Matches when every condition matches.
    pub const AND: &str = "AND";
    /// Matches when at least one condition matches.
    pub const OR: &str = "OR";
    /// Matches when no condition matches.
    pub const NOT: &str = "NOT";
}

/// `Foo/queryChanges` arguments (RFC 8620 §5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "F: Serialize", deserialize = "F: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct QueryChangesRequest<F> {
    /// The account to query.
    pub account_id: Id,
    /// Keeps only objects that match this filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<F>,
    /// Orders the results by these comparators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    /// Returns changes since this query state.
    pub since_query_state: State,
    /// The maximum number of changes to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
    /// Stops returning changes once this id is reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_id: Option<Id>,
    /// Asks the server to also return the total number of matching objects.
    #[serde(default, skip_serializing_if = "is_false")]
    pub calculate_total: bool,
    /// Collapses each thread to one matching message, matching the original query.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapse_threads: bool,
}

impl<F> QueryChangesRequest<F> {
    /// Creates a request with no limit and no anchor id.
    pub fn new(account_id: impl Into<Id>, since_query_state: impl Into<State>) -> Self {
        Self {
            account_id: account_id.into(),
            filter: None,
            sort: None,
            since_query_state: since_query_state.into(),
            max_changes: None,
            up_to_id: None,
            calculate_total: false,
            collapse_threads: false,
        }
    }

    /// Sets the filter.
    pub fn filter(mut self, filter: F) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Sets the sort order.
    pub fn sort(mut self, sort: impl IntoIterator<Item = Comparator>) -> Self {
        self.sort = Some(sort.into_iter().collect());
        self
    }

    /// Sets the maximum number of changes to return.
    pub fn max_changes(mut self, max_changes: u64) -> Self {
        self.max_changes = Some(max_changes);
        self
    }

    /// Stops returning changes once this id is reached.
    pub fn up_to_id(mut self, id: impl Into<Id>) -> Self {
        self.up_to_id = Some(id.into());
        self
    }

    /// Asks the server to also return the total count.
    pub fn calculate_total(mut self) -> Self {
        self.calculate_total = true;
        self
    }

    /// Collapses each thread to one matching message.
    pub fn collapse_threads(mut self) -> Self {
        self.collapse_threads = true;
        self
    }
}

/// An item added to query results in `/queryChanges` (RFC 8620 §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddedItem {
    /// The id of the object added to the query results.
    pub id: Id,
    /// The zero-based position of the object in the result list.
    pub index: u64,
}

impl AddedItem {
    /// Creates an added-item entry at the given index.
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
#[non_exhaustive]
pub struct QueryChangesResponse {
    /// The account that was queried.
    pub account_id: Id,
    /// The query state passed in as `since_query_state`.
    pub old_query_state: State,
    /// The query state the server is now at.
    pub new_query_state: State,
    /// Objects added to the results, with their new positions.
    #[serde(default)]
    pub added: Vec<AddedItem>,
    /// Ids removed from the results.
    #[serde(default)]
    pub removed: Vec<Id>,
    /// The total number of matching objects, if requested and known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl QueryChangesResponse {
    /// Creates a response with no added or removed ids.
    pub fn new(
        account_id: impl Into<Id>,
        old_query_state: impl Into<State>,
        new_query_state: impl Into<State>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            old_query_state: old_query_state.into(),
            new_query_state: new_query_state.into(),
            added: Vec::new(),
            removed: Vec::new(),
            total: None,
        }
    }

    /// Sets the added items.
    pub fn with_added(mut self, added: impl IntoIterator<Item = AddedItem>) -> Self {
        self.added = added.into_iter().collect();
        self
    }

    /// Sets the removed ids.
    pub fn with_removed(mut self, removed: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.removed = removed.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the total number of matching objects.
    pub fn with_total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }
}

/// `Foo/copy` arguments (RFC 8620 §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct CopyRequest<T> {
    /// The account to copy objects from.
    pub from_account_id: Id,
    /// Rejects the request unless the source account is in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_from_in_state: Option<State>,
    /// The account to copy objects into.
    pub account_id: Id,
    /// Rejects the request unless the destination account is in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<State>,
    /// Objects to copy, keyed by a client-chosen creation id.
    pub create: BTreeMap<String, T>,
    /// Destroys the source object after a successful copy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_success_destroy_original: bool,
    /// Rejects the destroy of source objects unless the source account is still in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy_from_if_in_state: Option<State>,
}

impl<T> CopyRequest<T> {
    /// Creates a request that copies nothing yet and keeps the source objects.
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

    /// Rejects the request unless the source account is in this state.
    pub fn if_from_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_from_in_state = Some(state.into());
        self
    }

    /// Rejects the request unless the destination account is in this state.
    pub fn if_in_state(mut self, state: impl Into<State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }

    /// Adds one object to copy under the given creation id.
    pub fn copy_object(mut self, creation_id: impl Into<String>, object: T) -> Self {
        self.create.insert(creation_id.into(), object);
        self
    }

    /// Destroys the source object after a successful copy.
    pub fn on_success_destroy_original(mut self) -> Self {
        self.on_success_destroy_original = true;
        self
    }

    /// Rejects the destroy of source objects unless they are still in this state.
    pub fn destroy_from_if_in_state(mut self, state: impl Into<State>) -> Self {
        self.destroy_from_if_in_state = Some(state.into());
        self
    }
}

/// `Foo/copy` response (RFC 8620 §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: serde::Deserialize<'de>"))]
#[non_exhaustive]
pub struct CopyResponse<T> {
    /// The account the objects were copied from.
    pub from_account_id: Id,
    /// The account the objects were copied into.
    pub account_id: Id,
    /// The destination account's state before this request, if the server tracked one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<State>,
    /// The destination account's state after this request.
    pub new_state: State,
    /// The copied objects, keyed by their creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, T>>,
    /// Creation ids that failed to copy, with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
}

impl<T> CopyResponse<T> {
    /// Creates a response with nothing copied yet.
    pub fn new(
        from_account_id: impl Into<Id>,
        account_id: impl Into<Id>,
        new_state: impl Into<State>,
    ) -> Self {
        Self {
            from_account_id: from_account_id.into(),
            account_id: account_id.into(),
            old_state: None,
            new_state: new_state.into(),
            created: None,
            not_created: None,
        }
    }

    /// Sets the destination account's state before this request.
    pub fn with_old_state(mut self, old_state: impl Into<State>) -> Self {
        self.old_state = Some(old_state.into());
        self
    }

    /// Sets the copied objects.
    pub fn with_created(mut self, created: BTreeMap<String, T>) -> Self {
        self.created = Some(created);
        self
    }

    /// Sets the creation ids that failed to copy.
    pub fn with_not_created(mut self, not_created: BTreeMap<String, SetError>) -> Self {
        self.not_created = Some(not_created);
        self
    }
}

/// `Blob/copy` arguments (RFC 8620 §5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobCopyRequest {
    /// The account to copy blobs from.
    pub from_account_id: Id,
    /// The account to copy blobs into.
    pub account_id: Id,
    /// The ids of the blobs to copy.
    pub blob_ids: Vec<Id>,
}

impl BlobCopyRequest {
    /// Creates a request to copy the given blobs.
    pub fn new(
        from_account_id: impl Into<Id>,
        account_id: impl Into<Id>,
        blob_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            from_account_id: from_account_id.into(),
            account_id: account_id.into(),
            blob_ids: blob_ids.into_iter().map(Into::into).collect(),
        }
    }
}

/// `Blob/copy` response (RFC 8620 §5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobCopyResponse {
    /// The account the blobs were copied from.
    pub from_account_id: Id,
    /// The account the blobs were copied into.
    pub account_id: Id,
    /// Blob ids in the destination account, keyed by their source blob id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied: Option<BTreeMap<Id, Id>>,
    /// Source blob ids that failed to copy, with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_copied: Option<BTreeMap<Id, SetError>>,
}

impl BlobCopyResponse {
    /// Creates a response with nothing copied yet.
    pub fn new(from_account_id: impl Into<Id>, account_id: impl Into<Id>) -> Self {
        Self {
            from_account_id: from_account_id.into(),
            account_id: account_id.into(),
            copied: None,
            not_copied: None,
        }
    }

    /// Sets the copied blob ids.
    pub fn with_copied(mut self, copied: BTreeMap<Id, Id>) -> Self {
        self.copied = Some(copied);
        self
    }

    /// Sets the source blob ids that failed to copy.
    pub fn with_not_copied(mut self, not_copied: BTreeMap<Id, SetError>) -> Self {
        self.not_copied = Some(not_copied);
        self
    }
}
