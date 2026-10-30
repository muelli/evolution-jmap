// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Sieve operations (RFC 9661).
//!
//! Provides `SieveScript/get`, `/set`, `/query`, `/validate`, script upload and
//! download helpers, capability lookup, and `SetError` classification.

use serde_json::Value;

use jmap_proto::Id;
use jmap_proto::error::{SetError, set};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};
pub use jmap_proto::sieve::{
    SieveCapability, SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest,
    SieveScriptValidateRequest, SieveScriptValidateResponse, sieve_set_error,
};

use crate::client::Client;
use crate::error::Error;
use crate::set::{expect_created, expect_destroyed, expect_updated};

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_SIEVE];

/// How [`Client::sieve_script_put`] changes script activation once `/set` succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// Keep the account's current script activation state unchanged.
    Keep,
    /// Activate the created or updated script via `onSuccessActivateScript`.
    Activate,
    /// Deactivate the account's active script via `onSuccessDeactivateScript`.
    Deactivate,
}

/// Known `SetError` kinds returned by `SieveScript/set` and `SieveScript/validate`
/// (RFC 8620 Section 5.3 and RFC 9661 Sections 2.4 and 2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SieveSetErrorKind {
    /// The script failed syntax or semantic validation (`invalidSieve` or `invalidScript`).
    InvalidSieve,
    /// The script is active and cannot be destroyed (`sieveIsActive` or `scriptIsActive`).
    SieveIsActive,
    /// The script exceeds the server's `maxSizeScript` limit (`tooLarge`).
    TooLarge,
    /// The account has reached `maxNumberScripts` (`overQuota`).
    OverQuota,
    /// Another script with the same name already exists (`alreadyExists`).
    AlreadyExists,
}

/// Map a [`SetError`] to its typed [`SieveSetErrorKind`], if recognized.
///
/// Matches both standard RFC 9661 error types and early-draft variants returned
/// by Stalwart and Cyrus (`invalidScript` and `scriptIsActive`).
pub fn set_error_kind(error: &SetError) -> Option<SieveSetErrorKind> {
    match error.error_type.as_str() {
        sieve_set_error::INVALID_SIEVE | "invalidScript" => Some(SieveSetErrorKind::InvalidSieve),
        sieve_set_error::SIEVE_IS_ACTIVE | "scriptIsActive" => {
            Some(SieveSetErrorKind::SieveIsActive)
        }
        set::TOO_LARGE => Some(SieveSetErrorKind::TooLarge),
        set::OVER_QUOTA | "maxQuotaReached" => Some(SieveSetErrorKind::OverQuota),
        set::ALREADY_EXISTS => Some(SieveSetErrorKind::AlreadyExists),
        _ => None,
    }
}

