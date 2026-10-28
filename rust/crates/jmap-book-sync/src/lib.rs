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

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use jmap_client::{ChangeSet, Client};
use jmap_object_sync::{ObjectKind, revision_of};
use jmap_proto::contacts::{ContactCard, ContactCardQueryFilter};
use jmap_proto::{Id, State};
use jmap_vcard::{card_to_vcard, vcard_to_card};
use serde_json::{Map, Value};

pub use error::SyncError;
pub use jmap_object_sync::{OnConflict, edit_base};

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

/// The copy of a card that a local edit started from.
///
/// EDS keeps the `extra` string of a cached card through an offline edit.
/// Store [`ContactInfo::vcard`] there, and give it back here on the save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base<'a> {
    /// The vCard of the card as the user saw it before the edit.
    pub vcard: &'a str,
    /// What to do with a property that the server and the user both changed.
    pub on_conflict: OnConflict,
}

/// What changed in the address book since a given state.
pub type Changes = jmap_object_sync::Changes<ContactInfo>;

/// `ContactCard` for the shared synchronisation steps.
enum Cards {}

impl ObjectKind for Cards {
    type Object = ContactCard;
    const TYPE: &'static str = "ContactCard";
    const NOUN: &'static str = "card";
    const A_NOUN: &'static str = "a card";
    const MEMBERSHIP: &'static str = "addressBookIds";

    fn id(card: &ContactCard) -> Option<&Id> {
        card.id.as_ref()
    }

    fn membership(card: &ContactCard) -> Option<&BTreeMap<Id, bool>> {
        card.address_book_ids.as_ref()
    }

    fn diff(current: &ContactCard, edited: &ContactCard) -> Map<String, Value> {
        patch::diff(current, edited)
    }

    fn diff_since_except(
        base: &ContactCard,
        current: &ContactCard,
        edited: &ContactCard,
        kept: &BTreeSet<String>,
    ) -> Map<String, Value> {
        patch::diff_since_except(base, current, edited, kept)
    }

