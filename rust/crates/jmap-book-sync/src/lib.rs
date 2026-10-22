// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Address book synchronisation, in the shape that `EBookMetaBackend` requires.
//!
//! One EDS address book source is one JMAP address book. This crate lists the
//! cards, renders a card as a vCard, reads the changes since a state string,
//! and turns an Evolution edit into a `ContactCard/set`.
//! Each entry point matches one vfunc: [`BookSync::list_existing`] matches
//! `list_existing_sync`, [`BookSync::save_contact`] matches `save_contact_sync`,
//! and so on.
//!
//! The crate has no GObject or Evolution header dependency, so tests can run
//! it against `jmap-mockd` on any machine. The subclass keeps only lifecycle
//! and marshalling.

pub mod error;
pub mod patch;

use std::collections::BTreeMap;

use jmap_client::{ChangeSet, Client};
use jmap_proto::contacts::{ContactCard, ContactCardQueryFilter};
use jmap_proto::{Id, State};
use jmap_vcard::{card_to_vcard, vcard_to_card};
use serde_json::Value;

pub use error::SyncError;

/// One contact for the meta backend: an identifier, a change token and the vCard.
///
/// This is the payload of an `EBookMetaBackendInfo`. `uid` is the JMAP id, not
/// the JSContact `uid`. `jmap_vcard::card_to_vcard` gives the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    pub uid: String,
    pub revision: String,
    pub vcard: String,
}

/// What changed in the address book since a given state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The state to pass to the next [`BookSync::get_changes`].
    pub new_state: State,
    /// Cards that were created or modified, already rendered.
    pub changed: Vec<ContactInfo>,
    /// Identifiers that are no longer in *this* address book, because the
    /// server destroyed them or moved them to a different book.
    pub removed: Vec<String>,
}

/// Synchronises one JMAP address book.
pub struct BookSync {
    client: Client,
    account_id: Id,
    address_book_id: Id,
}

impl BookSync {
    pub fn new(client: Client, account_id: Id, address_book_id: Id) -> Self {
        Self {
            client,
            account_id,
            address_book_id,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn account_id(&self) -> &Id {
        &self.account_id
    }

    pub fn address_book_id(&self) -> &Id {
        &self.address_book_id
    }

    /// Every card in this address book, with the state of that listing
    /// (`list_existing_sync`).
    pub fn list_existing(&self) -> Result<(State, Vec<ContactInfo>), SyncError> {
        let ids = self.client.contact_query_every_id(
            &self.account_id,
            ContactCardQueryFilter::in_address_book(self.address_book_id.clone()),
        )?;
        let response = self.client.contact_get(&self.account_id, &ids)?;
        let contacts = response
            .list
            .iter()
            .map(ContactInfo::render)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.state, contacts))
    }

    /// One card by identifier (`load_contact_sync`).
    ///
    /// This does not check membership of this address book. EDS asks by the
    /// identifier it has, and [`BookSync::get_changes`] reports a moved card as removed.
    pub fn load_contact(&self, uid: &str) -> Result<ContactInfo, SyncError> {
        ContactInfo::render(&self.fetch(uid)?)
    }

    /// Store a vCard (`save_contact_sync`).
    ///
    /// With no `existing_uid`, this is a create. The vCard's `UID` is a local
    /// Evolution name (`pas-id-…`), so this drops it and the server assigns the id.
    /// Otherwise this is an edit: a PatchObject that names only the properties
    /// the vCard mapping covers (see [`patch`]).
    pub fn save_contact(
        &self,
        vcard: &str,
        existing_uid: Option<&str>,
    ) -> Result<ContactInfo, SyncError> {
        self.save_contact_since(None, vcard, existing_uid)
    }

