// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implement contact methods (`AddressBook/get`, `ContactCard/get|set|query`,
//! RFC 9610) and contact seed helpers.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use jmap_proto::Id;
use jmap_proto::contacts::{
    AddressBook, AddressBookRights, ContactCard, ContactCardQueryFilter, Media,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use percent_encoding::percent_decode_str;
use serde_json::Value;

#[cfg(feature = "parse")]
use crate::dispatch::project_properties;
use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::setops::{simple_set, simple_set_with_validation};
use crate::state::{AccountState, ServerState};

const HOSTILE_CONTACT_PHOTO_URI: &str = "file:///home/victim/.ssh/id_ed25519";
const DATA_URI_PREFIX: &str = "data:";
const DEFAULT_DATA_URI_MEDIA_TYPE: &str = "text/plain;charset=US-ASCII";

/// `caller` comes from [`crate::auth::AuthConfig::identity_for`].
/// `None` means owner access. Non-owners see only shared books.
pub fn address_book_get(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let is_owner =
        caller.is_none_or(|caller| account.current_user_principal_id.as_ref() == Some(caller));
    if !is_owner {
        let caller = caller.expect("is_owner is false only when caller is Some");
        let shared_with_caller = account
            .address_books
            .iter()
            .any(|(_, book)| book_rights_for(book, caller).is_some());
        if !shared_with_caller {
            return Err(MethodError::new(error::method::FORBIDDEN)
                .with_description("no address book in this account is shared with you"));
        }
    }

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            for (_, book) in account.address_books.iter() {
                if let Some(visible) = visible_book(book, is_owner, caller) {
                    list.push(visible);
                }
            }
        }
        Some(ids) => {
            for id in ids {
                match account.address_books.get(id) {
                    Some(book) => match visible_book(book, is_owner, caller) {
                        Some(visible) => list.push(visible),
                        None => not_found.push(id.clone()),
                    },
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(
        &GetResponse::new(request.account_id, account.address_books.state(), list)
            .with_not_found(not_found),
    )
}

/// Return rights that `book.share_with` grants to `principal`.
fn book_rights_for(
    book: &AddressBook,
    principal: &Id,
) -> Option<jmap_proto::contacts::AddressBookRights> {
    book.share_with.as_ref()?.get(principal).cloned()
}

/// Owners see all books unchanged.
/// Non-owners see only shared books with caller-specific `myRights`.
fn visible_book(book: &AddressBook, is_owner: bool, caller: Option<&Id>) -> Option<AddressBook> {
    if is_owner {
        return Some(book.clone());
    }
    let caller = caller.expect("is_owner is false only when caller is Some");
    let rights = book_rights_for(book, caller)?;
    let mut visible = book.clone();
    visible.my_rights = Some(rights);
    Some(visible)
}

/// `AddressBook/set` (RFC 9610 §2): create, update, and destroy books.
/// This mock requires a non-empty `name` and a server-set `id` on create.
pub fn address_book_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<AddressBook> = parse_arguments(arguments)?;
    let default_unsubscribed = state.new_collections_default_unsubscribed;
    let terse_collection_create = state.terse_collection_create;
    let id_omitting_collection_create = state.id_omitting_collection_create;
    let reject_collection_create = state.reject_collection_create;
    let account_id = request.account_id.clone();
    let account = account_mut(state, &account_id)?;

    // Keep pre-update `shareWith` values for `ShareNotification` comparisons.
    let old_share_with: BTreeMap<Id, Option<BTreeMap<Id, AddressBookRights>>> = request
        .update
        .iter()
        .flatten()
        .filter_map(|(id, _)| {
            account
                .address_books
                .get(id)
                .map(|book| (id.clone(), book.share_with.clone()))
        })
        .collect();

    let response = simple_set(&mut account.address_books, request, |id, book| {
        if book.id.is_some() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("id is set by the server and must not be given in a create"));
        }
        if book.name.is_empty() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("name must not be empty"));
        }
        if reject_collection_create {
            return Err(SetError::new(error::set::FORBIDDEN)
                .with_description("this mock is configured to refuse address book creates"));
        }
        book.id = Some(id.clone());
        if default_unsubscribed && book.is_subscribed != Some(true) {
            book.is_subscribed = Some(false);
        }
        Ok(())
    })?;

    if let Some(updated) = &response.updated {
        for id in updated.keys() {
            let new_share_with = account
                .address_books
                .get(id)
                .and_then(|book| book.share_with.clone());
            crate::principals::record_share_changes(
                account,
                jmap_proto::principals::share_notification_object_type::ADDRESS_BOOK,
                id,
                &account_id,
                old_share_with.get(id).and_then(Option::as_ref),
                new_share_with.as_ref(),
            );
        }
    }

    let mut result = to_result(&response)?;

    // RFC 8620 §5.3 allows omitting client-supplied `created` properties.
    // `terse_collection_create` keeps only `id`, `isDefault`, and `myRights`.
    if terse_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("name");
            }
        }
    }

    // Simulate a noncompliant server that omits `id` in `created`.
    if id_omitting_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("id");
            }
        }
    }

    Ok(result)
}

