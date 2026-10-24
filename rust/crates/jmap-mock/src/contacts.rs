// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implement contact methods (`AddressBook/get`, `ContactCard/get|set|query`,
//! RFC 9610) and contact seed helpers.

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::contacts::{
    AddressBook, AddressBookRights, ContactCard, ContactCardParseRequest, ContactCardQueryFilter,
    Media,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::setops::simple_set;
use crate::state::{AccountState, ServerState};

const HOSTILE_CONTACT_PHOTO_URI: &str = "file:///home/victim/.ssh/id_ed25519";

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
    let account = account_mut(state, &request.account_id)?;

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
            entry.uri = HOSTILE_CONTACT_PHOTO_URI.to_owned();
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
    let max_contact_size = state.max_contact_size;
    let account = account_mut(state, &request.account_id)?;

    // Split borrows: validation reads books while `simple_set` updates cards.
    let AccountState {
        address_books,
        contact_cards,
        ..
    } = account;

    let response = simple_set(contact_cards, request, |id, card| {
        // RFC 8620 §5.3: `id` is server-set and must not appear in create.
        if card.id.is_some() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("id is set by the server and must not be given in a create"));
        }
        if let Some(limit) = max_contact_size {
            let size = serde_json::to_vec(card).map_or(0, |json| json.len() as u64);
            if size > limit {
                return Err(
                    SetError::new(error::set::TOO_LARGE).with_description(format!(
                        "{size} octets is more than the {limit} a card may take"
                    )),
                );
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
        Ok(())
    })?;
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
    let request: ContactCardParseRequest = parse_arguments(arguments)?;
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
            Ok(card) => {
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
        let book = AddressBook {
            id: Some(id.clone()),
            name: name.to_owned(),
            is_default: Some(is_default),
            is_subscribed: Some(true),
            ..AddressBook::default()
        };
        self.address_books.seed_with_id(id.clone(), book);
        id
    }
}