    /// [`BookSync::save_contact`] for an edit of `base`, the vCard that the user edited.
    ///
    /// The edit patches only the properties that differ from `base`. Each other
    /// property keeps the value on the server, also when a different client changed it.
    pub fn save_contact_since(
        &self,
        base: Option<&str>,
        vcard: &str,
        existing_uid: Option<&str>,
    ) -> Result<ContactInfo, SyncError> {
        let mut card = vcard_to_card(vcard)?;
        let Some(uid) = existing_uid else {
            card.id = None;
            card.address_book_ids = Some(BTreeMap::from([(self.address_book_id.clone(), true)]));
            let account_id = self.account_id().to_string();
            let address_book_id = self.address_book_id().to_string();
            tracing::debug!(account_id, address_book_id, "creating contact card");
            let stored = match self.client.contact_create(&self.account_id, &card) {
                Ok(stored) => stored,
                Err(error) => {
                    tracing::warn!(
                        account_id,
                        address_book_id,
                        %error,
                        "contact card create failed"
                    );
                    return Err(refused(&card, error));
                }
            };
            // RFC 8620 §5.3 requires only the properties the server set, so
            // `stored` can hold only `id` (see `tests/terse_create.rs`).
            // A fresh load gives EDS the card as the server stored it.
            let id = stored
                .id
                .as_ref()
                .ok_or_else(|| SyncError::protocol("ContactCard/set created a card without an id"))?
                .to_string();
            return self.load_contact(&id);
        };

        let current = self.fetch(uid)?;
        let patch = match base {
            Some(base) => patch::diff_since(&vcard_to_card(base)?, &current, &card),
            None => patch::diff(&current, &card),
        };
        if patch.is_empty() {
            return ContactInfo::render(&current);
        }
        let account_id = self.account_id().to_string();
        let address_book_id = self.address_book_id().to_string();
        tracing::debug!(account_id, address_book_id, uid, "updating contact card");
        if let Err(error) =
            self.client
                .contact_update(&self.account_id, &Id::from(uid), Value::Object(patch))
        {
            tracing::warn!(
                account_id,
                address_book_id,
                uid,
                %error,
                "contact card update failed"
            );
            return Err(refused(&card, error));
        }
        self.load_contact(uid)
    }

    /// Destroy a card (`remove_contact_sync`).
    pub fn remove_contact(&self, uid: &str) -> Result<(), SyncError> {
        let account_id = self.account_id().to_string();
        let address_book_id = self.address_book_id().to_string();
        tracing::debug!(account_id, address_book_id, uid, "removing contact card");
        if let Err(error) = self
            .client
            .contact_destroy(&self.account_id, &Id::from(uid))
        {
            tracing::warn!(
                account_id,
                address_book_id,
                uid,
                %error,
                "contact card destroy failed"
            );
            return Err(error.into());
        }
        Ok(())
    }

    /// What changed since `since` (`get_changes_sync`).
    ///
    /// If the state is too old for the server, this fails with a
    /// [`SyncError::is_cannot_calculate_changes`] error. The caller then lists the full book.
    pub fn get_changes(&self, since: &State) -> Result<Changes, SyncError> {
        self.classify(
            self.client
                .all_changes(&self.account_id, "ContactCard", since)?,
        )
    }

