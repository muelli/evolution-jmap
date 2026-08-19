// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contact methods (`AddressBook/get`, `ContactCard/get|set|query`,
//! RFC 9610) and contact seeding helpers.

use jmap_proto::Id;
use jmap_proto::contacts::{AddressBook, ContactCard, ContactCardQueryFilter};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::setops::simple_set;
use crate::state::{AccountState, ServerState};

pub fn address_book_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(account.address_books.iter().map(|(_, book)| book.clone())),
        Some(ids) => {
            for id in ids {
                match account.address_books.get(id) {
                    Some(book) => list.push(book.clone()),
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

/// `AddressBook/set` (RFC 9610 §2): making and removing an address book.
///
/// No hierarchy and no cross-object placement rules the way `Mailbox/set`
/// has, so [`simple_set`] is the whole of it — the only per-create check is
/// the one every `/set` create shares (server-set `id` rejected) plus the one
/// RFC 9610 states for `name`.
pub fn address_book_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<AddressBook> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

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
        Ok(())
    })?;
    to_result(&response)
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
    to_result(&response)
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