impl Client {
    /// Return the [`SieveCapability`] for `account`, if both the session and the
    /// account advertise `urn:ietf:params:jmap:sieve` (RFC 9661 Section 1.2).
    ///
    /// Merges top-level session capability properties (such as `implementation`)
    /// with per-account capability limits and `sieveExtensions`.
    pub fn sieve_capability(&self, account: &Id) -> Option<SieveCapability> {
        let session_val = self.session().capabilities.get(CAPABILITY_SIEVE)?;
        let acct = self.session().accounts.get(account)?;
        let acct_val = acct.account_capabilities.get(CAPABILITY_SIEVE)?;
        let mut merged = session_val.as_object()?.clone();
        for (key, value) in acct_val.as_object()? {
            if !value.is_null() || !merged.contains_key(key) {
                merged.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(Value::Object(merged)).ok()
    }

    /// Download the UTF-8 source text of `script` (RFC 9661 Section 2.1).
    ///
    /// Returns [`Error::Protocol`] if `script.blob_id` is empty or the blob
    /// bytes are not valid UTF-8.
    pub fn sieve_script_text(&self, account: &Id, script: &SieveScript) -> Result<String, Error> {
        if script.blob_id.is_empty() {
            return Err(Error::Protocol(
                "SieveScript has an empty blobId".to_owned(),
            ));
        }
        let max_bytes = self
            .sieve_capability(account)
            .map(|cap| cap.max_size_script)
            .filter(|&limit| limit > 0)
            .map_or(crate::limits::MAX_BLOB_BYTES, |limit| {
                limit.min(crate::limits::MAX_BLOB_BYTES)
            });
        let download_name = if script.name.is_empty() {
            "script.sieve"
        } else {
            script.name.as_str()
        };
        let bytes = self.download_blob(account, &script.blob_id, download_name, max_bytes)?;
        String::from_utf8(bytes)
            .map_err(|err| Error::Protocol(format!("Sieve script blob is not valid UTF-8: {err}")))
    }

    /// Upload `text` and create or update a Sieve script in one `SieveScript/set` call
    /// (RFC 9661 Section 2.4).
    ///
    /// Creates a new script when `id` is `None`, or updates the existing script when
    /// `id` is `Some`. Applies `activate` in the same `/set` request.
    pub fn sieve_script_put(
        &self,
        account: &Id,
        id: Option<&Id>,
        name: &str,
        text: &str,
        activate: Activation,
    ) -> Result<SieveScript, Error> {
        let uploaded = self.upload_blob_slice(account, "application/sieve", text.as_bytes())?;
        let blob_id = uploaded.blob_id;
        let is_cyrus = self.is_cyrus_sieve(account);

        match id {
            None => {
                let script = SieveScript::new(name, blob_id.clone());
                let set = SetRequest::<SieveScript>::new(account.clone()).create("new", script);
                let mut request = SieveScriptSetRequest::new(set);
                request = match activate {
                    Activation::Keep => request,
                    Activation::Activate => request.activating("#new"),
                    Activation::Deactivate => request.deactivating(),
                };
                let response = self.sieve_script_set_inner(&request, is_cyrus)?;
                let mut created = expect_created(&response, "new")?;
                if created.name.is_empty() {
                    created.name = name.to_owned();
                }
                if created.blob_id.is_empty() || is_cyrus {
                    created.blob_id = blob_id;
                }
                match activate {
                    Activation::Activate => created.is_active = true,
                    Activation::Deactivate => created.is_active = false,
                    Activation::Keep => {}
                }
                Ok(created)
            }
            Some(existing_id) => {
                let patch = serde_json::json!({
                    "name": name,
                    "blobId": blob_id,
                });
                let set = SetRequest::<SieveScript>::new(account.clone())
                    .update(existing_id.clone(), patch);
                let mut request = SieveScriptSetRequest::new(set);
                request = match activate {
                    Activation::Keep => request,
                    Activation::Activate => request.activating(existing_id.clone()),
                    Activation::Deactivate => request.deactivating(),
                };
                let response = self.sieve_script_set_inner(&request, is_cyrus)?;
                expect_updated(&response, existing_id)?;
                let mut updated = response
                    .updated
                    .as_ref()
                    .and_then(|map| map.get(existing_id))
                    .and_then(Option::clone)
                    .unwrap_or_default();
                updated.id = Some(existing_id.clone());
                if updated.name.is_empty() {
                    updated.name = name.to_owned();
                }
                if updated.blob_id.is_empty() || is_cyrus {
                    updated.blob_id = blob_id;
                }
                match activate {
                    Activation::Activate => updated.is_active = true,
                    Activation::Deactivate => updated.is_active = false,
                    Activation::Keep => {}
                }
                Ok(updated)
            }
        }
    }

    fn is_cyrus_sieve(&self, account: &Id) -> bool {
        self.sieve_capability(account)
            .and_then(|cap| cap.implementation)
            .is_some_and(|imp| imp.contains("Cyrus"))
    }
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
        let mut merged = expect_created(&response, "new")?;
        // Fall back to caller-supplied properties when the server omits
        // unchanged fields in `created` (RFC 8620 Section 5.3).
        if merged.name.is_empty() {
            merged.name = script.name.clone();
        }
        if merged.blob_id.is_empty() {
            merged.blob_id = script.blob_id.clone();
        }
        Ok(merged)
    }

    /// Patch a Sieve script (raw JSON Patch), e.g. renaming it. `isActive`
    /// is server-set and rejected here; activate it via
    /// [`Client::sieve_script_activate`] instead.
    pub fn sieve_script_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SieveScriptSetRequest::new(
            SetRequest::<SieveScript>::new(account_id.clone()).update(id.clone(), patch),
        );
        let response = self.sieve_script_set(&request)?;
        expect_updated(&response, id)
    }