    fn conflicts(
        base: &ContactCard,
        current: &ContactCard,
        edited: &ContactCard,
    ) -> BTreeSet<String> {
        patch::conflicts(base, current, edited)
    }
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
            .map(|card| ContactInfo::render(&self.client, &self.account_id, card))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.state, contacts))
    }

    /// One card by identifier (`load_contact_sync`).
    ///
    /// This does not check membership of this address book. EDS asks by the
    /// identifier it has, and [`BookSync::get_changes`] reports a moved card as removed.
    pub fn load_contact(&self, uid: &str) -> Result<ContactInfo, SyncError> {
        let card = self.fetch(uid)?;
        ContactInfo::render(&self.client, &self.account_id, &card)
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
    /// A property that both changed follows [`Base::on_conflict`].
    pub fn save_contact_since(
        &self,
        base: Option<Base<'_>>,
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
            Some(base) => jmap_object_sync::patch_since::<Cards>(
                vcard_to_card(base.vcard),
                base.on_conflict,
                &current,
                &card,
                uid,
            )
            .map_err(|_| SyncError::OutOfSync(uid.to_owned()))?,
            None => patch::diff(&current, &card),
        };
        if patch.is_empty() {
            return ContactInfo::render(&self.client, &self.account_id, &current);
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

    /// Turn a raw `/changes` delta into the two lists that the meta backend uses
    /// (see [`jmap_object_sync::classify`]).
    fn classify(&self, delta: ChangeSet) -> Result<Changes, SyncError> {
        jmap_object_sync::classify::<Cards, _, _>(
            delta,
            &self.address_book_id,
            |ids| self.client.contact_get(&self.account_id, ids),
            |card| ContactInfo::render(&self.client, &self.account_id, card),
        )
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
    ///
    /// `client` and `account_id` resolve a photo that names a `blob_id` (RFC
    /// 9610 section 2) into an inline `data:` URI; see [`resolve_inline_photos`].
    fn render(client: &Client, account_id: &Id, card: &ContactCard) -> Result<Self, SyncError> {
        let uid = jmap_object_sync::id_of::<Cards>(card)?.to_string();
        let card = resolve_inline_photos(client, account_id, card);
        let vcard = card_to_vcard(&card);
        Ok(Self {
            revision: revision_of(&vcard),
            uid,
            vcard,
        })
    }
}

/// The largest photo this crate downloads for display, in decoded octets.
///
/// Real Stalwart refuses a whole card over 512 KiB (see the comment on
/// [`is_refused_for_size`]'s own test), so a photo past this size could never
/// round-trip as inline base64 in a card anyway.
const MAX_INLINE_PHOTO_BYTES: u64 = 512 * 1024;

/// Give EDS an inline photo for each `photo` media entry that names a `blob_id`
/// (RFC 9610 section 2), downloading its bytes through `client`.
///
/// A photo over [`MAX_INLINE_PHOTO_BYTES`], with no stated `image/*` media
/// type, or whose download fails, keeps its `blob_id` and gets no `uri`:
/// [`card_to_vcard`] already renders such an entry as a hidden entry with no
/// `PHOTO` line, the same fallback it gives a kind or URI scheme it does not
/// understand. A later sync retries the download; nothing here is final.
fn resolve_inline_photos(client: &Client, account_id: &Id, card: &ContactCard) -> ContactCard {
    let mut card = card.clone();
    for (_, entry) in card.media.iter_mut().flatten() {
        if entry.kind.as_deref() != Some("photo") || entry.uri.is_some() {
            continue;
        }
        let Some(blob_id) = entry.blob_id.clone() else {
            continue;
        };
        let Some(media_type) = entry.media_type.clone() else {
            continue;
        };
        if !media_type.starts_with("image/") {
            continue;
        }
        if let Ok(bytes) =
            client.download_blob(account_id, &blob_id, "photo", MAX_INLINE_PHOTO_BYTES)
        {
            entry.uri = Some(format!("data:{media_type};base64,{}", BASE64.encode(bytes)));
        }
    }
    card
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
        .filter_map(|media| media.uri.as_deref())
        .filter(|uri| is_data_uri(uri))
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

/// The `data:` URI marker for base64 content (RFC 2397). It has no case.
const BASE64_MARKER: &str = ";base64";

/// Whether `uri` is a `data:` URI (RFC 2397). A URI scheme has no case (RFC 3986 section 3.1).
fn is_data_uri(uri: &str) -> bool {
    uri.get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
}

/// The size of the content in a `data:` URI (RFC 2397), decoded from base64
/// when the URI declares base64. Other encodings give an approximate size.
/// Every inline `PHOTO` uses base64.
fn decoded_octets(uri: &str) -> u64 {
    let Some((header, payload)) = uri.split_once(',') else {
        return 0;
    };
    let declares_base64 = header.len() >= BASE64_MARKER.len()
        && header.as_bytes()[header.len() - BASE64_MARKER.len()..]
            .eq_ignore_ascii_case(BASE64_MARKER.as_bytes());
    if !declares_base64 {
        return payload.len() as u64;
    }
    let payload = payload.trim_end_matches('=');
    (payload.len() as u64 * 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use jmap_proto::contacts::Media;
    use jmap_proto::error::{RequestError, SetError, set};
    use serde_json::json;

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

    #[test]
    fn a_too_large_photo_with_an_upper_case_scheme_is_blamed() {
        let photo = "aGVsbG8t".repeat(1000);
        let vcard = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Vera\r\nPHOTO:data:image/jpeg;base64,{photo}\r\nEND:VCARD\r\n"
        );
        let mut card = vcard_to_card(&vcard).unwrap();
        for media in card.media.iter_mut().flat_map(BTreeMap::values_mut) {
            media.uri = media.uri.as_ref().map(|u| {
                u.replacen("data:", "DATA:", 1)
                    .replacen(";base64", ";BASE64", 1)
            });
        }
        let error = jmap_client::Error::Set(SetError::new(set::TOO_LARGE));
        assert!(matches!(
            refused(&card, error),
            SyncError::PhotoTooLarge {
                photo_octets: 6_000
            }
        ));
    }

    #[test]
    fn a_data_uri_in_upper_case_measures_its_decoded_content() {
        assert_eq!(decoded_octets("DATA:image/jpeg;BASE64,aGVsbG8="), 5);
        assert_eq!(decoded_octets("Data:image/jpeg;Base64,aGVsbG8="), 5);
    }

    #[test]
    fn a_data_uri_is_recognised_and_others_are_not() {
        assert!(is_data_uri("data:image/png;base64,aGVsbG8="));
        assert!(is_data_uri("DATA:,hello"));
        assert!(!is_data_uri("http://example.test/vera.jpg"));
        assert!(!is_data_uri("urn:uuid:1"));
    }

    #[test]
    fn a_forbidden_error_with_a_size_sounding_description_is_not_one_for_size() {
        // Only `tooLarge` and a size-shaped `invalidProperties` are about size.
        // A `forbidden` refusal that happens to mention a size in its text is not.
        let error = jmap_client::Error::Set(SetError::new(set::FORBIDDEN).with_description(
            "Contact size 844872 exceeds the maximum allowed size of 524288 bytes.",
        ));
        assert!(!is_refused_for_size(&error));
    }

    #[test]
    fn exactly_half_the_card_in_photos_is_blamed_on_the_photo() {
        // `refused` blames the photo once inline photos are "at least half" the
        // card (see its doc comment); the boundary must round in that direction.
        const PREFIX: &str = "data:image/png;base64,";
        let mut media = BTreeMap::new();
        media.insert("m".to_owned(), Media::new(PREFIX).with_kind("photo"));
        let mut card = ContactCard::simple("b", "Vera", "vera@example.test");
        card.media = Some(media.clone());
        let card_octets_at_zero_payload = serde_json::to_vec(&card).unwrap().len();
        // Solve for a payload length N so that `uri.len() * 2 == card_octets`
        // exactly, given how N feeds back into the card's own serialized size.
        let payload_len = card_octets_at_zero_payload - 2 * PREFIX.len();
        let uri = format!("{PREFIX}{}", "A".repeat(payload_len));
        media.insert("m".to_owned(), Media::new(uri.clone()).with_kind("photo"));
        card.media = Some(media);
        let card_octets = serde_json::to_vec(&card).unwrap().len();
        assert_eq!(
            uri.len() * 2,
            card_octets,
            "test setup must hit the boundary exactly"
        );

        let error = jmap_client::Error::Set(SetError::new(set::TOO_LARGE));
        assert!(matches!(
            refused(&card, error),
            SyncError::PhotoTooLarge { .. }
        ));
    }
}
