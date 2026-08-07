// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Response envelope (RFC 8620 §3.4).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::request::Invocation;
use crate::state::State;

/// A JMAP API response mirroring [`crate::request::Request`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub method_responses: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    pub session_state: State,
}

impl Response {
    /// All responses belonging to the method call with the given call id (a
    /// single call may produce several responses).
    pub fn responses_for<'a>(
        &'a self,
        call_id: &'a str,
    ) -> impl Iterator<Item = &'a Invocation> {
        self.method_responses
            .iter()
            .filter(move |invocation| invocation.call_id == call_id)
    }
}
