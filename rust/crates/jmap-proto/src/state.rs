// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `State` and `UtcDate` primitives (RFC 8620 §1.2, §1.4).

use serde::{Deserialize, Serialize};

/// An opaque per-datatype state token; changes whenever server data changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct State(String);

impl State {
    pub fn new(state: impl Into<String>) -> Self {
        Self(state.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for State {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for State {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// A date-time in UTC, `YYYY-MM-DDTHH:MM:SSZ` (RFC 3339 subset).
///
/// Deliberately a string: Evolution Data Server converts to `GDateTime`; this
/// crate never does date arithmetic. Lexicographic order equals chronological
/// order for this format, which is all queries need.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct UtcDate(String);

impl UtcDate {
    pub fn new(date: impl Into<String>) -> Self {
        Self(date.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UtcDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for UtcDate {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
