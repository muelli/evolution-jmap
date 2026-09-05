// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contact methods (`AddressBook/get`, `ContactCard/get|set|query`,
//! RFC 9610) and contact seeding helpers.

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::contacts::{
    AddressBook, AddressBookRights, ContactCard, ContactCardParseRequest, ContactCardQueryFilter,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::setops::simple_set;
use crate::state::{AccountState, ServerState};

/// `caller` is the identity `AddressBook/get`'s request carried, as resolved
/// by [`crate::auth::AuthConfig::identity_for`] — `None` (no identity bound
/// to the credential) reads as "this account's own owner", matching every
/// test that predates sharing. A caller who *is* a distinct principal only
/// sees books that principal's own `shareWith` entry grants, and gets
/// `forbidden` outright if the account shares nothing with them at all
/// (verified against a live Stalwart server: Track E Phase C step 1's
/// findings, recorded in the work queue).
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

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.address_books.state(),
        list,
        not_found,
    })
}

/// The rights `book.share_with` grants `principal`, or `None` if it grants
/// them nothing (including "not shared at all").
fn book_rights_for(
    book: &AddressBook,
    principal: &Id,
) -> Option<jmap_proto::contacts::AddressBookRights> {
    book.share_with.as_ref()?.get(principal).cloned()
}

/// The owner sees every book unchanged, exactly as before sharing existed. A
/// foreign caller sees a book only if it is shared with them, with
/// `myRights` replaced by the grant itself rather than whatever the owner's
/// own `myRights` happened to be.
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

/// `AddressBook/set` (RFC 9610 §2): making and removing an address book.
///
/// No hierarchy and no cross-object placement rules the way `Mailbox/set`
/// has, so [`simple_set`] is the whole of it — the only per-create check is
/// the one every `/set` create shares (server-set `id` rejected) plus the one
/// RFC 9610 states for `name`.
pub fn address_book_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<AddressBook> = parse_arguments(arguments)?;
    let default_unsubscribed = state.new_collections_default_unsubscribed;
    let terse_collection_create = state.terse_collection_create;
    let account_id = request.account_id.clone();
    let account = account_mut(state, &account_id)?;

    // Captured before `simple_set` consumes `request.update`, so a
    // `shareWith` change can be diffed against what it was, for
    // `ShareNotification` delivery (Track E Phase C step 2, RFC 9670 §4).
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

    // RFC 8620 §5.3: the `created` map need only carry properties the client
    // did not already send. `name` was named by the client itself in a
    // create, so it is not server-set — a server reading that literally
    // (Fastmail among them) leaves it out. See
    // `MockServerBuilder::terse_collection_create`'s doc for the finding
    // this reproduces. Unlike `contact_card_set`'s identical-shaped stanza,
    // only `name` is stripped: `isDefault`/`myRights` are genuinely
    // server-computed here and every test relying on them must still see
    // them.
    if terse_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("name");
            }
        }
    }

    Ok(result)
}

pub fn contact_card_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
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

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.contact_cards.state(),
        list,
        not_found,
    })
}

pub fn contact_card_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<ContactCard> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    // Split the borrow: validation reads address_books while simple_set
    // mutates contact_cards.
    let AccountState {
        address_books,
        contact_cards,
        ..
    } = account;

    let response = simple_set(contact_cards, request, |id, card| {
        // `id` is server-set (RFC 8620 §5.3): a client that names one in a
        // create is confusing some other identifier for a JMAP id, which is
        // exactly the mistake a backend makes with a vCard `UID` invented by
        // the local cache. Silently overwriting it would hide that.
        if card.id.is_some() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("id is set by the server and must not be given in a create"));
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

    // RFC 8620 §5.3: the `created` map need only carry properties the
    // client did not already send. Every property but `id` was named by the
    // client itself in a create, so none is server-set — a server reading
    // that literally (Stalwart among them) leaves everything else out. See
    // `MockServerBuilder::terse_contact_create`'s doc for the finding this
    // reproduces.
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
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .contact_cards
        .iter()
        .filter(|(_, card)| card_matches(card, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .contact_cards
        .iter()
        .filter(|(_, card)| card_matches(card, &filter))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.contact_cards.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

/// `ContactCard/parse` (RFC 9610 §3.4): reads an uploaded vCard blob into a
/// `ContactCard`, without filing it into any address book. Building the
/// response through `project_properties` lets `properties` drop fields
/// before the typed `ContactCard` ever serializes, the same way
/// `calendar_event_parse` and `email_parse` already do.
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
    /// Seed an address book; returns its id. Does not bump state.
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
