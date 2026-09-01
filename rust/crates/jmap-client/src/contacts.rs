// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contact operations (RFC 9610).

use jmap_proto::Id;
use jmap_proto::contacts::{AddressBook, ContactCard, ContactCardQueryFilter};
use jmap_proto::error::SetError;
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
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

    /// Patch an address book (`AddressBook/set` update) — a raw JSON Patch
    /// object, the same shape `Client::mailbox_update` takes, so callers can
    /// write typed fields the client has no dedicated setter for yet, such
    /// as `shareWith` (RFC 9610 §2, Track E Phase C).
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

    fn address_book_set(
        &self,
        request: &SetRequest<AddressBook>,
    ) -> Result<SetResponse<AddressBook>, Error> {
        let arguments = self.single_call(USING, "AddressBook/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Create a contact card; returns the stored card (with server-set id,
    /// uid, …).
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

    /// Fetch contact cards by id.
    pub fn contact_get(
        &self,
        account_id: &Id,
        ids: &[Id],
    ) -> Result<GetResponse<ContactCard>, Error> {
        let request = GetRequest::ids(account_id.clone(), ids.iter().cloned());
        let arguments = self.single_call(USING, "ContactCard/get", &request)?;
        Ok(serde_json::from_value(arguments)?)
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

    /// The current `ContactCard` state string (cheap `/get` with no ids).
    pub fn contact_state(&self, account_id: &Id) -> Result<jmap_proto::State, Error> {
        Ok(self.contact_get(account_id, &[])?.state)
    }

    fn contact_set(
        &self,
        request: &SetRequest<ContactCard>,
    ) -> Result<SetResponse<ContactCard>, Error> {
        let arguments = self.single_call(USING, "ContactCard/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }
}

pub(crate) fn set_failure(set_error: Option<&SetError>) -> Error {
    match set_error {
        Some(set_error) => Error::Set(set_error.clone()),
        None => Error::Protocol("/set response reports neither success nor failure".to_owned()),
    }
}
