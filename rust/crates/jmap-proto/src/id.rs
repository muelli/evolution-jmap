// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `Id` primitive (RFC 8620 §1.2).

use serde::{Deserialize, Serialize};

/// An object identifier representing an opaque server-assigned string (RFC 8620 §1.2).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Id {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Id {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Splits a shared child's identity (`<accountId>/<collectionId>`) into its two halves.
///
/// `None` means `identity` is not shared: a primary child's bare id, or a
/// value this lane does not recognise as a split. The split needs exactly
/// one `/`, with non-empty text on each side, because an `Id` (RFC 8620
/// §1.2) cannot itself contain a `/`.
pub fn split_shared_identity(identity: &str) -> Option<(&str, &str)> {
    let (account, collection) = identity.split_once('/')?;
    (!account.is_empty() && !collection.is_empty() && !collection.contains('/'))
        .then_some((account, collection))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_identity_is_split_only_when_both_halves_are_there() {
        assert_eq!(split_shared_identity("A2/C1"), Some(("A2", "C1")));
        for whole in ["C1", "/C1", "A2/", "A/B/C", ""] {
            assert_eq!(split_shared_identity(whole), None, "{whole:?}");
        }
    }
}
