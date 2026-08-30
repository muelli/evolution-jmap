// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Request envelope, method invocations, and result references
//! (RFC 8620 §3.3, §3.7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

/// A JMAP API request: capabilities in use plus an ordered list of method
/// calls (RFC 8620 §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub using: Vec<String>,
    pub method_calls: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
}

impl Request {
    pub fn new(using: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            using: using.into_iter().map(Into::into).collect(),
            method_calls: Vec::new(),
            created_ids: None,
        }
    }

    /// Append a method call with typed arguments.
    pub fn call(
        mut self,
        name: impl Into<String>,
        arguments: &impl Serialize,
        call_id: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        self.method_calls
            .push(Invocation::new(name, arguments, call_id)?);
        Ok(self)
    }

    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }
}

/// One method call or response: on the wire a three-element array of
/// `[name, arguments, callId]` (RFC 8620 §3.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub name: String,
    pub arguments: Value,
    pub call_id: String,
}

impl Invocation {
    pub fn new(
        name: impl Into<String>,
        arguments: &impl Serialize,
        call_id: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            name: name.into(),
            arguments: serde_json::to_value(arguments)?,
            call_id: call_id.into(),
        })
    }

    pub fn from_value(
        name: impl Into<String>,
        arguments: Value,
        call_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            arguments,
            call_id: call_id.into(),
        }
    }

    /// Parse the arguments into a typed structure.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.arguments.clone())
    }

    /// Whether this is a method-level error response (RFC 8620 §3.6.2).
    pub fn is_error(&self) -> bool {
        self.name == "error"
    }
}

impl Serialize for Invocation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.name, &self.arguments, &self.call_id).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Invocation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (name, arguments, call_id) = <(String, Value, String)>::deserialize(deserializer)?;
        Ok(Self {
            name,
            arguments,
            call_id,
        })
    }
}

/// A reference to the result of a previous method call in the same request;
/// appears under a `#`-prefixed argument name (RFC 8620 §3.7, draft-ietf-jmap-refplus).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultReference {
    pub result_of: String,
    pub name: String,
    pub path: String,
}

impl ResultReference {
    pub fn new(
        result_of: impl Into<String>,
        name: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            result_of: result_of.into(),
            name: name.into(),
            path: path.into(),
        }
    }
}
