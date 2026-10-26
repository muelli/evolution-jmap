// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail operations (RFC 8621).

use std::collections::BTreeMap;

use jmap_proto::error::SetError;
use jmap_proto::mail::{
    Email, EmailImport, EmailImportRequest, EmailImportResponse, EmailParseRequest,
    EmailParseResponse, EmailQueryFilter, EmailSubmission, EmailSubmissionQueryFilter,
    EmailSubmissionSetRequest, Envelope, Identity, Mailbox, Schedule, SearchSnippet,
    SearchSnippetGetRequest, SearchSnippetGetResponse, Thread, VacationResponse,
};
use jmap_proto::methods::{
    Comparator, Filter, GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest,
    SetResponse, UploadResponse,
};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{
    CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION, CAPABILITY_VACATION_RESPONSE,
};
use jmap_proto::{Id, State};
use serde_json::Value;

use crate::client::Client;
use crate::error::Error;
use crate::set::{expect_created, expect_destroyed, expect_updated};
use crate::transport::HttpMethod;
use crate::url::encode_template_value;

/// Extract created item from maps returned by methods like `Email/import` (RFC 8621 Section 4.8).
fn creation_outcome<T: Clone>(
    created: Option<&BTreeMap<String, T>>,
    not_created: Option<&BTreeMap<String, SetError>>,
    creation_id: &str,
) -> Result<T, Error> {
    if let Some(object) = created.and_then(|created| created.get(creation_id)) {
        return Ok(object.clone());
    }
    if let Some(set_error) = not_created.and_then(|not_created| not_created.get(creation_id)) {
        return Err(Error::Set(set_error.clone()));
    }
    Err(Error::Protocol(format!(
        "server response contains neither created nor notCreated for {creation_id}"
    )))
}

