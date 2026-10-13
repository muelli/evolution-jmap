// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Sieve (RFC 9661): `SieveScript/get`, `/set`, `/query` and
//! `/validate`. Evolution's filters UI is not wired to any of this yet.

use serde_json::Value;

use jmap_proto::Id;
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};
use jmap_proto::sieve::{
    SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest, SieveScriptValidateRequest,
    SieveScriptValidateResponse,
};

use crate::client::Client;
use crate::contacts::set_failure;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_SIEVE];

impl Client {
    /// Fetch all Sieve scripts (`SieveScript/get` with `ids: null`).
    pub fn sieve_scripts(&self, account_id: &Id) -> Result<Vec<SieveScript>, Error> {
        let arguments = self.single_call(
            USING,
            "SieveScript/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<SieveScript> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Create a Sieve script; returns the stored script (with server-set id).
    pub fn sieve_script_create(
        &self,
        account_id: &Id,
        script: &SieveScript,
    ) -> Result<SieveScript, Error> {
        let request = SieveScriptSetRequest::new(
            SetRequest::<SieveScript>::new(account_id.clone()).create("new", script.clone()),
        );
        let response = self.sieve_script_set(&request)?;
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

    /// Patch a Sieve script (raw JSON Patch), e.g. renaming it. `isActive`
    /// is server-set and rejected here; activate it via
    /// [`Client::sieve_script_activate`] instead.
    pub fn sieve_script_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SieveScriptSetRequest::new(
            SetRequest::<SieveScript>::new(account_id.clone()).update(id.clone(), patch),
        );
        let response = self.sieve_script_set(&request)?;
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

    /// Destroy a Sieve script. Fails with `sieveIsActive` if it is the
    /// currently active one (RFC 9661 §2.4): deactivate it first.
    pub fn sieve_script_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SieveScriptSetRequest::new(
            SetRequest::<SieveScript>::new(account_id.clone()).destroy(id.clone()),
        );
        let response = self.sieve_script_set(&request)?;
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

    /// Activate a Sieve script, deactivating whichever one was active
    /// before (RFC 9661 §2.4 `onSuccessActivateScript`).
    pub fn sieve_script_activate(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request =
            SieveScriptSetRequest::new(SetRequest::new(account_id.clone())).activating(id.clone());
        self.sieve_script_set(&request)?;
        Ok(())
    }

    /// Deactivate the currently active Sieve script, if any.
    pub fn sieve_script_deactivate(&self, account_id: &Id) -> Result<(), Error> {
        let request =
            SieveScriptSetRequest::new(SetRequest::new(account_id.clone())).deactivating();
        self.sieve_script_set(&request)?;
        Ok(())
    }

    /// Resolve Sieve script ids matching `filter` (`SieveScript/query`, RFC
    /// 9661 §2.5), e.g. by name, or to find the one currently active script.
    pub fn sieve_script_query(
        &self,
        account_id: &Id,
        filter: SieveScriptQueryFilter,
    ) -> Result<Vec<Id>, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(USING, "SieveScript/query", &request)?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }

    /// Validate an already-uploaded Sieve script by its `blobId` (RFC 9661
    /// section 2.6). The RFC takes only `blobId`; content must be uploaded
    /// first (section 2.2, [`Client::upload_blob`]), there is no raw-content
    /// shorthand on the wire. Use [`Client::sieve_script_validate_content`]
    /// to validate raw content directly.
    pub fn sieve_script_validate(
        &self,
        account_id: &Id,
        blob_id: impl Into<Id>,
    ) -> Result<SieveScriptValidateResponse, Error> {
        let request = SieveScriptValidateRequest::new(account_id.clone(), blob_id);
        let arguments = self.single_call(USING, "SieveScript/validate", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Validate raw Sieve script content: uploads it as a blob first (RFC
    /// 9661 §2.6 requires content to already be uploaded before it can be
    /// validated), then validates the resulting `blobId`.
    pub fn sieve_script_validate_content(
        &self,
        account_id: &Id,
        content: impl Into<Vec<u8>>,
    ) -> Result<SieveScriptValidateResponse, Error> {
        let uploaded = self.upload_blob(account_id, "application/sieve", content.into())?;
        self.sieve_script_validate(account_id, uploaded.blob_id)
    }

    fn sieve_script_set(
        &self,
        request: &SieveScriptSetRequest,
    ) -> Result<SetResponse<SieveScript>, Error> {
        let arguments = self.single_call(USING, "SieveScript/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }
}