pub fn contact_card_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;

    // Enforce `maxObjectsInGet`, as other `/get` handlers do.
    let limit = state.objects_in_get();
    if request
        .ids
        .as_ref()
        .is_some_and(|ids| ids.len() as u64 > limit)
    {
        return Err(MethodError::new(error::method::REQUEST_TOO_LARGE)
            .with_description(format!("ContactCard/get accepts at most {limit} ids")));
    }

    let contact_photo_uses_file_uri = state.contact_photo_uses_file_uri;
    let contact_photo_data_uri_as_blob_id = state.contact_photo_data_uri_as_blob_id;
    let account = account_mut(state, &request.account_id)?;

    if contact_photo_data_uri_as_blob_id {
        let card_ids: Vec<Id> = match &request.ids {
            Some(ids) => ids.clone(),
            None => account
                .contact_cards
                .iter()
                .map(|(id, _)| id.clone())
                .collect(),
        };
        promote_cards_photo_data_uris(account, card_ids);
    }

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(account.contact_cards.iter().map(|(_, card)| card.clone())),
        Some(ids) => {
            for id in ids {
                match account.contact_cards.get(id) {
                    Some(card) => list.push(card.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    if contact_photo_uses_file_uri {
        for card in &mut list {
            rewrite_photo_uri_to_file_scheme(card);
        }
    }

    to_result(
        &GetResponse::new(request.account_id, account.contact_cards.state(), list)
            .with_not_found(not_found),
    )
}

fn rewrite_photo_uri_to_file_scheme(card: &mut ContactCard) {
    let Some(media) = card.media.as_mut() else {
        return;
    };

    for entry in media.values_mut() {
        if is_photo_media(entry) {
            entry.uri = Some(HOSTILE_CONTACT_PHOTO_URI.to_owned());
            entry.blob_id = None;
        }
    }
}

fn is_photo_media(entry: &Media) -> bool {
    entry
        .kind
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("photo"))
}

pub fn contact_card_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<ContactCard> = parse_arguments(arguments)?;
    let contact_photo_data_uri_as_blob_id = state.contact_photo_data_uri_as_blob_id;
    let max_contact_size = state.max_contact_size;
    let account = account_mut(state, &request.account_id)?;
    let known_uids = known_contact_uids(account);

    let address_books = &account.address_books;
    let mut response =
        simple_set_with_validation(
            &mut account.contact_cards,
            request,
            |id, card| {
                // RFC 8620 §5.3: `id` is server-set and must not appear in create.
                if card.id.is_some() {
                    return Err(
                        SetError::new(error::set::INVALID_PROPERTIES).with_description(
                            "id is set by the server and must not be given in a create",
                        ),
                    );
                }
                if let Some(limit) = max_contact_size {
                    let size = serde_json::to_vec(card).map_or(0, |json| json.len() as u64);
                    if size > limit {
                        return Err(SetError::new(error::set::TOO_LARGE).with_description(
                            format!("{size} octets is more than the {limit} a card may take"),
                        ));
                    }
                }
                let Some(book_ids) = card
                    .address_book_ids
                    .as_ref()
                    .filter(|book_ids| !book_ids.is_empty())
                else {
                    return Err(SetError::new(error::set::INVALID_PROPERTIES)
                        .with_description("addressBookIds must name at least one address book"));
                };
                if let Some(unknown) = book_ids
                    .keys()
                    .find(|book_id| !address_books.contains(book_id))
                {
                    return Err(SetError::new(error::set::INVALID_PROPERTIES)
                        .with_description(format!("address book {unknown} does not exist")));
                }
                card.id = Some(id.clone());
                if card.card_type.is_none() {
                    card.card_type = Some("Card".to_owned());
                }
                if card.version.is_none() {
                    card.version = Some("1.0".to_owned());
                }
                if card.uid.is_none() {
                    card.uid = Some(format!("urn:example:card:{}", id.as_str()));
                }
                validate_members_keys_are_known_uids(card, &known_uids)
            },
            |_, _, patched| validate_members_keys_are_known_uids(patched, &known_uids),
            |_, _| Ok(()),
        )?;

    if contact_photo_data_uri_as_blob_id {
        promote_photo_data_uris_in_set_response(account, &mut response);
    }

    let mut result = to_result(&response)?;

    // RFC 8620 §5.3 allows omitting client `created` properties.
    // `terse_contact_create` keeps only `id`.
    if state.terse_contact_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(id) = object.get("id").cloned() {
                *object = Value::Object(serde_json::Map::from_iter([("id".to_owned(), id)]));
            }
        }
    }

    Ok(result)
}

