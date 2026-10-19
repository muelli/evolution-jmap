// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contact operations (RFC 9610).

use jmap_proto::Id;
use jmap_proto::contacts::{
    AddressBook, ContactCard, ContactCardParseRequest, ContactCardParseResponse,
    ContactCardQueryFilter,
};
use jmap_proto::error::SetError;
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::request::Request;
use jmap_proto::session::{CAPABILITY_CONTACTS, CAPABILITY_CORE};
use serde_json::Value;

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_CONTACTS];

impl Client {
    /// Fetch all address books (`AddressBook/get` with `ids: null`).
    pub fn address_books(&self, account_id: &Id) -> Result<Vec<AddressBook>, Error> {
        let arguments = self.single_call(
            USING,
            "AddressBook/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<AddressBook> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Create an address book; returns the stored book (with server-set id).
    pub fn address_book_create(
        &self,
        account_id: &Id,
        book: &AddressBook,
    ) -> Result<AddressBook, Error> {
        let request =
            SetRequest::<AddressBook>::new(account_id.clone()).create("new", book.clone());
        let response = self.address_book_set(&request)?;
        if let Some(created) = response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
        {
            return Ok(created.clone());
        }
        Err(set_failure(
            response.not_created.as_ref().and_then(|map| map.get("new")),
        ))
    }

    /// Update an address book (`AddressBook/set`) with a JSON Patch object.
    ///
    /// Callers can update properties such as `shareWith` (RFC 9610 Section 2).
    pub fn address_book_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<AddressBook>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.address_book_set(&request)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Destroy an address book.
    pub fn address_book_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<AddressBook>::new(account_id.clone()).destroy(id.clone());
        let response = self.address_book_set(&request)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Modify address books (`AddressBook/set`, RFC 9610 Section 2.3).
    pub fn address_book_set(
        &self,
        request: &SetRequest<AddressBook>,
    ) -> Result<SetResponse<AddressBook>, Error> {
        self.set_chunked(USING, "AddressBook/set", request)
    }

    /// Create a contact card.
    ///
    /// Returns the stored card with server-assigned properties such as `id` and `uid`.
    pub fn contact_create(
        &self,
        account_id: &Id,
        card: &ContactCard,
    ) -> Result<ContactCard, Error> {
        let request =
            SetRequest::<ContactCard>::new(account_id.clone()).create("new", card.clone());
        let response = self.contact_set(&request)?;
        if let Some(created) = response
            .created
            .as_ref()
            .and_then(|created| created.get("new"))
        {
            return Ok(created.clone());
        }
        Err(set_failure(
            response.not_created.as_ref().and_then(|map| map.get("new")),
        ))
    }

    /// Fetch contact cards by identifier.
    ///
    /// Splits requests into chunks when the ID count exceeds `maxObjectsInGet` (RFC 8620 Section 2).
    /// Sends the full list at once if the server reports no limit.
    ///
    /// Always executes at least one call. Methods like [`Client::contact_state`] pass empty IDs to fetch the current state.
    pub fn contact_get(
        &self,
        account_id: &Id,
        ids: &[Id],
    ) -> Result<GetResponse<ContactCard>, Error> {
        let max_objects = self
            .session()
            .max_objects_in_get()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|&limit| limit > 0)
            .unwrap_or(ids.len().max(1));

        let mut combined: Option<GetResponse<ContactCard>> = None;
        let mut rest = ids;
        loop {
            let take = rest.len().min(max_objects);
            let (chunk, remaining) = rest.split_at(take);

            let call_id = self.next_call_id();
            let request = Request::new([CAPABILITY_CORE, CAPABILITY_CONTACTS]).call(
                "ContactCard/get",
                &GetRequest::ids(account_id.clone(), chunk.iter().cloned()),
                &call_id,
            )?;
            let response = self.api_call(&request)?;
            let invocation = response
                .responses_for(&call_id)
                .next()
                .ok_or_else(|| Error::Protocol("no ContactCard/get response".to_owned()))?;
            let arguments = Self::unwrap_invocation(invocation, "ContactCard/get")?;
            let chunk_response: GetResponse<ContactCard> = serde_json::from_value(arguments)?;
            match &mut combined {
                None => combined = Some(chunk_response),
                Some(combined) => {
                    combined.list.extend(chunk_response.list);
                    combined.not_found.extend(chunk_response.not_found);
                    combined.state = chunk_response.state;
                }
            }

            rest = remaining;
            if rest.is_empty() {
                return Ok(combined.expect("the loop always sets combined on its first iteration"));
            }
        }
    }

    /// Apply a patch to a contact card (RFC 8620 PatchObject).
    pub fn contact_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<ContactCard>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.contact_set(&request)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Destroy a contact card.
    pub fn contact_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<ContactCard>::new(account_id.clone()).destroy(id.clone());
        let response = self.contact_set(&request)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// `ContactCard/query`: matching card ids.
    pub fn contact_query(
        &self,
        account_id: &Id,
        filter: ContactCardQueryFilter,
    ) -> Result<QueryResponse, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "ContactCard/query", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Parse an uploaded vCard blob into a `ContactCard` (RFC 9610 Section 3.4).
    ///
    /// The card is not added to an address book.
    /// Unwraps single-element arrays returned by non-conforming servers.
    /// Blobs that parse into multiple cards are marked as `notParsable`.
    pub fn contact_card_parse(
        &self,
        request: &ContactCardParseRequest,
    ) -> Result<ContactCardParseResponse, Error> {
        let mut arguments = self.single_call(USING, "ContactCard/parse", request)?;
        demote_multi_element_parsed_blobs(&mut arguments);
        Ok(serde_json::from_value(arguments)?)
    }

    /// Fetch the current `ContactCard` state string.
    pub fn contact_state(&self, account_id: &Id) -> Result<jmap_proto::State, Error> {
        Ok(self.contact_get(account_id, &[])?.state)
    }

    /// Modify contact cards (`ContactCard/set`, RFC 9610 Section 3).
    pub fn contact_set(
        &self,
        request: &SetRequest<ContactCard>,
    ) -> Result<SetResponse<ContactCard>, Error> {
        self.set_chunked(USING, "ContactCard/set", request)
    }
}

pub(crate) fn set_failure(set_error: Option<&SetError>) -> Error {
    match set_error {
        Some(set_error) => Error::Set(set_error.clone()),
        None => Error::Protocol("/set response reports neither success nor failure".to_owned()),
    }
}

/// Normalize array-wrapped parse results from servers such as Stalwart.
///
/// Some servers wrap parsed items in a JSON array instead of an object.
/// If an array contains exactly one item, this function unwraps it.
/// If an array contains zero or multiple items, this function moves the ID to `notParsable`.
pub(crate) fn demote_multi_element_parsed_blobs(arguments: &mut Value) {
    let Some(obj) = arguments.as_object_mut() else {
        return;
    };
    let Some(parsed) = obj.get_mut("parsed").and_then(Value::as_object_mut) else {
        return;
    };
    let mut unrepresentable_ids = Vec::new();
    for (id, val) in parsed.iter_mut() {
        if let Value::Array(arr) = val {
            if arr.len() == 1 {
                *val = arr.pop().expect("len checked above");
            } else {
                unrepresentable_ids.push(id.clone());
            }
        }
    }
    for id in &unrepresentable_ids {
        parsed.remove(id);
    }
    if !unrepresentable_ids.is_empty() {
        let not_parsable = obj
            .entry("notParsable")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(not_parsable) = not_parsable {
            not_parsable.extend(unrepresentable_ids.into_iter().map(Value::String));
        }
    }
}