    /// Turn a raw `/changes` delta into the two lists that the meta backend uses.
    ///
    /// `ContactCard/changes` is account-wide, so a card can be outside this book.
    /// The created/updated distinction decides such a card without the local cache.
    /// An **updated** card that is not in this book can have moved out, so it is
    /// reported as removed. Otherwise, Evolution continues to show it.
    /// A **created** card that is not in this book was never in it, so it is ignored.
    ///
    /// [`jmap_client::Client::all_changes`] normalises the delta across pages,
    /// so no card is both a candidate and a removal.
    fn classify(&self, delta: ChangeSet) -> Result<Changes, SyncError> {
        let mut removed: Vec<String> = delta.destroyed.iter().map(Id::to_string).collect();
        let candidates: Vec<Id> = delta.created.union(&delta.updated).cloned().collect();
        let mut changed = Vec::new();

        if !candidates.is_empty() {
            let response = self.client.contact_get(&self.account_id, &candidates)?;
            for card in &response.list {
                let Some(id) = &card.id else {
                    return Err(SyncError::protocol(
                        "ContactCard/get returned a card without an id",
                    ));
                };
                if self.holds(card)? {
                    changed.push(ContactInfo::render(card)?);
                } else if delta.updated.contains(id) {
                    removed.push(id.to_string());
                }
            }
            // Destroyed between /changes and /get: report only a card that existed before.
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

    /// Whether `card` is filed in the address book this instance syncs.
    ///
    /// RFC 9610's `addressBookIds` is a set (RFC 8620 §1.5), so a present
    /// member is always `true`. A `false` entry violates this.
    /// This refuses that entry, as `jmap-mail-sync`'s `messages_since` refuses
    /// the same `mailboxIds` violation. No reading of it is safe.
    fn holds(&self, card: &ContactCard) -> Result<bool, SyncError> {
        match card
            .address_book_ids
            .as_ref()
            .and_then(|books| books.get(&self.address_book_id).copied())
        {
            Some(false) => Err(SyncError::protocol(format!(
                "ContactCard/get returned {} with a false-valued addressBookIds entry for {}",
                card.id
                    .as_ref()
                    .map(Id::to_string)
                    .unwrap_or_else(|| "<no id>".to_string()),
                self.address_book_id
            ))),
            Some(true) => Ok(true),
            None => Ok(false),
        }
    }

    fn fetch(&self, uid: &str) -> Result<ContactCard, SyncError> {
        let id = Id::from(uid);
        let response = self
            .client
            .contact_get(&self.account_id, std::slice::from_ref(&id))?;
        response
            .list
            .into_iter()
            .next()
            .ok_or_else(|| SyncError::NotFound(uid.to_owned()))
    }
}

impl ContactInfo {
    /// Render a card, deriving its revision from the result.
    fn render(card: &ContactCard) -> Result<Self, SyncError> {
        let uid = card
            .id
            .as_ref()
            .ok_or_else(|| SyncError::protocol("ContactCard/get returned a card without an id"))?
            .to_string();
        let vcard = card_to_vcard(card);
        Ok(Self {
            revision: revision_of(&vcard),
            uid,
            vcard,
        })
    }
}

/// The change token for a rendered card.
///
/// JSContact's `updated` timestamp is optional (RFC 9553). Without it, every
/// card looks unchanged forever. A digest of the vCard is always available.
/// The digest changes only when content that EDS can see changes. Thus, an
/// edit to a property that the mapping drops does not cause a cache reload.
///
/// The EDS cache persists revisions across restarts. `DefaultHasher` output can
/// change between Rust releases, so this uses FNV-1a.
fn revision_of(vcard: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in vcard.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What to report for a save of `card` that the server refused with `error`.
///
/// A size refusal becomes [`SyncError::PhotoTooLarge`] when the inline photos
/// are at least half of the card. Then the user must change the photo.
/// RFC 9610 advertises no card size limit, so the client cannot check it before
/// sending. All other refusals keep the server's error.
fn refused(card: &ContactCard, error: jmap_client::Error) -> SyncError {
    let photos: Vec<&str> = card
        .media
        .iter()
        .flat_map(|media| media.values())
        .filter(|media| media.kind.as_deref() == Some("photo"))
        .map(|media| media.uri.as_str())
        .filter(|uri| uri.starts_with("data:"))
        .collect();
    let inline_octets: usize = photos.iter().map(|uri| uri.len()).sum();
    let card_octets = serde_json::to_vec(card).map_or(0, |json| json.len());
    if inline_octets == 0 || inline_octets * 2 < card_octets || !is_refused_for_size(&error) {
        return error.into();
    }
    let photo_octets = photos.iter().map(|uri| decoded_octets(uri)).sum();
    SyncError::PhotoTooLarge { photo_octets }
}

/// Whether `error` refuses a request or an object for its size.
fn is_refused_for_size(error: &jmap_client::Error) -> bool {
    use jmap_proto::error::set;
    match error {
        // RFC 8620 section 5.3: the object exceeds a server limit.
        jmap_client::Error::Set(error) if error.error_type == set::TOO_LARGE => true,
        // Real Stalwart (0.16) refuses a card over its limit (512 KiB) as
        // `invalidProperties` with no `properties`. Only the description gives
        // the reason: "Contact size 844872 exceeds the maximum allowed size
        // of 524288 bytes."
        jmap_client::Error::Set(error) if error.error_type == set::INVALID_PROPERTIES => {
            error.properties.as_deref().is_none_or(<[String]>::is_empty)
                && error
                    .description
                    .as_deref()
                    .is_some_and(|text| text.contains("exceeds the maximum allowed size"))
        }
        // RFC 8620 section 2: over `maxSizeRequest`. The client refuses this
        // before sending. The server refusal below is the same limit.
        jmap_client::Error::RequestTooLarge { .. } => true,
        jmap_client::Error::Http {
            status: 400,
            problem: Some(problem),
        } => {
            problem.error_type == "urn:ietf:params:jmap:error:limit"
                && problem.extra.get("limit").and_then(Value::as_str) == Some("maxSizeRequest")
        }
        _ => false,
    }
}

/// The size of the content in a `data:` URI (RFC 2397), decoded from base64
/// when the URI declares base64. Other encodings give an approximate size.
/// Every inline `PHOTO` uses base64.
fn decoded_octets(uri: &str) -> u64 {
    let Some((header, payload)) = uri.split_once(',') else {
        return 0;
    };
    if !header.ends_with(";base64") {
        return payload.len() as u64;
    }
    let payload = payload.trim_end_matches('=');
    (payload.len() as u64 * 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use jmap_proto::error::{RequestError, SetError, set};
    use serde_json::json;

    /// The EDS cache keeps revisions across restarts, so the published FNV-1a
    /// 64-bit test vectors pin the digest. A drift makes every cached card look changed.
    #[test]
    fn revision_of_is_fnv1a_64() {
        assert_eq!(revision_of(""), "cbf29ce484222325");
        assert_eq!(revision_of("a"), "af63dc4c8601ec8c");
        assert_eq!(revision_of("foobar"), "85944171f73967e8");
    }

    #[test]
    fn real_stalwarts_contact_size_refusal_is_one_for_size() {
        let error =
            jmap_client::Error::Set(SetError::new(set::INVALID_PROPERTIES).with_description(
                "Contact size 844872 exceeds the maximum allowed size of 524288 bytes.",
            ));
        assert!(is_refused_for_size(&error));
    }

    #[test]
    fn invalid_properties_naming_a_property_is_not_one_for_size() {
        let mut refusal = SetError::new(set::INVALID_PROPERTIES).with_description(
            "Contact size 844872 exceeds the maximum allowed size of 524288 bytes.",
        );
        refusal.properties = Some(vec!["name".into()]);
        assert!(!is_refused_for_size(&jmap_client::Error::Set(refusal)));
        let other = SetError::new(set::INVALID_PROPERTIES).with_description("bad name");
        assert!(!is_refused_for_size(&jmap_client::Error::Set(other)));
    }

    #[test]
    fn only_the_max_size_request_limit_problem_is_one_for_size() {
        let problem = |limit: &str| jmap_client::Error::Http {
            status: 400,
            problem: Some({
                let mut problem = RequestError::new("urn:ietf:params:jmap:error:limit");
                problem.extra.insert("limit".into(), json!(limit));
                problem
            }),
        };
        assert!(is_refused_for_size(&problem("maxSizeRequest")));
        assert!(!is_refused_for_size(&problem("maxCallsInRequest")));
        assert!(!is_refused_for_size(&jmap_client::Error::Http {
            status: 400,
            problem: None,
        }));
    }

    #[test]
    fn a_data_uri_measures_its_decoded_content() {
        assert_eq!(
            decoded_octets("data:image/jpeg;base64,aGVsbG8tcGhvdG8="),
            11
        );
        assert_eq!(decoded_octets("data:image/jpeg;base64,aGVsbG8="), 5);
        assert_eq!(decoded_octets("data:,hello"), 5);
        assert_eq!(decoded_octets("data:nothing"), 0);
    }
}
