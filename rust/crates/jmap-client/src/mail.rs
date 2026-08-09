// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail operations (RFC 8621).

use jmap_proto::mail::{
    Email, EmailQueryFilter, EmailSubmission, EmailSubmissionSetRequest, Identity, Mailbox,
};
use jmap_proto::methods::{
    Comparator, GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
    UploadResponse,
};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION};
use jmap_proto::{Id, State};
use serde_json::Value;

use crate::client::Client;
use crate::error::Error;
use crate::transport::HttpMethod;

/// Pull the object for `creation_id` out of a `/set` response, mapping a
/// rejection to [`Error::Set`].
fn expect_created<T: Clone>(response: &SetResponse<T>, creation_id: &str) -> Result<T, Error> {
    if let Some(object) = response
        .created
        .as_ref()
        .and_then(|created| created.get(creation_id))
    {
        return Ok(object.clone());
    }
    if let Some(set_error) = response
        .not_created
        .as_ref()
        .and_then(|not_created| not_created.get(creation_id))
    {
        return Err(Error::Set(set_error.clone()));
    }
    Err(Error::Protocol(format!(
        "server response contains neither created nor notCreated for {creation_id}"
    )))
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

    /// `Email/query`: matching email ids.
    ///
    /// `position` is the offset into the result set the answer should start at
    /// (RFC 8620 §5.5) — 0 for the first page. A server may answer with fewer
    /// ids than were asked for whether or not the client set a `limit`, and
    /// reports the cap it applied in [`QueryResponse::limit`], so a caller that
    /// wants the whole result set asks again from where the last answer ended.
    pub fn email_query(
        &self,
        account_id: &Id,
        filter: EmailQueryFilter,
        sort: Option<Vec<Comparator>>,
        limit: Option<u64>,
        position: i64,
    ) -> Result<QueryResponse, Error> {
        let mut request = QueryRequest::new(account_id.clone()).filter(filter);
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
    pub fn email_get(
        &self,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let mut request = GetRequest::ids(account_id.clone(), ids.iter().cloned());
        request.properties =
            properties.map(|properties| properties.iter().map(|s| s.to_string()).collect());
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/get", &request)?;
        let response: GetResponse<Email> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// `Email/query` chained to `Email/get` through a `#ids` back-reference —
    /// one round-trip (RFC 8620 §3.7).
    pub fn email_query_then_get(
        &self,
        account_id: &Id,
        filter: EmailQueryFilter,
        sort: Option<Vec<Comparator>>,
        properties: Option<&[&str]>,
    ) -> Result<Vec<Email>, Error> {
        let query_call_id = self.next_call_id();
        let get_call_id = self.next_call_id();

        let mut query = QueryRequest::new(account_id.clone()).filter(filter);
        query.sort = sort;

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

        let mut by_id: std::collections::BTreeMap<Id, Email> = get_response
            .list
            .into_iter()
            .filter_map(|email| email.id.clone().map(|id| (id, email)))
            .collect();
        Ok(query_response
            .ids
            .iter()
            .filter_map(|id| by_id.remove(id))
            .collect())
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
    pub fn send_email(
        &self,
        account_id: &Id,
        email: &Email,
        identity_id: &Id,
        on_success_update: Option<Value>,
    ) -> Result<(Email, EmailSubmission), Error> {
        const DRAFT: &str = "draft";
        const SUBMISSION: &str = "submission";

        let email_call_id = self.next_call_id();
        let submission_call_id = self.next_call_id();

        let email_set = SetRequest::<Email>::new(account_id.clone()).create(DRAFT, email.clone());
        let submission_set = EmailSubmissionSetRequest {
            set: SetRequest::<EmailSubmission>::new(account_id.clone()).create(
                SUBMISSION,
                EmailSubmission {
                    id: None,
                    identity_id: identity_id.clone(),
                    email_id: Id::new(format!("#{DRAFT}")),
                    thread_id: None,
                    envelope: None,
                    send_at: None,
                    undo_status: None,
                    extra: Default::default(),
                },
            ),
            on_success_update_email: on_success_update
                .map(|patch| [(format!("#{SUBMISSION}"), patch)].into()),
            on_success_destroy_email: None,
        };

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
        let submission_response: SetResponse<EmailSubmission> = serde_json::from_value(
            Self::unwrap_invocation(submission_invocation, "EmailSubmission/set")?,
        )?;
        let submission = expect_created(&submission_response, SUBMISSION)?;

        Ok((created_email, submission))
    }

    /// Upload a blob via the session's `uploadUrl` template (RFC 8620 §6.1).
    pub fn upload_blob(
        &self,
        account_id: &Id,
        content_type: &str,
        data: Vec<u8>,
    ) -> Result<UploadResponse, Error> {
        let url = self
            .session()
            .upload_url
            .replace("{accountId}", account_id.as_str());
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
    pub fn download_blob(
        &self,
        account_id: &Id,
        blob_id: &Id,
        name: &str,
    ) -> Result<Vec<u8>, Error> {
        let url = self
            .session()
            .download_url
            .replace("{accountId}", account_id.as_str())
            .replace("{blobId}", blob_id.as_str())
            .replace("{name}", name)
            .replace("{type}", "application/octet-stream");
        let response = self.execute(HttpMethod::Get, &url, None)?;
        Ok(response.body)
    }
}