/// Put `emails` back into the order `ids` names them in, dropping any the
/// server did not answer with.
///
/// `/get` does not preserve the order of requested IDs (RFC 8620 §5.1), so the
/// query sort order survives only if reapplied here. Messages destroyed between
/// query and fetch are omitted.
fn in_query_order(ids: &[Id], emails: Vec<Email>) -> Vec<Email> {
    let mut by_id: BTreeMap<Id, Email> = emails
        .into_iter()
        .filter_map(|email| email.id.clone().map(|id| (id, email)))
        .collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

/// Fill in `identityId`/`emailId` on a just-created `EmailSubmission` if the
/// server omitted them.
///
/// RFC 8620 §5.3 allows omitting properties unchanged from the request.
/// This patches the raw JSON with client-supplied values before deserialization.
fn backfill_submission_created(
    mut arguments: Value,
    creation_id: &str,
    identity_id: &Id,
    email_id: &Id,
) -> Value {
    if let Some(object) = arguments
        .get_mut("created")
        .and_then(|created| created.get_mut(creation_id))
        .and_then(Value::as_object_mut)
    {
        object
            .entry("identityId")
            .or_insert_with(|| Value::from(identity_id.as_str()));
        object
            .entry("emailId")
            .or_insert_with(|| Value::from(email_id.as_str()));
    }
    arguments
}

/// Build an `EmailSubmission/set` request.
///
/// `email_id` is either a `#draft` reference or an existing message ID.
/// `on_success_update` patches the message after the server accepts the submission.
///
/// `envelope` is the optional SMTP envelope. When omitted, the server derives recipients from headers (RFC 8621 Section 7).
fn submission_request(
    account_id: &Id,
    identity_id: &Id,
    email_id: Id,
    envelope: Option<Envelope>,
    on_success_update: Option<Value>,
) -> EmailSubmissionSetRequest {
    const SUBMISSION: &str = "submission";
    // Server-set (RFC 8621 §7.1): a release time travels as a FUTURERELEASE
    // envelope parameter, never as the submission's own property.
    let mut submission = EmailSubmission::new(identity_id.clone(), email_id);
    if let Some(envelope) = envelope {
        submission = submission.with_envelope(envelope);
    }

    let mut request = EmailSubmissionSetRequest::new(
        SetRequest::<EmailSubmission>::new(account_id.clone()).create(SUBMISSION, submission),
    );
    if let Some(patch) = on_success_update {
        request = request.with_on_success_update_email([(format!("#{SUBMISSION}"), patch)].into());
    }
    request
}

impl Client {
    /// Fetch all mailboxes for an account (`Mailbox/get` with `ids: null`).
    ///
    /// Returns the full response containing the mailbox list and state string.
    /// The state string is required for incremental sync via `Mailbox/changes`.
    pub fn mailbox_get(&self, account_id: &Id) -> Result<GetResponse<Mailbox>, Error> {
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/get",
            &GetRequest::all(account_id.clone()),
        )?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Create a mailbox (`Mailbox/set`).
    ///
    /// Returns the created mailbox, including its server-assigned ID.
    /// Returns [`Error::Set`] if rejected (RFC 8621 Section 2).
    pub fn mailbox_create(&self, account_id: &Id, mailbox: &Mailbox) -> Result<Mailbox, Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).create("new", mailbox.clone());
        let response = self.mailbox_set(&request)?;
        let mut created = expect_created(&response, "new")?;
        // If the server omitted properties (RFC 8620 Section 5.3 allows omitting unchanged
        // properties), fall back to the requested properties.
        if created.name.is_empty() {
            created.name = mailbox.name.trim().to_string();
        } else {
            created.name = created.name.trim().to_string();
        }
        if created.parent_id.is_none() {
            created.parent_id = mailbox.parent_id.clone();
        }
        if created.role.is_none() {
            created.role = mailbox.role.clone();
        }
        if created.sort_order.is_none() {
            created.sort_order = mailbox.sort_order;
        }
        if created.is_subscribed.is_none() {
            created.is_subscribed = mailbox.is_subscribed;
        }
        Ok(created)
    }

    /// Update mailbox properties (`Mailbox/set`).
    ///
    /// Takes a JSON Patch object (RFC 8620 Section 5.3) containing modified properties.
    pub fn mailbox_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.mailbox_set(&request)?;
        expect_updated(&response, id)
    }

    /// Destroy a mailbox (`Mailbox/set`).
    ///
    /// Returns [`Error::Set`] if the server refuses removal (RFC 8621 Section 2.5).
    /// Common rejection codes include `mailboxHasChild` and `mailboxHasEmail`.
    pub fn mailbox_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).destroy(id.clone());
        let response = self.mailbox_set(&request)?;
        expect_destroyed(&response, id)
    }

    /// Modify mailboxes (`Mailbox/set`, RFC 8621 Section 2.5).
    pub fn mailbox_set(
        &self,
        request: &SetRequest<Mailbox>,
    ) -> Result<SetResponse<Mailbox>, Error> {
        self.set_chunked(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Mailbox/set", request)
    }

    /// Fetch the account vacation responder settings (RFC 8621 Section 8.1).
    ///
    /// Returns the singleton vacation response object.
    pub fn vacation_response_get(&self, account_id: &Id) -> Result<VacationResponse, Error> {
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_VACATION_RESPONSE],
            "VacationResponse/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<VacationResponse> = serde_json::from_value(arguments)?;
        response.list.into_iter().next().ok_or_else(|| {
            Error::Protocol("server answered VacationResponse/get with no singleton".to_owned())
        })
    }

    /// Change the vacation responder (`VacationResponse/set` update: RFC
    /// 8621 §8.1 forbids create and destroy outright, so this is the only
    /// mutation the singleton allows).
    pub fn vacation_response_update(&self, account_id: &Id, patch: Value) -> Result<(), Error> {
        let id: Id = "singleton".into();
        let request =
            SetRequest::<VacationResponse>::new(account_id.clone()).update(id.clone(), patch);
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_VACATION_RESPONSE],
            "VacationResponse/set",
            &request,
        )?;
        let response: SetResponse<VacationResponse> = serde_json::from_value(arguments)?;
        expect_updated(&response, &id)
    }

    /// Query matching email IDs (`Email/query`).
    ///
    /// `position` is the zero-based result offset for pagination (RFC 8620 Section 5.5).
    /// Servers can return fewer items than requested and report the cap in [`QueryResponse::limit`].
    /// `filter` accepts flat conditions or structured AND/OR/NOT trees.
    pub fn email_query(
        &self,
        account_id: &Id,
        filter: impl Into<Filter<EmailQueryFilter>>,
        sort: Option<Vec<Comparator>>,
        limit: Option<u64>,
        position: i64,
    ) -> Result<QueryResponse, Error> {
        let mut request = QueryRequest::new(account_id.clone()).filter(filter.into());
        request.sort = sort;
        request.limit = limit;
        request.position = position;
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/query", &request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Query all email IDs matching `filter`, optionally sorted by `sort`.
    ///
    /// Fetches all pages across `Email/query`, validating the returned count
    /// against `total` (RFC 8620 Section 5.5) and tracking against the client ID budget.
    pub fn email_query_every_id(
        &self,
        account_id: &Id,
        filter: impl Into<Filter<EmailQueryFilter>>,
        sort: Option<Vec<Comparator>>,
    ) -> Result<Vec<Id>, Error> {
        self.query_every_id(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/query",
            account_id,
            filter.into(),
            sort,
        )
    }

    /// Fetch current `Email` state string without downloading messages.
    ///
    /// Issues an empty `Email/get` request (RFC 8620 Section 5.1).
    /// Callers use this state as the baseline for future `Email/changes` requests.
    pub fn email_state(&self, account_id: &Id) -> Result<State, Error> {
        let mut request = GetRequest::ids(account_id.clone(), std::iter::empty::<Id>());
        // Request only the id property to minimize response payload size.
        request.properties = Some(vec!["id".to_owned()]);
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/get", &request)?;
        let response: GetResponse<Email> = serde_json::from_value(arguments)?;
        Ok(response.state)
    }

    /// `Email/get` for explicit ids.
    ///
    /// Sent in chunks when naming every ID exceeds the session's
    /// `maxSizeRequest` (RFC 8620 §2) or `maxObjectsInGet`. Over the limit the
    /// server refuses the request, so requests are split when necessary.
    /// A server that names no limit is sent the list whole.
    pub fn email_get(
        &self,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let response = self.get_chunked(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/get",
            account_id,
            ids,
            properties,
            |email: &Email| email.id.as_ref(),
        )?;
        Ok(response.list)
    }

    /// `Email/query` chained to `Email/get` through a `#ids` back-reference in
    /// one round-trip (RFC 8620 §3.7).
    ///
    /// Two round-trips against a server whose `maxCallsInRequest` is smaller
    /// than the chain: the query first, then a `/get` naming the ids it
    /// answered.
    pub fn email_query_then_get(
        &self,
        account_id: &Id,
        filter: impl Into<Filter<EmailQueryFilter>>,
        sort: Option<Vec<Comparator>>,
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let mut query = QueryRequest::new(account_id.clone()).filter(filter.into());
        query.sort = sort;

        if !self.takes_calls_in_one_request(2) {
            return self.email_query_then_get_separately(account_id, &query, properties);
        }

        let query_call_id = self.next_call_id();
        let get_call_id = self.next_call_id();

        let mut get = GetRequest::all(account_id.clone());
        get.ids_ref = Some(ResultReference::new(
            query_call_id.clone(),
            "Email/query",
            "/ids",
        ));
        get.properties =
            properties.map(|properties| properties.iter().map(|s| s.to_string()).collect());

        let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
            .call("Email/query", &query, &query_call_id)?
            .call("Email/get", &get, &get_call_id)?;
        let response = self.api_call(&request)?;

        let invocation = response
            .responses_for(&get_call_id)
            .next()
            .ok_or_else(|| Error::Protocol("no Email/get response".to_owned()))?;
        let arguments = Self::unwrap_invocation(invocation, "Email/get")?;
        let get_response: GetResponse<Email> = serde_json::from_value(arguments)?;

        // /get does not preserve request order; restore the query's.
        let query_invocation = response
            .responses_for(&query_call_id)
            .next()
            .ok_or_else(|| Error::Protocol("no Email/query response".to_owned()))?;
        let query_response: QueryResponse =
            serde_json::from_value(Self::unwrap_invocation(query_invocation, "Email/query")?)?;

        Ok(in_query_order(&query_response.ids, get_response.list))
    }

    /// [`Client::email_query_then_get`] as two requests, for a server that will
    /// not take the chain.
    fn email_query_then_get_separately(
        &self,
        account_id: &Id,
        query: &QueryRequest<Filter<EmailQueryFilter>>,
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/query", query)?;
        let query_response: QueryResponse = serde_json::from_value(arguments)?;
        // Nothing matched, so there is nothing to fetch, and an `Email/get`
        // naming no ids is a whole round trip spent on an empty list. The
        // chained form has no equivalent case: its `/get` travels with the
        // query whether or not the query finds anything.
        if query_response.ids.is_empty() {
            return Ok(Vec::new());
        }
        let emails = self.email_get(account_id, &query_response.ids, properties)?;
        Ok(in_query_order(&query_response.ids, emails))
    }

    /// Modify emails (`Email/set`, RFC 8621 Section 4.6).
    pub fn email_set(&self, request: &SetRequest<Email>) -> Result<SetResponse<Email>, Error> {
        self.set_chunked(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/set", request)
    }

    /// Apply a patch to one message (`Email/set` with a single `update`).
    ///
    /// A `PatchObject` (RFC 8620 §5.3), not a whole `Email`: mutable fields
    /// (`keywords` and `mailboxIds`) are sets that other clients can modify.
    ///
    /// Rejections return [`Error::Set`] with the server's reason, such as
    /// `notFound` if the message was destroyed before update.
    pub fn email_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<Email>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.email_set(&request)?;
        expect_updated(&response, id)
    }

    /// Remove one message from the store entirely (`Email/set` destroy).
    ///
    /// Destroys the email across all mailboxes (RFC 8621 §4.6). To remove an
    /// email from a single mailbox, use [`Client::email_update`] on `mailboxIds`.
    ///
    /// `notFound` (if another client destroyed the message first) arrives as
    /// [`Error::Set`].
    pub fn email_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<Email>::new(account_id.clone()).destroy(id.clone());
        let response = self.email_set(&request)?;
        expect_destroyed(&response, id)
    }

    /// Put a message into the store from bytes already uploaded
    /// (`Email/import`, RFC 8621 §4.8); answers the `Email` the server made of
    /// it.
    ///
    /// Uploads message bytes via [`Client::upload_blob`] followed by import.
    /// Unlike composing by properties, imported RFC 5322 bytes survive intact.
    ///
    /// Refusal returns [`Error::Set`]: `invalidProperties`, `invalidEmail`,
    /// or `overQuota`.
    pub fn email_import(&self, account_id: &Id, email: &EmailImport) -> Result<Email, Error> {
        const IMPORTED: &str = "imported";

        let request = EmailImportRequest::new(account_id.clone()).import(IMPORTED, email.clone());
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/import",
            &request,
        )?;
        let response: EmailImportResponse = serde_json::from_value(arguments)?;
        creation_outcome(
            response.created.as_ref(),
            response.not_created.as_ref(),
            IMPORTED,
        )
    }

    /// `Email/parse` (RFC 8621 §4.8): reads an uploaded RFC 5322 blob into an
    /// `Email`, without importing it into the mailbox: the read-only
    /// counterpart to [`Client::email_import`].
    pub fn email_parse(&self, request: &EmailParseRequest) -> Result<EmailParseResponse, Error> {
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/parse", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Fetch Threads by id (`Thread/get`, RFC 8621 §3.1).
    ///
    /// Splits requests into chunks when IDs exceed `maxObjectsInGet`.
    /// An ID naming no thread is silently absent from the result.
    pub fn thread_get(
        &self,
        account_id: &Id,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Result<Vec<Thread>, Error> {
        let all_ids: Vec<Id> = ids.into_iter().map(Into::into).collect();
        let response = self.get_chunked(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Thread/get",
            account_id,
            &all_ids,
            None,
            |thread: &Thread| thread.id.as_ref(),
        )?;
        Ok(response.list)
    }

    /// Fetch search snippets for email IDs (`SearchSnippet/get`, RFC 8621 Section 5.1).
    ///
    /// Splits requests into chunks when IDs exceed `maxSizeRequest` or `maxObjectsInGet`.
    /// Snippets contain matching text wrapped in HTML `<mark>` tags.
    /// `filter` specifies the query criteria used to locate matching text.
    /// Omitted or invalid IDs do not produce snippet entries.
    pub fn search_snippet_get(
        &self,
        account_id: &Id,
        email_ids: impl IntoIterator<Item = impl Into<Id>>,
        filter: Option<impl Into<Filter<EmailQueryFilter>>>,
    ) -> Result<Vec<SearchSnippet>, Error> {
        let all_ids: Vec<Id> = email_ids.into_iter().map(Into::into).collect();
        let filter_val: Option<Filter<EmailQueryFilter>> = filter.map(Into::into);
        let response = self.chunked_get_custom(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "SearchSnippet/get",
            &all_ids,
            |chunk| {
                let mut req =
                    SearchSnippetGetRequest::new(account_id.clone(), chunk.iter().cloned());
                if let Some(f) = &filter_val {
                    req = req.with_filter(f.clone());
                }
                req
            },
            |resp: SearchSnippetGetResponse| {
                (
                    resp.list,
                    resp.not_found.unwrap_or_default(),
                    None,
                    resp.account_id,
                )
            },
            |snippet: &SearchSnippet| Some(&snippet.email_id),
        )?;
        Ok(response.list)
    }

    /// Fetch the account's sending identities (`Identity/get`).
    pub fn identities(&self, account_id: &Id) -> Result<Vec<Identity>, Error> {
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "Identity/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<Identity> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// Create a draft and submit it in a single request: `Email/set` chained
    /// to `EmailSubmission/set` through a `#draft` creation reference.
    ///
    /// `on_success_update` is an optional patch applied to the email once the
    /// submission is accepted (RFC 8621 §7.5), typically moving it to Sent.
    /// Returns the server-set email properties and the accepted submission.
    ///
    /// Falls back to two requests when `maxCallsInRequest` is less than 2.
    pub fn send_email(
        &self,
        account_id: &Id,
        email: &Email,
        identity_id: &Id,
        on_success_update: Option<Value>,
    ) -> Result<(Email, EmailSubmission), Error> {
        const DRAFT: &str = "draft";
        const SUBMISSION: &str = "submission";

        let email_set = SetRequest::<Email>::new(account_id.clone()).create(DRAFT, email.clone());

        if !self.takes_calls_in_one_request(2) {
            return self.send_email_separately(
                account_id,
                &email_set,
                identity_id,
                on_success_update,
            );
        }

        let email_call_id = self.next_call_id();
        let submission_call_id = self.next_call_id();

        let submission_set = submission_request(
            account_id,
            identity_id,
            Id::new(format!("#{DRAFT}")),
            None,
            on_success_update,
        );

        let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION])
            .call("Email/set", &email_set, &email_call_id)?
            .call("EmailSubmission/set", &submission_set, &submission_call_id)?;
        let response = self.api_call(&request)?;

        let email_invocation = response
            .responses_for(&email_call_id)
            .next()
            .ok_or_else(|| Error::Protocol("no Email/set response".to_owned()))?;
        let email_response: SetResponse<Email> =
            serde_json::from_value(Self::unwrap_invocation(email_invocation, "Email/set")?)?;
        let created_email = expect_created(&email_response, DRAFT)?;

        let submission_invocation = response
            .responses_for(&submission_call_id)
            .next()
            .ok_or_else(|| Error::Protocol("no EmailSubmission/set response".to_owned()))?;
        let submission_arguments = backfill_submission_created(
            Self::unwrap_invocation(submission_invocation, "EmailSubmission/set")?,
            SUBMISSION,
            identity_id,
            created_email.id.as_ref().ok_or_else(|| {
                Error::Protocol("Email/set created a draft without an id".to_owned())
            })?,
        );
        let submission_response: SetResponse<EmailSubmission> =
            serde_json::from_value(submission_arguments)?;
        let submission = expect_created(&submission_response, SUBMISSION)?;

        Ok((created_email, submission))
    }

    /// [`Client::send_email`] as two requests, for a server that will not take
    /// the chain.
    fn send_email_separately(
        &self,
        account_id: &Id,
        email_set: &SetRequest<Email>,
        identity_id: &Id,
        on_success_update: Option<Value>,
    ) -> Result<(Email, EmailSubmission), Error> {
        const DRAFT: &str = "draft";
        const SUBMISSION: &str = "submission";

        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/set", email_set)?;
        let email_response: SetResponse<Email> = serde_json::from_value(arguments)?;
        let created_email = expect_created(&email_response, DRAFT)?;
        // The second request must use the server-assigned message ID.
        let email_id = created_email
            .id
            .clone()
            .ok_or_else(|| Error::Protocol("Email/set created a draft without an id".to_owned()))?;

        let submission_set = submission_request(
            account_id,
            identity_id,
            email_id.clone(),
            None,
            on_success_update,
        );
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &submission_set,
        )?;
        let submission_response: SetResponse<EmailSubmission> = serde_json::from_value(
            backfill_submission_created(arguments, SUBMISSION, identity_id, &email_id),
        )?;
        let submission = expect_created(&submission_response, SUBMISSION)?;

        Ok((created_email, submission))
    }

    /// Hand a message the account already holds to the server's submission
    /// machinery (`EmailSubmission/set`, RFC 8621 §7).
    ///
    /// Submits an existing email (such as an imported MIME message) by ID.
    ///
    /// `on_success_update` is a patch applied to the message once the server
    /// accepts the submission (RFC 8621 §7.5), moving it out of Drafts.
    pub fn submit_email(
        &self,
        account_id: &Id,
        email_id: &Id,
        identity_id: &Id,
        envelope: Option<Envelope>,
        on_success_update: Option<Value>,
    ) -> Result<EmailSubmission, Error> {
        const SUBMISSION: &str = "submission";

        let submission_set = submission_request(
            account_id,
            identity_id,
            email_id.clone(),
            envelope,
            on_success_update,
        );
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &submission_set,
        )?;
        let response: SetResponse<EmailSubmission> = serde_json::from_value(
            backfill_submission_created(arguments, SUBMISSION, identity_id, email_id),
        )?;
        expect_created(&response, SUBMISSION)
    }

    /// Submit email with delayed release via SMTP FUTURERELEASE (RFC 4865).
    ///
    /// Attaches the schedule parameters to the SMTP envelope.
    /// The server sets `undoStatus: "pending"` and assigns the `sendAt` timestamp.
    /// Callers can cancel pending submissions using [`Client::cancel_email_submission`].
    pub fn submit_email_at(
        &self,
        account_id: &Id,
        email_id: &Id,
        identity_id: &Id,
        envelope: Envelope,
        schedule: &Schedule,
        on_success_update: Option<Value>,
    ) -> Result<EmailSubmission, Error> {
        self.submit_email(
            account_id,
            email_id,
            identity_id,
            Some(envelope.with_schedule(schedule)),
            on_success_update,
        )
    }

    /// Attempt to cancel a still-pending [`Client::submit_email_at`] submission
    /// (RFC 8621 §7.4): an `EmailSubmission/set` update setting `undoStatus` to
    /// `"canceled"`. A submission the server has already sent answers
    /// [`Error::Set`] with `forbidden`: delivery that already happened cannot be undone.
    pub fn cancel_email_submission(
        &self,
        account_id: &Id,
        submission_id: &Id,
    ) -> Result<(), Error> {
        let request = SetRequest::<EmailSubmission>::new(account_id.clone()).update(
            submission_id.clone(),
            serde_json::json!({"undoStatus": "canceled"}),
        );
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/set",
            &request,
        )?;
        let response: SetResponse<EmailSubmission> = serde_json::from_value(arguments)?;
        expect_updated(&response, submission_id)
    }

    /// Fetch EmailSubmissions by id (`EmailSubmission/get`, RFC 8621 §7.4).
    ///
    /// Splits requests into chunks when IDs exceed `maxSizeRequest` or `maxObjectsInGet`.
    /// An ID naming no submission is silently absent from the result.
    pub fn email_submission_get(
        &self,
        account_id: &Id,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Result<Vec<EmailSubmission>, Error> {
        let all_ids: Vec<Id> = ids.into_iter().map(Into::into).collect();
        let response = self.get_chunked(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/get",
            account_id,
            &all_ids,
            None,
            |submission: &EmailSubmission| submission.id.as_ref(),
        )?;
        Ok(response.list)
    }

    /// Resolve submission ids matching `filter` (`EmailSubmission/query`, RFC
    /// 8621 §7.3): e.g. by `emailIds` or `undoStatus`, to find a submission
    /// without already knowing its id.
    pub fn email_submission_query(
        &self,
        account_id: &Id,
        filter: EmailSubmissionQueryFilter,
    ) -> Result<Vec<Id>, Error> {
        let request = QueryRequest::new(account_id.clone()).filter(filter);
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION],
            "EmailSubmission/query",
            &request,
        )?;
        let response: QueryResponse = serde_json::from_value(arguments)?;
        Ok(response.ids)
    }

    /// Upload a blob via the session's `uploadUrl` template (RFC 8620 §6.1).
    ///
    /// Refused client-side when data exceeds `maxSizeUpload`.
    pub fn upload_blob_slice(
        &self,
        account_id: &Id,
        content_type: &str,
        data: &[u8],
    ) -> Result<UploadResponse, Error> {
        let size = data.len() as u64;
        if let Some(limit) = self.session().max_size_upload()
            && size > limit
        {
            return Err(Error::TooLarge { size, limit });
        }

        let url = self
            .session()
            .upload_url
            .replace("{accountId}", &encode_template_value(account_id.as_str()));
        let response = match self.execute_with_content_type(
            HttpMethod::Post,
            &url,
            Some(data),
            Some(content_type),
        ) {
            Ok(response) => response,
            Err(Error::Http {
                status: 429,
                problem,
            }) => {
                let retry_after = crate::transport::observed_retry_after().or_else(|| {
                    problem
                        .as_ref()
                        .and_then(crate::client::extract_problem_retry_after)
                });
                return Err(Error::RateLimited {
                    retry_after,
                    method: Some("upload".to_string()),
                    problem,
                });
            }
            Err(err) => return Err(err),
        };
        Ok(serde_json::from_slice(&response.body)?)
    }

    /// Upload a binary blob via the session's `uploadUrl` template (RFC 8620
    /// §6.1).
    pub fn upload_blob(
        &self,
        account_id: &Id,
        content_type: &str,
        data: Vec<u8>,
    ) -> Result<UploadResponse, Error> {
        self.upload_blob_slice(account_id, content_type, &data)
    }

    /// Download raw blob bytes using the session's `downloadUrl` template (RFC 8620 Section 6.2).
    ///
    /// `account_id` and `blob_id` identify the blob.
    /// `name` supplies an optional download filename.
    /// All template parameters are percent-encoded.
    ///
    /// `max_bytes` sets the maximum accepted response size.
    /// Returns [`Error::ResponseTooLarge`] if the response exceeds this limit.
    /// Redirects to differing origins return [`Error::CrossOriginRedirect`].
    pub fn download_blob(
        &self,
        account_id: &Id,
        blob_id: &Id,
        name: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, Error> {
        let url = self
            .session()
            .download_url
            .replace("{accountId}", &encode_template_value(account_id.as_str()))
            .replace("{blobId}", &encode_template_value(blob_id.as_str()))
            .replace("{name}", &encode_template_value(name))
            .replace("{type}", &encode_template_value("application/octet-stream"));
        let response =
            self.execute_idempotent_within(HttpMethod::Get, &url, None, None, "*/*", max_bytes)?;
        if !crate::url::same_origin(&url, &response.final_url) {
            return Err(Error::CrossOriginRedirect {
                requested: crate::url::origin_of(&url).into_owned(),
                followed: crate::url::origin_of(&response.final_url).into_owned(),
                rebase_note: self.rebase_note().map(str::to_owned),
            });
        }
        Ok(response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Credentials;
    use crate::transport::{HttpRequest, HttpResponse, Transport, TransportError};
    use serde_json::json;
    use std::sync::Arc;

    struct TestTransport {
        handler: Arc<dyn Fn(&HttpRequest<'_>) -> HttpResponse + Send + Sync>,
    }

    impl Transport for TestTransport {
        fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            Ok((self.handler)(&req))
        }
    }

    fn test_client_with_session(session: Value) -> Client {
        Client::builder()
            .transport(TestTransport {
                handler: Arc::new(move |req| {
                    if req.method == HttpMethod::Get {
                        HttpResponse {
                            status: 200,
                            content_type: Some("application/json".to_owned()),
                            body: session.to_string().into_bytes(),
                            final_url: req.url.to_owned(),
                        }
                    } else {
                        panic!(
                            "unexpected request in test_client: {:?} {}",
                            req.method, req.url
                        );
                    }
                }),
            })
            .connect("https://mail.example.internal", Credentials::none())
            .expect("Client::connect succeeds")
    }

    fn test_session(max_size_request: Option<u64>) -> Value {
        let mut core_cap = json!({
            "maxSizeUpload": 50000000,
            "maxConcurrentUpload": 4,
            "maxConcurrentRequests": 4,
            "maxCallsInRequest": 16,
            "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
        });
        if let Some(limit) = max_size_request {
            core_cap["maxSizeRequest"] = json!(limit);
        }

        json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": core_cap,
                "urn:ietf:params:jmap:mail": {}
            },
            "accounts": {
                "acc1": {
                    "name": "user@example.com",
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": {
                        "urn:ietf:params:jmap:core": {},
                        "urn:ietf:params:jmap:mail": {}
                    }
                }
            },
            "primaryAccounts": {
                "urn:ietf:params:jmap:core": "acc1",
                "urn:ietf:params:jmap:mail": "acc1"
            },
            "username": "user@example.com",
            "apiUrl": "https://mail.example.internal/jmap/api",
            "downloadUrl": "https://mail.example.internal/jmap/download/{blobId}",
            "uploadUrl": "https://mail.example.internal/jmap/upload",
            "eventSourceUrl": "https://mail.example.internal/jmap/events",
            "state": "session-state-1"
        })
    }

    #[test]
    fn creation_outcome_success_refusal_and_protocol_error() {
        let mut created = BTreeMap::new();
        created.insert("item1".to_string(), "success_val".to_string());
        let res = creation_outcome(Some(&created), None, "item1");
        assert_eq!(res.unwrap(), "success_val");

        let mut not_created = BTreeMap::new();
        not_created.insert("item2".to_string(), SetError::new("invalidProperties"));
        let res: Result<String, Error> = creation_outcome(None, Some(&not_created), "item2");
        match res {
            Err(Error::Set(err)) => assert_eq!(err.error_type, "invalidProperties"),
            other => panic!("expected Error::Set, got {:?}", other),
        }

        let res: Result<String, Error> = creation_outcome(None, None, "item3");
        match res {
            Err(Error::Protocol(msg)) => {
                assert!(msg.contains("item3"));
            }
            other => panic!("expected Error::Protocol, got {:?}", other),
        }
    }

    #[test]
    fn in_query_order_sorts_by_ids_and_drops_unmatched() {
        let ids = vec![Id::new("m1"), Id::new("m2"), Id::new("m3")];
        let emails = vec![
            Email::new().with_id(Id::new("m2")),
            Email::new().with_id(Id::new("m1")),
            Email::new(),
            Email::new().with_id(Id::new("m_other")),
        ];
        let ordered = in_query_order(&ids, emails);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].id.as_ref().map(|id| id.as_str()), Some("m1"));
        assert_eq!(ordered[1].id.as_ref().map(|id| id.as_str()), Some("m2"));
    }

    #[test]
    fn backfill_submission_created_patches_missing_fields_and_preserves_existing() {
        let raw = json!({
            "created": {
                "sub1": {
                    "id": "s1"
                },
                "sub2": {
                    "id": "s2",
                    "identityId": "existing-identity",
                    "emailId": "existing-email"
                }
            }
        });
        let identity_id = Id::new("new-identity");
        let email_id = Id::new("new-email");

        let patched1 = backfill_submission_created(raw, "sub1", &identity_id, &email_id);
        assert_eq!(patched1["created"]["sub1"]["identityId"], "new-identity");
        assert_eq!(patched1["created"]["sub1"]["emailId"], "new-email");
        assert_eq!(patched1["created"]["sub1"]["id"], "s1");

        let patched2 = backfill_submission_created(patched1, "sub2", &identity_id, &email_id);
        assert_eq!(
            patched2["created"]["sub2"]["identityId"],
            "existing-identity"
        );
        assert_eq!(patched2["created"]["sub2"]["emailId"], "existing-email");

        let empty_raw = json!({"created": {}});
        let patched_empty =
            backfill_submission_created(empty_raw, "nonexistent", &identity_id, &email_id);
        assert_eq!(patched_empty, json!({"created": {}}));
    }

    #[test]
    fn submission_request_builds_expected_shape() {
        let account_id = Id::new("acc1");
        let identity_id = Id::new("ident1");
        let email_id = Id::new("m1");
        let patch = json!({"/keywords/$seen": true});

        let req = submission_request(
            &account_id,
            &identity_id,
            email_id.clone(),
            None,
            Some(patch.clone()),
        );

        let sub = req.set.create.as_ref().unwrap().get("submission").unwrap();
        assert_eq!(sub.identity_id, identity_id);
        assert_eq!(sub.email_id, email_id);
        assert!(sub.envelope.is_none());

        let patches = req.on_success_update_email.as_ref().unwrap();
        assert_eq!(patches.get("#submission"), Some(&patch));
        assert!(req.on_success_destroy_email.is_none());
    }

    #[test]
    fn ids_that_fit_boundaries_comma_cost_and_oversized_first_id() {
        let client = test_client_with_session(test_session(None));
        let account_id = Id::new("acc1");
        let ids = vec![Id::new("id_one"), Id::new("id_two"), Id::new("id_three")];
        let call_id = "c1";

        let empty_req = GetRequest::ids(account_id, std::iter::empty::<Id>());
        let empty = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL])
            .call("Email/get", &empty_req, call_id)
            .unwrap();
        let base_used = serde_json::to_vec(&empty).unwrap().len() as u64;

        let cost0 = serde_json::to_string(&ids[0]).unwrap().len() as u64;
        let cost1 = serde_json::to_string(&ids[1]).unwrap().len() as u64 + 1;
        let cost2 = serde_json::to_string(&ids[2]).unwrap().len() as u64 + 1;

        let under_first = base_used + cost0 - 1;
        let res = client.ids_that_fit(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/get",
            &empty_req,
            &ids,
            call_id,
            under_first,
        );
        match res {
            Err(Error::RequestTooLarge { size, limit }) => {
                assert_eq!(size, base_used + cost0);
                assert_eq!(limit, under_first);
            }
            other => panic!("expected RequestTooLarge, got {:?}", other),
        }

        let exact_first = base_used + cost0;
        let res = client
            .ids_that_fit(
                &[CAPABILITY_CORE, CAPABILITY_MAIL],
                "Email/get",
                &empty_req,
                &ids,
                call_id,
                exact_first,
            )
            .unwrap();
        assert_eq!(res, 1);

        let under_second = base_used + cost0 + cost1 - 1;
        let res = client
            .ids_that_fit(
                &[CAPABILITY_CORE, CAPABILITY_MAIL],
                "Email/get",
                &empty_req,
                &ids,
                call_id,
                under_second,
            )
            .unwrap();
        assert_eq!(res, 1);

        let exact_second = base_used + cost0 + cost1;
        let res = client
            .ids_that_fit(
                &[CAPABILITY_CORE, CAPABILITY_MAIL],
                "Email/get",
                &empty_req,
                &ids,
                call_id,
                exact_second,
            )
            .unwrap();
        assert_eq!(res, 2);

        let all_fit = base_used + cost0 + cost1 + cost2;
        let res = client
            .ids_that_fit(
                &[CAPABILITY_CORE, CAPABILITY_MAIL],
                "Email/get",
                &empty_req,
                &ids,
                call_id,
                all_fit,
            )
            .unwrap();
        assert_eq!(res, 3);
    }
}