fn known_contact_uids(account: &AccountState) -> BTreeSet<String> {
    account
        .contact_cards
        .iter()
        .filter_map(|(_, card)| card.uid.clone())
        .collect()
}

fn validate_members_keys_are_known_uids(
    card: &ContactCard,
    known_uids: &BTreeSet<String>,
) -> Result<(), SetError> {
    for (uid, is_member) in card.members.iter().flatten() {
        if !*is_member {
            continue;
        }
        if uid.trim().is_empty() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("members keys must be non-empty card uid strings"));
        }
        let is_self_uid = card.uid.as_deref().is_some_and(|self_uid| self_uid == uid);
        if !is_self_uid && !known_uids.contains(uid) {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description(format!("members key {uid} does not match any card uid")));
        }
    }
    Ok(())
}

fn promote_photo_data_uris_in_set_response(
    account: &mut AccountState,
    response: &mut SetResponse<ContactCard>,
) {
    let mut card_ids: Vec<Id> = response
        .created
        .as_ref()
        .into_iter()
        .flat_map(|created| created.values())
        .filter_map(|card| card.id.clone())
        .collect();

    card_ids.extend(
        response
            .updated
            .as_ref()
            .into_iter()
            .flat_map(|updated| updated.keys().cloned()),
    );

    promote_cards_photo_data_uris(account, card_ids);

    if let Some(created) = response.created.as_mut() {
        for card in created.values_mut() {
            if let Some(id) = card.id.as_ref()
                && let Some(stored) = account.contact_cards.get(id)
            {
                *card = stored.clone();
            }
        }
    }
}

fn promote_cards_photo_data_uris(account: &mut AccountState, card_ids: Vec<Id>) {
    for id in card_ids {
        promote_card_photo_data_uris(account, &id);
    }
}

