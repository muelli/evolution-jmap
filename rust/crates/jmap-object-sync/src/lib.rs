// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! The synchronisation steps that `jmap-book-sync` and `jmap-cal-sync` share.
//!
//! Both crates sync one collection (an address book or a calendar) of one object
//! type for an EDS meta backend. They differ in the object type, its conversion
//! and their own rules (photo size, iTIP). An [`ObjectKind`] names the object
//! type, and the functions here do the steps that are the same for both.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use jmap_client::ChangeSet;
use jmap_proto::methods::GetResponse;
use jmap_proto::{Id, State};
use serde_json::{Map, Value};

/// One JMAP object type that a meta backend syncs, and its patch functions.
pub trait ObjectKind {
    /// The JMAP object.
    type Object;
    /// The JMAP type name, for example `ContactCard`.
    const TYPE: &'static str;
    /// The object in log messages, for example `card`.
    const NOUN: &'static str;
    /// The object with its indefinite article, for example `a card`.
    const A_NOUN: &'static str;
    /// The property that files the object in its collection, for example `addressBookIds`.
    const MEMBERSHIP: &'static str;

    fn id(object: &Self::Object) -> Option<&Id>;
    fn membership(object: &Self::Object) -> Option<&BTreeMap<Id, bool>>;
    /// The PatchObject from `current` to `edited`.
    fn diff(current: &Self::Object, edited: &Self::Object) -> Map<String, Value>;
    /// [`ObjectKind::diff`] limited to the properties that differ between `base` and
    /// `edited`, without the properties in `kept`.
    fn diff_since_except(
        base: &Self::Object,
        current: &Self::Object,
        edited: &Self::Object,
        kept: &BTreeSet<String>,
    ) -> Map<String, Value>;
    /// The properties that the user and a different client both changed after `base`.
    fn conflicts(
        base: &Self::Object,
        current: &Self::Object,
        edited: &Self::Object,
    ) -> BTreeSet<String>;
}

/// What a save does with a property that the user and a different client both
/// changed after the base copy (`EConflictResolution`). A property that only one
/// side changed is not a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnConflict {
    /// Send the user's value (`E_CONFLICT_RESOLUTION_KEEP_LOCAL`).
    #[default]
    KeepLocal,
    /// Keep the server's value, and send the other edits (`E_CONFLICT_RESOLUTION_KEEP_SERVER`).
    KeepServer,
    /// Write nothing and fail the save as out of sync (`E_CONFLICT_RESOLUTION_FAIL`).
    Fail,
}

/// A conflict that [`OnConflict::Fail`] refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfSync;

/// What changed in one collection since a given state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes<I> {
    /// The state to pass to the next `get_changes`.
    pub new_state: State,
    /// Objects that were created or modified, already rendered.
    pub changed: Vec<I>,
    /// Identifiers that are no longer in *this* collection, because the server
    /// destroyed them or moved them to a different collection.
    pub removed: Vec<String>,
}

/// The id of `object`, or a protocol error when the server sent none.
pub fn id_of<K: ObjectKind>(object: &K::Object) -> Result<&Id, jmap_client::Error> {
    K::id(object).ok_or_else(|| {
        jmap_client::Error::Protocol(format!(
            "{}/get returned {} without an id",
            K::TYPE,
            K::A_NOUN
        ))
    })
}

/// Whether `object` is filed in the collection `own`.
///
/// The membership property is a set (RFC 8620 §1.5), so a present member is
/// always `true`. A `false` entry violates this. This refuses that entry, as
/// `jmap-mail-sync`'s `messages_since` refuses the same `mailboxIds` violation.
/// No reading of it is safe.
pub fn holds<K: ObjectKind>(object: &K::Object, own: &Id) -> Result<bool, jmap_client::Error> {
    match K::membership(object).and_then(|members| members.get(own).copied()) {
        Some(false) => Err(jmap_client::Error::Protocol(format!(
            "{}/get returned {} with a false-valued {} entry for {}",
            K::TYPE,
            K::id(object)
                .map(Id::to_string)
                .unwrap_or_else(|| "<no id>".to_string()),
            K::MEMBERSHIP,
            own
        ))),
        Some(true) => Ok(true),
        None => Ok(false),
    }
}

