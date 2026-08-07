// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail operations (RFC 8621).

use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailQueryFilter, Mailbox};
use jmap_proto::methods::{Comparator, GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};

use crate::client::Client;
use crate::error::Error;
use crate::transport::HttpMethod;

impl Client {
    /// Fetch all mailboxes of an account (`Mailbox/get` with `ids: null`).
    pub fn mailboxes(&self, account_id: &Id) -> Result<Vec<Mailbox>, Error> {
        let arguments = self.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/get",
            &GetRequest::all(account_id.clone()),
        )?;
        let response: GetResponse<Mailbox> = serde_json::from_value(arguments)?;
        Ok(response.list)
    }

    /// `Email/query`: matching email ids.
    pub fn email_query(
        &self,
        account_id: &Id,
        filter: EmailQueryFilter,
        sort: Option<Vec<Comparator>>,
        limit: Option<u64>,
    ) -> Result<QueryResponse, Error> {
        let mut request = QueryRequest::new(account_id.clone()).filter(filter);
        request.sort = sort;
        request.limit = limit;
        let arguments =
            self.single_call(&[CAPABILITY_CORE, CAPABILITY_MAIL], "Email/query", &request)?;
        Ok(serde_json::from_value(arguments)?)
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