fn promote_card_photo_data_uris(account: &mut AccountState, card_id: &Id) {
    let replacements = account
        .contact_cards
        .get(card_id)
        .map(planned_photo_data_uri_replacements)
        .unwrap_or_default();
    if replacements.is_empty() {
        return;
    }

    let replacements: Vec<(String, Id, String)> = replacements
        .into_iter()
        .map(|(key, media_type, bytes)| {
            let blob_id = account.add_blob(media_type.clone(), bytes);
            (key, blob_id, media_type)
        })
        .collect();

    if let Some(card) = account.contact_cards.get_mut(card_id)
        && let Some(media) = card.media.as_mut()
    {
        for (key, blob_id, media_type) in replacements {
            let Some(entry) = media.get_mut(&key) else {
                continue;
            };
            if !is_photo_media(entry) {
                continue;
            }
            if entry.blob_id.is_some() {
                continue;
            }
            if !entry
                .uri
                .as_deref()
                .is_some_and(|uri| uri.starts_with(DATA_URI_PREFIX))
            {
                continue;
            }

            entry.uri = None;
            entry.blob_id = Some(blob_id);
            if entry.media_type.is_none() {
                entry.media_type = Some(media_type);
            }
        }
    }
}

fn planned_photo_data_uri_replacements(card: &ContactCard) -> Vec<(String, String, Vec<u8>)> {
    let Some(media) = card.media.as_ref() else {
        return Vec::new();
    };

    media
        .iter()
        .filter_map(|(key, entry)| {
            if !is_photo_media(entry) {
                return None;
            }
            if entry.blob_id.is_some() {
                return None;
            }
            let uri = entry.uri.as_ref()?;
            parse_data_uri(uri).map(|(media_type, bytes)| (key.clone(), media_type, bytes))
        })
        .collect()
}

fn parse_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    let encoded = uri.strip_prefix(DATA_URI_PREFIX)?;
    let (metadata, payload) = encoded.split_once(',')?;

    let mut base64 = false;
    let mut media_parts = Vec::new();
    for part in metadata.split(';') {
        if part.eq_ignore_ascii_case("base64") {
            base64 = true;
            continue;
        }
        if !part.is_empty() {
            media_parts.push(part);
        }
    }

    let media_type = if media_parts.is_empty() {
        DEFAULT_DATA_URI_MEDIA_TYPE.to_owned()
    } else if media_parts[0].contains('/') {
        media_parts.join(";")
    } else {
        format!("text/plain;{}", media_parts.join(";"))
    };

    let bytes = if base64 {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?
    } else {
        percent_decode_str(payload).collect()
    };

    Some((media_type, bytes))
}

#[cfg(feature = "parse")]
fn apply_group_data_from_vcard(card: &mut ContactCard, text: &str) {
    let mut parsed_kind = None;
    let mut parsed_members = BTreeMap::new();

    for line in unfold_vcard_lines(text) {
        let Some((name_and_params, value)) = line.split_once(':') else {
            continue;
        };
        let property_name = name_and_params.split(';').next().unwrap_or_default().trim();
        let value = value.trim();

        if property_name.eq_ignore_ascii_case("KIND")
            || property_name.eq_ignore_ascii_case("X-ADDRESSBOOKSERVER-KIND")
        {
            if !value.is_empty() {
                parsed_kind = Some(value.to_ascii_lowercase());
            }
            continue;
        }

        if property_name.eq_ignore_ascii_case("MEMBER") {
            if let Some(uid) = member_uid_from_member_property(value) {
                parsed_members.insert(uid, true);
            }
            continue;
        }

        if property_name.eq_ignore_ascii_case("X-ADDRESSBOOKSERVER-MEMBER") && !value.is_empty() {
            parsed_members.insert(value.to_owned(), true);
        }
    }

    if let Some(kind) = parsed_kind {
        card.kind = Some(kind);
    }
    if !parsed_members.is_empty() {
        card.members = Some(parsed_members);
    }
}

#[cfg(feature = "parse")]
fn member_uid_from_member_property(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    value
        .to_ascii_lowercase()
        .starts_with("urn:")
        .then(|| value.to_owned())
}

#[cfg(feature = "parse")]
fn unfold_vcard_lines(text: &str) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(current) = unfolded.last_mut()
            && (line.starts_with(' ') || line.starts_with('\t'))
        {
            current.push_str(line.trim_start_matches([' ', '\t']));
        } else {
            unfolded.push(line.to_owned());
        }
    }

    unfolded
}

