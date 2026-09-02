// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail operations (RFC 8621).

use std::collections::BTreeMap;

use jmap_proto::error::SetError;
use jmap_proto::mail::{
    Email, EmailImport, EmailImportRequest, EmailImportResponse, EmailQueryFilter, EmailSubmission,
    EmailSubmissionSetRequest, Envelope, Identity, Mailbox, Thread, VacationResponse,
};
use jmap_proto::methods::{
    Comparator, Filter, GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest,
    SetResponse, UploadResponse,
};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{
    CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION, CAPABILITY_VACATION_RESPONSE,
};
use jmap_proto::{Id, State, UtcDate};
use serde_json::Value;

use crate::client::Client;
use crate::error::Error;
use crate::transport::HttpMethod;
use crate::url::encode_template_value;

/// Pull the object for `creation_id` out of a `/set` response, mapping a
/// rejection to [`Error::Set`].
fn expect_created<T: Clone>(response: &SetResponse<T>, creation_id: &str) -> Result<T, Error> {
    creation_outcome(
        response.created.as_ref(),
        response.not_created.as_ref(),
        creation_id,
    )
}

/// The same question of the two maps alone, for a method that creates without
/// being a `/set` — `Email/import` (RFC 8621 §4.8), whose response carries
/// `created` and `notCreated` and none of the rest of the `/set` shape.
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
/// `/get` does not preserve the order of the ids it was asked for (RFC 8620
/// §5.1), so the sort a query was made with survives only if it is reapplied
/// here — and a message destroyed between the query and the fetch is simply
/// absent, which is the same shape as a `notFound` and is why this filters
/// rather than fails.
fn in_query_order(ids: &[Id], emails: Vec<Email>) -> Vec<Email> {
    let mut by_id: BTreeMap<Id, Email> = emails
        .into_iter()
        .filter_map(|email| email.id.clone().map(|id| (id, email)))
        .collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

/// Fill in `identityId`/`emailId` on a just-created `EmailSubmission` if the
/// server left them out.
///
/// RFC 8620 §5.3 says the `created` map need only carry properties "that
/// were not sent by the client" — and a client always states both when it
/// asks to create a submission, so neither is server-set. This client
/// assumed both would be echoed back in full until the live Stalwart
/// deployment took the RFC at its word and omitted them, which
/// `EmailSubmission`'s deserialization (both fields non-optional, since
/// every other caller of this type needs them) then panicked on. Rather than
/// make the fields optional everywhere — every consumer of a fetched or
/// mock-seeded `EmailSubmission` reasonably expects them — this patches the
/// raw JSON with values the caller already knows before deserializing, since
/// the whole reason the server may omit them is that the caller supplied
/// them itself moments earlier.
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

/// The `EmailSubmission/set` half of sending: submit `email_id` through
/// `identity_id`, and patch the message once the server accepts it.
///
/// `email_id` is a `#draft` creation reference when the draft is being created
/// in the same request and the message's real id when it is not — the one place
/// the two forms of [`Client::send_email`] differ. The `on_success_update` key
/// stays a creation reference either way: it names the *submission*, which is
/// always created by this very call.
///
/// `envelope` is the SMTP envelope the message is to go out with. `None` is not
/// "no recipients": RFC 8621 §7 has the server derive one from the message's
/// own headers, which is right for a message this client composed and wrong for
/// one it was handed — a `Bcc` recipient has no header to be derived from.
fn submission_request(
    account_id: &Id,
    identity_id: &Id,
    email_id: Id,
    envelope: Option<Envelope>,
    send_at: Option<UtcDate>,
    on_success_update: Option<Value>,
) -> EmailSubmissionSetRequest {
    const SUBMISSION: &str = "submission";
    EmailSubmissionSetRequest {
        set: SetRequest::<EmailSubmission>::new(account_id.clone()).create(
            SUBMISSION,
            EmailSubmission {
                id: None,
                identity_id: identity_id.clone(),
                email_id,
                thread_id: None,
                envelope,
                send_at,
                undo_status: None,
                delivery_status: None,
                extra: Default::default(),
            },
        ),
        on_success_update_email: on_success_update
            .map(|patch| [(format!("#{SUBMISSION}"), patch)].into()),
        on_success_destroy_email: None,
    }
}

impl Client {
    /// Fetch all mailboxes of an account (`Mailbox/get` with `ids: null`).
    ///
    /// The whole response, not just the list: its `state` is what
    /// `Mailbox/changes` is asked from, and a folder list without the state it
    /// was current at is one that can only ever be re-fetched in full.
    pub fn mailbox_get(&self, account_id: &Id) -> Result<GetResponse<Mailbox>, Error> {
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/get",
            &GetRequest::all(account_id.clone()),
        )?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Ask the server to make a mailbox (`Mailbox/set` create); answers the
    /// one it made.
    ///
    /// Answered with the object rather than with nothing because the id is the
    /// server's to hand out: a folder the caller cannot name afterwards is one
    /// it would have to go looking for by name, in a list where the name is
    /// unique only among siblings.
    ///
    /// A refusal is [`Error::Set`] with the server's own reason — RFC 8621 §2's
    /// `invalidProperties` for a name a sibling already has, a `parentId` that
    /// is not a mailbox, or a role the account has already given away.
    pub fn mailbox_create(&self, account_id: &Id, mailbox: &Mailbox) -> Result<Mailbox, Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).create("new", mailbox.clone());
        let response = self.mailbox_set(&request)?;
        expect_created(&response, "new")
    }

    /// Change a mailbox (`Mailbox/set` update): its name, where it hangs, or
    /// whether it is subscribed.
    ///
    /// A `PatchObject` (RFC 8620 §5.3) rather than a whole `Mailbox`, for the
    /// reason [`Client::email_update`] takes one: most of what a `Mailbox`
    /// carries is the server's — the message counts above all — and sending it
    /// back would be a client telling a server what it has just been told.
    pub fn mailbox_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).update(id.clone(), patch);
        let response = self.mailbox_set(&request)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        Err(crate::contacts::set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(id)),
        ))
    }

    /// Remove a mailbox (`Mailbox/set` destroy).
    ///
    /// The two refusals worth expecting are RFC 8621 §2.5's own:
    /// `mailboxHasChild` and `mailboxHasEmail`, which arrive as
    /// [`Error::Set`] and are the server declining rather than failing — what
    /// is inside the folder is the user's to decide about, and this client
    /// sends no `onDestroyRemoveEmails`.
    pub fn mailbox_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<Mailbox>::new(account_id.clone()).destroy(id.clone());
        let response = self.mailbox_set(&request)?;
        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        Err(crate::contacts::set_failure(
            response.not_destroyed.as_ref().and_then(|map| map.get(id)),
        ))
    }

    fn mailbox_set(&self, request: &SetRequest<Mailbox>) -> Result<SetResponse<Mailbox>, Error> {
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Mailbox/set", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Fetch the account's `VacationResponse` (RFC 8621 §8.1).
    ///
    /// Returned as the object itself rather than a list or a [`GetResponse`]:
    /// this is a singleton, so `list` always holds exactly the one object a
    /// server that follows RFC 8621 answers even before any client has ever
    /// set anything.
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

    /// Change the vacation responder (`VacationResponse/set` update — RFC
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
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(&id))
        {
            return Ok(());
        }
        Err(crate::contacts::set_failure(
            response.not_updated.as_ref().and_then(|map| map.get(&id)),
        ))
    }

    /// `Email/query`: matching email ids.
    ///
    /// `position` is the offset into the result set the answer should start at
    /// (RFC 8620 §5.5) — 0 for the first page. A server may answer with fewer
    /// ids than were asked for whether or not the client set a `limit`, and
    /// reports the cap it applied in [`QueryResponse::limit`], so a caller that
    /// wants the whole result set asks again from where the last answer ended.
    ///
    /// `filter` takes a plain [`EmailQueryFilter`] or a
    /// [`Filter<EmailQueryFilter>`] AND/OR/NOT tree (RFC 8620 §5.5) — the
    /// former converts via [`Filter::condition`], so every existing caller
    /// passing a flat filter keeps compiling unchanged.
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

    /// The account's `Email` state, without fetching a single message.
    ///
    /// `Email/get` naming no ids at all: RFC 8620 §5.1 has the response carry
    /// the type's current state whatever the list came back as, so an empty
    /// request is the one way to learn a state without paying for the objects
    /// it describes. `Email/changes` is asked from this.
    ///
    /// Why a caller wants it *before* it wants the messages: a state read after
    /// a listing was taken already covers whatever arrived while it was being
    /// taken, and a delta asked from it would never mention those messages
    /// again.
    pub fn email_state(&self, account_id: &Id) -> Result<State, Error> {
        let mut request = GetRequest::ids(account_id.clone(), std::iter::empty::<Id>());
        // Asked for explicitly, so that a server which answers a property list
        // it does not understand with an error cannot be handed `null` — and so
        // that nothing here depends on the empty id list being honoured.
        request.properties = Some(vec!["id".to_owned()]);
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/get", &request)?;
        let response: GetResponse<Email> = serde_json::from_value(arguments)?;
        Ok(response.state)
    }

    /// `Email/get` for explicit ids.
    ///
    /// Sent as several requests when naming every id at once would be longer
    /// than the session's `maxSizeRequest` (RFC 8620 §2). This is the one call
    /// this client builds whose length is the user's mailbox rather than the
    /// client's choice — a folder of ten thousand messages is a list of ten
    /// thousand ids — and over the limit the server refuses the *request*, so
    /// the alternative to splitting is fetching nothing.
    ///
    /// The number of ids is bounded elsewhere, by `maxObjectsInGet`, and by the
    /// caller: this is about octets, and the two limits do not imply each other
    /// — ids may be up to 255 characters (RFC 8620 §1.2), so a list well inside
    /// one limit can be well outside the other.
    ///
    /// Splitting is not free, and is not done when it is not needed: between
    /// two requests another client may destroy a message the first named, and
    /// it comes back one short rather than as an error. A server that names no
    /// limit is sent the list whole.
    pub fn email_get(
        &self,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let limit = self.session().max_size_request();
        let mut fetched: Vec<Email> = Vec::with_capacity(ids.len());
        let mut rest = ids;
        loop {
            let call_id = self.next_call_id();
            let take = match limit {
                Some(limit) => self.ids_that_fit(account_id, rest, properties, &call_id, limit)?,
                None => rest.len(),
            };
            let (chunk, remaining) = rest.split_at(take);
            let request = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL]).call(
                "Email/get",
                &Self::email_get_arguments(account_id, chunk, properties),
                &call_id,
            )?;
            let response = self.api_call(&request)?;
            let invocation = response
                .responses_for(&call_id)
                .next()
                .ok_or_else(|| Error::Protocol("no Email/get response".to_owned()))?;
            let arguments = Self::unwrap_invocation(invocation, "Email/get")?;
            let get_response: GetResponse<Email> = serde_json::from_value(arguments)?;
            fetched.extend(get_response.list);

            rest = remaining;
            if rest.is_empty() {
                return Ok(fetched);
            }
        }
    }

    /// The arguments of an `Email/get` naming `ids`.
    fn email_get_arguments(account_id: &Id, ids: &[Id], properties: Option<&[&str]>) -> GetRequest {
        let mut request = GetRequest::ids(account_id.clone(), ids.iter().cloned());
        request.properties =
            properties.map(|properties| properties.iter().map(|s| s.to_string()).collect());
        request
    }

    /// How many of `ids`, from the front, an `Email/get` under `call_id` may
    /// name before the request goes over `limit` octets.
    ///
    /// Measured rather than estimated, and measured once: the request naming no
    /// ids at all is everything whose length is not the id list, and a JSON
    /// array grows by exactly each element's serialized length plus one comma
    /// between them. So one serialization places every boundary, and it places
    /// them on the same count the server will — the bytes counted here are the
    /// bytes [`Client::api_call`] sends.
    ///
    /// [`Error::RequestTooLarge`] when even the first id does not fit, which is
    /// where splitting runs out: a call naming one id cannot be made into two.
    fn ids_that_fit(
        &self,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
        call_id: &str,
        limit: u64,
    ) -> Result<usize, Error> {
        let empty = Request::new([CAPABILITY_CORE, CAPABILITY_MAIL]).call(
            "Email/get",
            &Self::email_get_arguments(account_id, &[], properties),
            call_id,
        )?;
        let mut used = serde_json::to_vec(&empty)?.len() as u64;

        for (taken, id) in ids.iter().enumerate() {
            // The comma that separates this id from the one before it.
            let cost = serde_json::to_string(id)?.len() as u64 + u64::from(taken > 0);
            if used + cost > limit {
                if taken == 0 {
                    return Err(Error::RequestTooLarge {
                        size: used + cost,
                        limit,
                    });
                }
                return Ok(taken);
            }
            used += cost;
        }
        Ok(ids.len())
    }

    /// `Email/query` chained to `Email/get` through a `#ids` back-reference —
    /// one round-trip (RFC 8620 §3.7).
    ///
    /// Two round-trips against a server whose `maxCallsInRequest` is smaller
    /// than the chain: the query first, then a `/get` naming the ids it
    /// answered. The result is the same list in the same order; what it costs
    /// is the round trip the back-reference exists to save, which is the right
    /// price for reading mail at all. Splitting is not merely slower, though —
    /// between the two requests another client may destroy a message the query
    /// found, and it comes back one short rather than as an error. The chain
    /// has no such window, which is the second reason it stays the default.
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
        get.ids_ref = Some(ResultReference {
            result_of: query_call_id.clone(),
            name: "Email/query".to_owned(),
            path: "/ids".to_owned(),
        });
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
        // Nothing matched, so there is nothing to fetch — and an `Email/get`
        // naming no ids is a whole round trip spent on an empty list. The
        // chained form has no equivalent case: its `/get` travels with the
        // query whether or not the query finds anything.
        if query_response.ids.is_empty() {
            return Ok(Vec::new());
        }
        let emails = self.email_get(account_id, &query_response.ids, properties)?;
        Ok(in_query_order(&query_response.ids, emails))
    }

    /// Apply a patch to one message (`Email/set` with a single `update`).
    ///
    /// A `PatchObject` (RFC 8620 §5.3), not a whole `Email`: most of what an
    /// `Email` holds is immutable (RFC 8621 §4.1), and the two members that are
    /// not — `keywords` and `mailboxIds` — are sets other clients change too.
    /// Sending either of them whole would overwrite whatever happened to the
    /// message since it was last read.
    ///
    /// A rejection is [`Error::Set`] with the server's own reason, which is how
    /// `notFound` — the message was destroyed between the read and the write —
    /// stays distinguishable from a refusal.
    pub fn email_update(&self, account_id: &Id, id: &Id, patch: Value) -> Result<(), Error> {
        let request = SetRequest::<Email>::new(account_id.clone()).update(id.clone(), patch);
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/set", &request)?;
        let response: SetResponse<Email> = serde_json::from_value(arguments)?;

        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(id))
        {
            return Ok(());
        }
        if let Some(set_error) = response
            .not_updated
            .as_ref()
            .and_then(|not_updated| not_updated.get(id))
        {
            return Err(Error::Set(set_error.clone()));
        }
        Err(Error::Protocol(format!(
            "Email/set answered neither updated nor notUpdated for {id}"
        )))
    }

    /// Remove one message from the store entirely (`Email/set` destroy).
    ///
    /// The one write here with no `PatchObject` in it, because it is the one
    /// that is not about a property: RFC 8621 §4.6 has a destroyed `Email`
    /// leave every mailbox it was in and stop existing, which is what makes it
    /// the wrong call for a message that is filed in more than one place. A
    /// caller that means "take it out of *this* mailbox" wants
    /// [`Client::email_update`] over `mailboxIds` instead.
    ///
    /// `notFound` — another client destroyed the message first — arrives as
    /// [`Error::Set`] like every other refusal, so the caller can tell it from
    /// a server that would not do it.
    pub fn email_destroy(&self, account_id: &Id, id: &Id) -> Result<(), Error> {
        let request = SetRequest::<Email>::new(account_id.clone()).destroy(id.clone());
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/set", &request)?;
        let response: SetResponse<Email> = serde_json::from_value(arguments)?;

        if response
            .destroyed
            .as_ref()
            .is_some_and(|destroyed| destroyed.contains(id))
        {
            return Ok(());
        }
        if let Some(set_error) = response
            .not_destroyed
            .as_ref()
            .and_then(|not_destroyed| not_destroyed.get(id))
        {
            return Err(Error::Set(set_error.clone()));
        }
        Err(Error::Protocol(format!(
            "Email/set answered neither destroyed nor notDestroyed for {id}"
        )))
    }

    /// Put a message into the store from bytes already uploaded
    /// (`Email/import`, RFC 8621 §4.8); answers the `Email` the server made of
    /// it.
    ///
    /// The two-step — [`Client::upload_blob`] and then this — is the protocol's,
    /// not this client's: an import names a blob, and there is no way to send a
    /// message's bytes inside a method call. It is also the only way to add a
    /// message that *is* a message rather than a set of properties. An
    /// `Email/set` create builds one out of `from`, `subject` and body values,
    /// which is what composing a draft does; a message Evolution already holds
    /// — a draft it wrote itself, a message being copied out of another account
    /// — exists as RFC 5322 bytes, and importing them is how they survive
    /// intact rather than being taken apart and reassembled by a server.
    ///
    /// Answered with the `Email` because its `id` is the server's to hand out
    /// and the caller has nothing to look the message up by until it has one.
    /// The RFC has the server fill in `id`, `blobId`, `threadId` and `size`, and
    /// `blobId` is worth reading rather than assuming: a server that repaired
    /// the message answers with the blob it stored, not the one it was given.
    ///
    /// A refusal is [`Error::Set`] — `invalidProperties` for a blob or mailbox
    /// that is not there, `invalidEmail` for bytes the server will not read as a
    /// message, `overQuota` for an account that is full.
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

    /// Fetch Threads by id (`Thread/get`, RFC 8621 §3.1). Ids come from
    /// `Email::thread_id`; an id naming no thread is silently absent from the
    /// result, the same as [`Client::email_get`] treats a missing `Email` id.
    pub fn thread_get(
        &self,
        account_id: &Id,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Result<Vec<Thread>, Error> {
        let request = GetRequest::ids(account_id.clone(), ids);
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Thread/get", &request)?;
        let response: GetResponse<Thread> = serde_json::from_value(arguments)?;
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
    /// submission is accepted (RFC 8621 §7.5) — typically moving it to Sent.
    /// Returns the server-set email properties and the accepted submission.
    ///
    /// Two requests against a server whose `maxCallsInRequest` is smaller than
    /// the chain, with the submission naming the draft's real id instead of
    /// `#draft`. What that costs is not only a round trip: the draft exists
    /// alone between the two, so a submission the server refuses leaves it
    /// behind in Drafts. The chained form leaves it behind too — RFC 8620 §3.2
    /// runs the calls of a request in order and does not undo the earlier one
    /// when a later one fails — so the difference is the window, not the
    /// outcome: split, the draft is visible to the user's other clients while
    /// it lasts.
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
        // The id the chained form never needs: `#draft` resolves only inside
        // the request that created the draft, so the second request has to name
        // the message the server actually made.
        let email_id = created_email
            .id
            .clone()
            .ok_or_else(|| Error::Protocol("Email/set created a draft without an id".to_owned()))?;

        let submission_set = submission_request(
            account_id,
            identity_id,
            email_id.clone(),
            None,
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
    /// The half of [`Client::send_email`] that does not compose. Sending a
    /// message another program built — a `CamelMimeMessage` out of Evolution's
    /// composer — cannot go through an `Email/set` create, because that names
    /// the message by properties and what has to go out is the *bytes*: a
    /// message taken apart and rebuilt is no longer the one the sender signed.
    /// So the message arrives in the account through `Email/import` and this is
    /// what submits it afterwards, naming it by the id the import minted.
    ///
    /// `envelope` is the SMTP envelope, and passing one is the ordinary case
    /// here rather than the exception: the caller was given the recipients
    /// separately from the message and they are not the same thing as its
    /// headers. See `submission_request`.
    ///
    /// `on_success_update` is a patch applied to the message once the server
    /// accepts the submission (RFC 8621 §7.5) — moving it out of the mailbox it
    /// was staged in, and out of being a draft.
    pub fn submit_email(
        &self,
        account_id: &Id,
        email_id: &Id,
        identity_id: &Id,
        envelope: Option<Envelope>,
        on_success_update: Option<Value>,
    ) -> Result<EmailSubmission, Error> {
        self.submit_email_at(
            account_id,
            email_id,
            identity_id,
            envelope,
            None,
            on_success_update,
        )
    }

    /// [`Client::submit_email`], with an RFC 8621 §7.1 `sendAt` in the
    /// future: the server holds the message rather than delivering it
    /// immediately, and answers with `undoStatus: "pending"` instead of
    /// `"final"`. Only meaningful against a server whose submission account
    /// capability names a `maxDelayedSend`
    /// ([`jmap_proto::session::Account::max_delayed_send`]) — nothing here
    /// checks that before sending, since a server that never advertised
    /// support is free to refuse or ignore `sendAt` on its own terms, the
    /// same as any other capability-gated property.
    ///
    /// Nothing in this project's EDS integration calls this yet: Evolution
    /// has no scheduled-send UI or Camel plumbing to drive it.
    /// This exists so the client side is ready
    /// and proven against the day that changes.
    pub fn submit_email_at(
        &self,
        account_id: &Id,
        email_id: &Id,
        identity_id: &Id,
        envelope: Option<Envelope>,
        send_at: Option<UtcDate>,
        on_success_update: Option<Value>,
    ) -> Result<EmailSubmission, Error> {
        const SUBMISSION: &str = "submission";

        let submission_set = submission_request(
            account_id,
            identity_id,
            email_id.clone(),
            envelope,
            send_at,
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

    /// Attempt to cancel a still-pending [`Client::submit_email_at`] submission
    /// (RFC 8621 §7.4): an `EmailSubmission/set` update setting `undoStatus` to
    /// `"canceled"`. A submission the server has already sent answers
    /// [`Error::Set`] with `forbidden` — undoing a delivery that already
    /// happened is not on offer, spec or otherwise.
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
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(submission_id))
        {
            return Ok(());
        }
        Err(crate::contacts::set_failure(
            response
                .not_updated
                .as_ref()
                .and_then(|map| map.get(submission_id)),
        ))
    }

    /// Upload a blob via the session's `uploadUrl` template (RFC 8620 §6.1).
    ///
    /// Refused here, without a request, when the data is larger than the
    /// session's `maxSizeUpload`. The limit is in the session document exactly
    /// so a client can ask before it sends: an upload is the one request whose
    /// body is the whole message, and finding out it was too big by sending it
    /// costs the user the upload — over a slow link, minutes of it — for an
    /// answer that was already on hand. What comes back is
    /// [`Error::TooLarge`], carrying both numbers, rather than the server's
    /// `urn:ietf:params:jmap:error:limit`, which cannot be told apart from the
    /// other request-level limits by anything but its `limit` property.
    ///
    /// A server that names no limit is sent the data: see
    /// [`Session::max_size_upload`] for why no number is invented for it.
    ///
    /// [`Session::max_size_upload`]: jmap_proto::session::Session::max_size_upload
    pub fn upload_blob(
        &self,
        account_id: &Id,
        content_type: &str,
        data: Vec<u8>,
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
        let response = self.execute_with_content_type(
            HttpMethod::Post,
            &url,
            Some(&data),
            Some(content_type),
        )?;
        Ok(serde_json::from_slice(&response.body)?)
    }

    /// Download a blob's raw bytes via the session's `downloadUrl` template
    /// (RFC 8620 §6.2).
    ///
    /// The `name` is a label for the download, not part of its identity: the
    /// blob is addressed by `account_id` and `blob_id` alone, and the name is
    /// only what the server may echo as the `Content-Disposition` filename.
    /// It still has to be encoded — a label is no less able to reshape a URL
    /// than an id is.
    ///
    /// Every substituted value is percent-encoded — see `crate::url` for why
    /// that matters when the values are the server's own.
    ///
    /// `max_bytes` is how many octets of answer the caller will take, and it is
    /// a parameter rather than a constant because the good number is the
    /// account's: `Email/get` reports each message's `size`, RFC 8621 §4.1.1
    /// defines that as the octets this download returns, and a caller holding
    /// that row already knows what it is about to receive. A caller with no
    /// such number passes [`crate::limits::MAX_BLOB_BYTES`]. Exactly
    /// `max_bytes` arrives; more is [`Error::ResponseTooLarge`], with the body
    /// abandoned at the ceiling rather than buffered and then judged.
    ///
    /// Refuses a response that arrived via a redirect to a different origin
    /// than `downloadUrl` itself named ([`Error::CrossOriginRedirect`]) — a
    /// blob is raw bytes, with no shape a wrong answer could fail to match,
    /// so nothing else here would catch a redirect target's own unrelated 200
    /// being handed back as if it were the message.
    ///
    /// Declares `Accept: */*`, not `application/json` — every other request
    /// this client makes answers with JSON, but a blob download never does,
    /// and RFC 8620 §6.2 gives it no reason to claim otherwise; a server
    /// doing content negotiation on that header is free to refuse or
    /// redirect a request that says it only accepts JSON for a response that
    /// is not.
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
        let response = self.execute_within(HttpMethod::Get, &url, None, None, "*/*", max_bytes)?;
        if crate::url::origin_of(&response.final_url) != crate::url::origin_of(&url) {
            return Err(Error::CrossOriginRedirect {
                requested: crate::url::origin_of(&url).to_owned(),
                followed: crate::url::origin_of(&response.final_url).to_owned(),
                rebase_note: self.rebase_note().map(str::to_owned),
            });
        }
        Ok(response.body)
    }
}
