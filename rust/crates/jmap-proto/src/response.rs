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
    #[serde(default)]
    pub method_responses: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    #[serde(default)]
    pub session_state: State,
}

impl Response {
    pub fn new(session_state: impl Into<State>) -> Self {
        Self {
            method_responses: Vec::new(),
            created_ids: None,
            session_state: session_state.into(),
        }
    }

    pub fn with_method_response(mut self, invocation: Invocation) -> Self {
        self.method_responses.push(invocation);
        self
    }

    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }

    /// All responses belonging to the method call with the given call id (a
    /// single call may produce several responses).
    pub fn responses_for<'a>(&'a self, call_id: &'a str) -> impl Iterator<Item = &'a Invocation> {
        self.method_responses
            .iter()
            .filter(move |invocation| invocation.call_id == call_id)
    }
}