/// Turn a raw `/changes` delta into the two lists that the meta backend uses.
///
/// `/changes` is account-wide, so an object can be outside the collection `own`.
/// The created/updated distinction decides such an object without the local cache.
/// An **updated** object that is not in `own` can have moved out, so it is
/// reported as removed. Otherwise, Evolution continues to show it.
/// A **created** object that is not in `own` was never in it, so it is ignored.
///
/// [`jmap_client::Client::all_changes`] normalises the delta across pages,
/// so no object is both a candidate and a removal. `get` loads the candidates.
pub fn classify<K, I, E>(
    delta: ChangeSet,
    own: &Id,
    get: impl FnOnce(&[Id]) -> Result<GetResponse<K::Object>, jmap_client::Error>,
    render: impl Fn(&K::Object) -> Result<I, E>,
) -> Result<Changes<I>, E>
where
    K: ObjectKind,
    E: From<jmap_client::Error>,
{
    let mut removed: Vec<String> = delta.destroyed.iter().map(Id::to_string).collect();
    let candidates: Vec<Id> = delta.created.union(&delta.updated).cloned().collect();
    let mut changed = Vec::new();

    if !candidates.is_empty() {
        let response = get(&candidates)?;
        for object in &response.list {
            let id = id_of::<K>(object)?;
            if holds::<K>(object, own)? {
                changed.push(render(object)?);
            } else if delta.updated.contains(id) {
                removed.push(id.to_string());
            }
        }
        // Destroyed between /changes and /get: report only an object that existed before.
        removed.extend(
            response
                .not_found
                .iter()
                .filter(|id| delta.updated.contains(*id))
                .map(Id::to_string),
        );
    }

    Ok(Changes {
        new_state: delta.new_state,
        changed,
        removed,
    })
}

/// The patch of an edit from `base`, with the conflicts resolved as `on_conflict` asks.
///
/// `base` is the parsed base copy. A base that does not parse gives the plain
/// [`ObjectKind::diff`], so a bad base does not block every save of the object.
pub fn patch_since<K: ObjectKind>(
    base: Result<K::Object, impl Display>,
    on_conflict: OnConflict,
    current: &K::Object,
    edited: &K::Object,
    uid: &str,
) -> Result<Map<String, Value>, OutOfSync> {
    let base = match base {
        Ok(base) => base,
        Err(error) => {
            tracing::warn!(uid, %error, "the base copy of the {} does not parse", K::NOUN);
            return Ok(K::diff(current, edited));
        }
    };
    let conflicts = K::conflicts(&base, current, edited);
    if conflicts.is_empty() {
        return Ok(K::diff_since_except(
            &base,
            current,
            edited,
            &BTreeSet::new(),
        ));
    }
    tracing::info!(uid, ?conflicts, ?on_conflict, "{} edit conflict", K::NOUN);
    match on_conflict {
        OnConflict::KeepLocal => Ok(K::diff_since_except(
            &base,
            current,
            edited,
            &BTreeSet::new(),
        )),
        OnConflict::KeepServer => Ok(K::diff_since_except(&base, current, edited, &conflicts)),
        OnConflict::Fail => Err(OutOfSync),
    }
}

/// The change token for a rendered object.
///
/// The JSContact and JSCalendar `updated` timestamps are optional (RFC 9553,
/// RFC 8984). Without one, every object looks unchanged forever. A digest of the
/// rendered text is always available. It changes only when content that EDS can
/// see changes. Thus, an edit to a property that the mapping drops does not
/// cause a cache reload.
///
/// The EDS cache persists revisions across restarts. `DefaultHasher` output can
/// change between Rust releases, so this uses FNV-1a.
pub fn revision_of(rendered: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in rendered.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The EDS cache keeps revisions across restarts, so the published FNV-1a
    /// 64-bit test vectors pin the digest. A drift makes every cached object look changed.
    #[test]
    fn revision_of_is_fnv1a_64() {
        assert_eq!(revision_of(""), "cbf29ce484222325");
        assert_eq!(revision_of("a"), "af63dc4c8601ec8c");
        assert_eq!(revision_of("foobar"), "85944171f73967e8");
    }
}
