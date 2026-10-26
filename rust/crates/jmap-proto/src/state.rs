// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `State` and `UtcDate` primitives (RFC 8620 §1.2, §1.4).

use serde::{Deserialize, Serialize};

/// An opaque per-datatype state token; changes whenever server data changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct State(String);

impl State {
    /// Create a `State` from a string.
    pub fn new(state: impl Into<String>) -> Self {
        Self(state.into())
    }

    /// Return the state as a string slice.
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

impl AsRef<str> for State {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A date-time string in UTC format `YYYY-MM-DDTHH:MM:SSZ` (RFC 3339 subset).
/// Preserves lexicographical sorting without date arithmetic.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct UtcDate(String);

impl UtcDate {
    /// Create a `UtcDate` from a string.
    pub fn new(date: impl Into<String>) -> Self {
        Self(date.into())
    }

    /// Return the date as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UtcDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for UtcDate {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for UtcDate {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for UtcDate {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
