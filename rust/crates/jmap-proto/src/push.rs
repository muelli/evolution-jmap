// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `StateChange` object pushed over EventSource (RFC 8620 §7.1, §7.3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::state::State;

/// One account's changed data types and their new state tokens, as named in
/// a [`StateChange`]'s `changed` map: the type name ("Mailbox", "Email", …)
/// paired with the state string a `Foo/get` call would currently answer.
pub type TypeState = BTreeMap<String, State>;

/// A server-pushed notification that something changed (RFC 8620 §7.1),
/// delivered over the `text/event-stream` resource named by the session
/// object's `eventSourceUrl` (§7.3).
///
/// Carries no more than *that* something changed and roughly *what* — a
/// recipient compares each state string against what it already has and
/// fetches only the accounts/types that moved, via the existing `/changes`
/// methods. `@type` is required to be present and MUST be the literal string
/// `"StateChange"`; nothing else is ever pushed on this channel to confuse
/// it with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateChange {
    #[serde(rename = "@type")]
    pub kind: String,
    pub changed: BTreeMap<Id, TypeState>,
}

impl StateChange {
    /// The literal `@type` value RFC 8620 §7.1 requires.
    pub const TYPE: &'static str = "StateChange";

    /// Build a `StateChange` naming `changed` accounts/types, with `@type`
    /// set to the required [`Self::TYPE`] rather than left for a caller to
    /// get wrong.
    pub fn new(changed: BTreeMap<Id, TypeState>) -> Self {
        Self {
            kind: Self::TYPE.to_owned(),
            changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_the_required_type_marker() {
        let change = StateChange::new(BTreeMap::new());
        assert_eq!(change.kind, "StateChange");
    }
}
