// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every id that a `/query` matches, on all the pages of the server (RFC 8620 section 5.5).

use jmap_proto::Id;
use jmap_proto::methods::{Comparator, QueryRequest, QueryResponse};
use serde::Serialize;

use crate::client::Client;
use crate::error::Error;

/// The maximum number of pages in one list.
const MAX_QUERY_PAGES: usize = 1024;

impl Client {
    /// Gets all pages of `method` for `filter` and optional `sort`. It asks for the next page while the server
    /// reports a `limit` and the page has ids.
    ///
    /// It fails when the ids go past the id budget, or do not agree with the `total` of the
    /// server. A partial list must not reach the caller: EDS deletes each cached item that
    /// the list does not have.
    pub fn query_every_id<F: Serialize + Clone>(
        &self,
        using: &[&str],
        method: &str,
        account_id: &Id,
        filter: F,
        sort: Option<Vec<Comparator>>,
    ) -> Result<Vec<Id>, Error> {
        let mut ids: Vec<Id> = Vec::new();
        let mut budget = self.id_budget();

        for _ in 0..MAX_QUERY_PAGES {
            let mut request = QueryRequest::new(account_id.clone()).filter(filter.clone());
            request.sort = sort.clone();
            request.position = ids.len() as i64;
            request.calculate_total = true;
            let arguments = self.single_call(using, method, &request)?;
            let response: QueryResponse = serde_json::from_value(arguments)?;
            let capped = response.limit.is_some();
            let answered = !response.ids.is_empty();
            if !response.ids.iter().all(|id| budget.charge(id.as_str())) {
                return Err(Error::Protocol(format!(
                    "{method} reports more ids than the budget for one sync"
                )));
            }
            ids.extend(response.ids);
            // No limit: all remaining ids are in this page. A limited empty page: no ids remain.
            if !capped || !answered {
                return match response.total {
                    Some(total) if total != ids.len() as u64 => Err(Error::Protocol(format!(
                        "{method} gave {} ids, but reports a total of {total}",
                        ids.len()
                    ))),
                    _ => Ok(ids),
                };
            }
        }
        Err(Error::Protocol(format!(
            "{method} never stopped reporting a limited answer"
        )))
    }
}