    /// Destroy a Sieve script. Fails with `sieveIsActive` if it is the
    /// currently active one (RFC 9661 §2.4): deactivate it first.
    pub fn sieve_script_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SieveScriptSetRequest::new(
            SetRequest::<SieveScript>::new(account_id.clone()).destroy(id.clone()),
        );
        let response = self.sieve_script_set(&request)?;
        expect_destroyed(&response, id)
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

    /// Validate an uploaded Sieve script by blob ID (RFC 9661 Section 2.6).
    ///
    /// Content must be uploaded as a blob before validation (RFC 9661 Section 2.2).
    /// Use [`Client::sieve_script_validate_content`] to validate raw script bytes directly.
    pub fn sieve_script_validate(
        &self,
        account_id: &Id,
        blob_id: impl Into<Id>,
    ) -> Result<SieveScriptValidateResponse, Error> {
        let request = SieveScriptValidateRequest::new(account_id.clone(), blob_id);
        let arguments = self.single_call(USING, "SieveScript/validate", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Validate raw Sieve script bytes (RFC 9661 Section 2.6).
    ///
    /// Uploads content as a blob and then requests validation for the resulting `blob_id`.
    pub fn sieve_script_validate_content(
        &self,
        account_id: &Id,
        content: impl Into<Vec<u8>>,
    ) -> Result<SieveScriptValidateResponse, Error> {
        let uploaded = self.upload_blob(account_id, "application/sieve", content.into())?;
        self.sieve_script_validate(account_id, uploaded.blob_id)
    }

    /// Modify Sieve scripts (`SieveScript/set`, RFC 9661 Section 2.4).
    pub fn sieve_script_set(
        &self,
        request: &SieveScriptSetRequest,
    ) -> Result<SetResponse<SieveScript>, Error> {
        self.sieve_script_set_inner(request, false)
    }

    fn sieve_script_set_inner(
        &self,
        request: &SieveScriptSetRequest,
        cyrus_deactivate: bool,
    ) -> Result<SetResponse<SieveScript>, Error> {
        let limit = self
            .session()
            .max_objects_in_set()
            .and_then(|l| usize::try_from(l).ok());

        crate::set::execute_set_chunked(&request.set, limit, |chunk_set, created_ids| {
            let mut chunk_req = SieveScriptSetRequest::new(chunk_set.clone());
            chunk_req.on_success_activate_script = request.on_success_activate_script.clone();
            chunk_req.on_success_deactivate_script = request.on_success_deactivate_script;
            let mut wire_req = serde_json::to_value(&chunk_req)?;
            // RFC 9661 Section 2.1 defines `isActive` as server-set (`false` on create).
            // Strip `"isActive": false` from `create` entries so strict servers (e.g. Cyrus)
            // do not reject the request with `invalidProperties: ["isActive"]`.
            if let Some(creates) = chunk_set.create.as_ref()
                && let Some(wire_creates) =
                    wire_req.get_mut("create").and_then(Value::as_object_mut)
            {
                for (cid, script) in creates {
                    if !script.is_active
                        && let Some(obj) = wire_creates.get_mut(cid).and_then(Value::as_object_mut)
                    {
                        obj.remove("isActive");
                    }
                }
            }
            // RFC 9661 Section 2.4 forbids sending both `onSuccessActivateScript` and
            // `onSuccessDeactivateScript` in the same request. When deactivating via
            // `SieveScriptSetRequest::deactivating()`, omit the redundant null key.
            if chunk_req.on_success_deactivate_script == Some(true)
                && chunk_req.on_success_activate_script == Some(Value::Null)
                && let Some(obj) = wire_req.as_object_mut()
            {
                obj.remove("onSuccessActivateScript");
            }
            if cyrus_deactivate
                && chunk_req.on_success_deactivate_script == Some(true)
                && let Some(obj) = wire_req.as_object_mut()
            {
                obj.remove("onSuccessDeactivateScript");
                obj.insert("onSuccessDeactivatescript".to_owned(), Value::Bool(true));
            }
            let arguments = self.single_call_with_created_ids(
                USING,
                "SieveScript/set",
                &wire_req,
                created_ids,
            )?;
            Ok(serde_json::from_value(arguments)?)
        })
    }
}