pub fn contact_card_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<ContactCardQueryFilter> = parse_arguments(arguments)?;
    let paging = state.query_paging();
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let (ids, limit) = paging.page(
        account
            .contact_cards
            .iter()
            .filter(|(_, card)| card_matches(card, &filter))
            .map(|(id, _)| id.clone()),
        request.position,
        request.limit,
    );

    let total = account
        .contact_cards
        .iter()
        .filter(|(_, card)| card_matches(card, &filter))
        .count() as u64;

    let mut response = QueryResponse::new(request.account_id, account.contact_cards.state(), ids)
        .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    if let Some(limit) = limit {
        response = response.with_limit(limit);
    }
    to_result(&response)
}

/// `ContactCard/parse` (RFC 9610 §3.4): parse vCard blobs to projected
/// `ContactCard` responses without storing them.
pub fn contact_card_parse(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    #[cfg(not(feature = "parse"))]
    {
        let _ = state;
        let _ = arguments;
        Err(MethodError::new(error::method::UNKNOWN_METHOD))
    }

    #[cfg(feature = "parse")]
    {
        let request: jmap_proto::contacts::ContactCardParseRequest = parse_arguments(arguments)?;
        let account = account_mut(state, &request.account_id)?;

        let mut parsed = serde_json::Map::new();
        let mut not_found = Vec::new();
        let mut not_parsable = Vec::new();
        for id in &request.blob_ids {
            let Some(blob) = account.blobs.get(id) else {
                not_found.push(id.clone());
                continue;
            };
            let Ok(text) = std::str::from_utf8(&blob.data) else {
                not_parsable.push(id.clone());
                continue;
            };
            match jmap_vcard::vcard_to_card(text) {
                Ok(mut card) => {
                    apply_group_data_from_vcard(&mut card, text);
                    parsed.insert(
                        id.to_string(),
                        project_properties(&card, request.properties.as_deref())?,
                    );
                }
                Err(_) => not_parsable.push(id.clone()),
            }
        }

        to_result(&serde_json::json!({
            "accountId": request.account_id,
            "parsed": (!parsed.is_empty()).then_some(Value::Object(parsed)),
            "notParsable": (!not_parsable.is_empty()).then_some(not_parsable),
            "notFound": (!not_found.is_empty()).then_some(not_found),
        }))
    }
}

fn card_matches(card: &ContactCard, filter: &ContactCardQueryFilter) -> bool {
    if let Some(book_id) = &filter.in_address_book
        && !card
            .address_book_ids
            .as_ref()
            .is_some_and(|ids| ids.get(book_id).copied().unwrap_or(false))
    {
        return false;
    }
    let full_name = card
        .name
        .as_ref()
        .and_then(|name| name.full.clone())
        .unwrap_or_default();
    let emails: Vec<&str> = card
        .emails
        .iter()
        .flat_map(|emails| emails.values())
        .map(|email| email.address.as_str())
        .collect();
    if let Some(name) = &filter.name
        && !full_name.contains(name.as_str())
    {
        return false;
    }
    if let Some(email) = &filter.email
        && !emails
            .iter()
            .any(|address| address.contains(email.as_str()))
    {
        return false;
    }
    if let Some(kind) = &filter.kind
        && !card
            .kind
            .as_deref()
            .is_some_and(|card_kind| card_kind.eq_ignore_ascii_case(kind.as_str()))
    {
        return false;
    }
    if let Some(text) = &filter.text
        && !(full_name.contains(text.as_str())
            || emails.iter().any(|address| address.contains(text.as_str())))
    {
        return false;
    }
    true
}

impl AccountState {
    /// Seed an address book and return its id without bumping state.
    pub fn seed_address_book(&mut self, name: &str, is_default: bool) -> Id {
        let id = self.address_books.alloc_id();
        let book = AddressBook::new(name)
            .with_id(id.clone())
            .is_default(is_default)
            .is_subscribed(true);
        self.address_books.seed_with_id(id.clone(), book);
        id
    }
}
